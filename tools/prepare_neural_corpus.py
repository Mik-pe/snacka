#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build a native-rate, split-frozen raw corpus; no model training or credentials.

Requires ffmpeg/ffprobe. Default pilot: 108 Swedish + 72 English utterances.
Use --full on a cloud CPU to remove caps. Output is intentionally outside Git.
"""
import argparse
from collections import Counter, OrderedDict
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile
import time
import urllib.error
import urllib.request
import zipfile

try:
    import soundfile as sf
except ImportError:
    sf = None

REVISION = "70bb2e84b976b7e960aa89f1c648e09c59f894dd"
FLEURS = f"https://huggingface.co/datasets/google/fleurs/resolve/{REVISION}"
VCTK = "https://datashare.ed.ac.uk/server/api/core/bitstreams/535f4286-e54c-4038-838c-a02285e32cb2/content"
VCTK_SIZE = 11_747_302_977
BASELINE = "1abd3a60c2775b712508dadbae6de1b40f962ee6"
SALT = "snacka-neural-v2:2026-10-21:"
MAX_AUDIO_BYTES = 32 * 1024 * 1024


def digest(data):
    return hashlib.sha256(data).hexdigest()


def order(key):
    return digest((SALT + key).encode())


def split_for(source, group):
    bucket = int(order(f"{source}:{group}")[:8], 16) % 100
    if source == "fleurs-sv":
        return "train" if bucket < 90 else "validation"
    return "train" if bucket < 80 else "validation" if bucket < 90 else "test"


def request(url, headers=None):
    for attempt in range(4):
        try:
            return urllib.request.urlopen(urllib.request.Request(
                url, headers={"User-Agent": "SnackaCorpus/2.0", **(headers or {})}
            ), timeout=90)
        except (urllib.error.URLError, TimeoutError):
            if attempt == 3:
                raise
            time.sleep(2 ** attempt)


def fetch_small(url, limit=4 * 1024 * 1024):
    with request(url) as response:
        data = response.read(limit + 1)
    if len(data) > limit:
        raise ValueError("metadata exceeded limit")
    return data


class RangeReader(io.RawIOBase):
    """Seek the publisher ZIP without silently downloading its 11.75 GB body."""
    def __init__(self, url, size):
        self.url, self.size, self.position = url, size, 0
        self.cache = OrderedDict()
        self.chunk_size = 512 * 1024
        self.transferred = 0

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.position

    def seek(self, offset, whence=0):
        target = offset if whence == 0 else self.position + offset if whence == 1 else self.size + offset
        if target < 0:
            raise ValueError("negative seek")
        self.position = target
        return target

    def block(self, index):
        if index in self.cache:
            self.cache.move_to_end(index)
            return self.cache[index]
        data = self.fetch_block(index)
        self.cache[index] = data
        if len(self.cache) > 16:
            self.cache.popitem(last=False)
        return data

    def fetch_block(self, index):
        start = index * self.chunk_size
        end = min(start + self.chunk_size, self.size) - 1
        # Distinct query keys also prevent intermediary caches ignoring Range.
        url = self.url + f"?snacka_range={start}-{end}"
        with request(url, {"Range": f"bytes={start}-{end}"}) as response:
            expected = f"bytes {start}-{end}/{self.size}"
            if response.status != 206 or response.headers.get("Content-Range") != expected:
                raise ValueError("server did not honor the exact byte range")
            data = response.read(end - start + 2)
        if len(data) != end - start + 1:
            raise ValueError("truncated/oversized ZIP range")
        self.transferred += len(data)
        return data

    def prefetch(self, members):
        indices = set()
        for member in members:
            first = member.header_offset // self.chunk_size
            last = min(self.size - 1, member.header_offset + member.compress_size + 2048) // self.chunk_size
            indices.update(range(first, last + 1))
        ordered = sorted(indices)
        with ThreadPoolExecutor(max_workers=12) as pool:
            blocks = list(pool.map(self.fetch_block, ordered))
        self.cache = OrderedDict(zip(ordered, blocks))

    def read(self, size=-1):
        end = self.size if size < 0 else min(self.position + size, self.size)
        chunks = []
        while self.position < end:
            index, offset = divmod(self.position, self.chunk_size)
            block = self.block(index)
            take = min(end - self.position, len(block) - offset)
            chunks.append(block[offset:offset + take])
            self.position += take
        return b"".join(chunks)


class Corpus:
    def __init__(self, root):
        self.root = root
        self.rows = []
        self.pcm_seen = {}

    def add(self, data, source, member, group, split, rate, language, **extra):
        if not data or len(data) > MAX_AUDIO_BYTES:
            raise ValueError("audio member is empty or exceeds size limit")
        name = re.sub(r"[^a-zA-Z0-9_.-]", "_", Path(member).stem) + ".flac"
        relative = Path("audio") / split / source / name
        target = self.root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            raise FileExistsError(target)
        # FLAC stores PCM24 at the native rate; source float WAV is quantized.
        # libsndfile avoids spawning three ffmpeg processes per cloud utterance.
        if sf is not None:
            values, actual_rate = sf.read(io.BytesIO(data), dtype="float32", always_2d=True)
            if actual_rate != rate:
                raise ValueError(f"unexpected source sample rate: {member}")
            stream = io.BytesIO()
            sf.write(stream, values.mean(axis=1), rate, format="FLAC", subtype="PCM_24")
            converted = stream.getvalue()
            target.write_bytes(converted)
            recovered, actual_rate = sf.read(target, dtype="float32")
            pcm = recovered.astype("<f4").tobytes()
        else:
            converted = subprocess.run([
            "ffmpeg", "-v", "error", "-i", "pipe:0", "-ac", "1",
            "-c:a", "flac", "-f", "flac", "pipe:1"
            ], input=data, capture_output=True, check=True).stdout
            target.write_bytes(converted)
            probe = json.loads(subprocess.check_output([
            "ffprobe", "-v", "error", "-show_entries",
            "stream=sample_rate,channels:format=duration", "-of", "json", str(target)
            ]))
            stream = probe["streams"][0]
            if int(stream["sample_rate"]) != rate or stream["channels"] != 1:
                raise ValueError(f"unexpected native audio format: {member}")
            # Decode canonical mono f32 to catch duplicate audio across containers.
            pcm = subprocess.check_output([
            "ffmpeg", "-v", "error", "-i", str(target), "-f", "f32le", "pipe:1"
            ])
        pcm_sha = digest(pcm)
        if pcm_sha in self.pcm_seen:
            previous = self.pcm_seen[pcm_sha]
            if previous != split:
                raise ValueError(f"duplicate PCM across splits: {member}")
            target.unlink()
            return False
        self.pcm_seen[pcm_sha] = split
        self.rows.append(dict(
            path=relative.as_posix(), source=source, source_member=member,
            group_id=group, split=split, language=language, native_rate=rate,
            samples=len(pcm) // 4, seconds=len(pcm) / (4 * rate),
            source_sha256=digest(data), flac_sha256=digest(converted),
            pcm_f32_sha256=pcm_sha, license="CC-BY-4.0", **extra
        ))
        return True

    def fleurs(self, full=False):
        metadata = fetch_small(FLEURS + "/data/sv_se/train.tsv")
        (self.root / "sources" / "fleurs-sv-train.tsv").write_bytes(metadata)
        mapping = {}
        for line in metadata.decode().splitlines():
            fields = line.split("\t")
            if len(fields) >= 2:
                mapping[fields[1]] = fields[0]
        counts = Counter()
        caps = {"train": 96, "validation": 12}
        url = FLEURS + "/data/sv_se/audio/train.tar.gz"
        with request(url) as response, tarfile.open(fileobj=response, mode="r|gz") as archive:
            for member in archive:
                if not member.isfile() or not member.name.endswith(".wav"):
                    continue
                sentence = mapping[Path(member.name).name]
                split = split_for("fleurs-sv", sentence)
                if not full and counts[split] >= caps[split]:
                    continue
                if member.size > MAX_AUDIO_BYTES:
                    raise ValueError("oversized FLEURS member")
                with archive.extractfile(member) as audio:
                    added = self.add(audio.read(), "fleurs-sv", member.name, sentence,
                                     split, 16000, "sv", revision=REVISION,
                                     split_claim="sentence-group; not speaker-disjoint")
                counts[split] += added
                if sum(counts.values()) % 24 == 0:
                    print("FLEURS", dict(counts), flush=True)
                if not full and all(counts[s] >= cap for s, cap in caps.items()):
                    break
        if not full and any(counts[s] < cap for s, cap in caps.items()):
            raise ValueError(f"FLEURS pilot incomplete: {counts}")
        print("FLEURS complete", dict(counts), flush=True)

    def vctk(self, full=False, training=False, local_archive=None):
        reader = RangeReader(VCTK, VCTK_SIZE)
        with zipfile.ZipFile(local_archive or reader) as archive:
            speakers = {}
            for member in archive.infolist():
                match = re.search(r"wav48_silence_trimmed/(p\d+)/[^/]+_mic2\.flac$", member.filename)
                if match and match[1] not in {"p280", "p315"}:
                    speakers.setdefault(match[1], []).append(member)
            if not speakers:
                raise ValueError("VCTK ZIP structure changed")
            split_map = {speaker: split_for("vctk", speaker) for speaker in speakers}
            (self.root / "sources" / "vctk-speaker-splits.json").write_text(
                json.dumps(split_map, sort_keys=True, indent=2) + "\n")
            selected = []
            caps = {"train": 12, "validation": 3, "test": 3}
            for split, cap in caps.items():
                candidates = sorted((s for s in speakers if split_map[s] == split), key=order)
                if not full and len(candidates) < cap:
                    raise ValueError(f"insufficient VCTK {split} speakers")
                selected.extend(candidates if full or training else candidates[:cap])
            for speaker in selected:
                members = sorted(speakers[speaker], key=lambda m: order(m.filename))
                chosen = members if full else members[:32 if training else 4]
                if training and not local_archive:
                    reader.prefetch(chosen)
                for member in chosen:
                    if member.file_size > MAX_AUDIO_BYTES:
                        raise ValueError("oversized VCTK member")
                    self.add(archive.read(member), "vctk", member.filename, speaker,
                             split_map[speaker], 48000, "en", microphone="mic2",
                             revision="0.92", split_claim="speaker-disjoint")
                print("VCTK", speaker, split_map[speaker], len(self.rows), flush=True)
        print(f"VCTK ZIP fetched bytes: {reader.transferred:,} / {VCTK_SIZE:,}", flush=True)

    def finish(self, full, training=False):
        groups = {}
        for row in self.rows:
            key = (row["source"], row["group_id"])
            if key in groups and groups[key] != row["split"]:
                raise ValueError("group leakage across splits")
            groups[key] = row["split"]
        manifest = "".join(json.dumps(row, sort_keys=True) + "\n" for row in self.rows)
        (self.root / "manifest.jsonl").write_text(manifest)
        summary = dict(schema_version=1, baseline=BASELINE, seed_salt=SALT,
                       full=full, training=training, clips=len(self.rows), manifest_sha256=digest(manifest.encode()),
                       strata=[])
        for source, split in sorted({(r["source"], r["split"]) for r in self.rows}):
            rows = [r for r in self.rows if (r["source"], r["split"]) == (source, split)]
            summary["strata"].append(dict(source=source, split=split, clips=len(rows),
                groups=len({r["group_id"] for r in rows}),
                seconds=sum(r["seconds"] for r in rows), native_rate=rows[0]["native_rate"]))
        (self.root / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
        print(json.dumps(summary, indent=2), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--full", action="store_true", help="remove pilot caps; use cloud CPU/disk")
    parser.add_argument("--training", action="store_true", help="all Swedish train audio and 32 utterances per VCTK speaker")
    parser.add_argument("--source", choices=["all", "fleurs", "vctk"], default="all")
    parser.add_argument("--vctk-archive", type=Path, help="optional verified local publisher ZIP on cloud disk")
    parser.add_argument("--metadata-cache", type=Path, help="publisher metadata staged when a cloud IP is blocked")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists; select a fresh directory to preserve prior data")
    args.output.mkdir(parents=True)
    (args.output / "sources").mkdir()
    sources = {
        "fleurs": dict(revision=REVISION, audio=FLEURS + "/data/sv_se/audio/train.tar.gz",
                       card="https://huggingface.co/datasets/google/fleurs", license="CC-BY-4.0"),
        "vctk": dict(version="0.92", archive=VCTK, archive_bytes=VCTK_SIZE,
                     publisher_md5="8a6ba2946b36fcbef0212cad601f4bfa",
                     card="https://doi.org/10.7488/ds/2645", license="CC-BY-4.0",
                     archive_validation="per-member ZIP CRC and SHA256; full ZIP MD5 not fetched"),
    }
    (args.output / "sources" / "sources.json").write_text(json.dumps(sources, indent=2) + "\n")
    for name,url in [
        ("FLEURS-dataset-card.md",FLEURS + "/README.md"),
        ("VCTK-README.txt","https://datashare.ed.ac.uk/server/api/core/bitstreams/bb7edd96-5d96-4c0e-8989-1e45597e7b72/content"),
        ("CC-BY-4.0.txt","https://creativecommons.org/licenses/by/4.0/legalcode.txt")]:
        cached = args.metadata_cache/name if args.metadata_cache else None
        (args.output/"sources"/name).write_bytes(cached.read_bytes() if cached and cached.is_file() else fetch_small(url))
    (args.output / "README.txt").write_text(
        "Snacka neural v2 raw-audio pilot; NOT extracted PLC training features.\n"
        "FLEURS Swedish: Google, CC BY 4.0, pinned revision in sources.json.\n"
        "VCTK 0.92: Yamagishi, Veaux, MacDonald, University of Edinburgh, 2019.\n"
        "CC BY 4.0, DOI 10.7488/ds/2645. Attribution and license retained in sources/.\n"
        "Do not train on audio/test. Validation is for selection; test is for final acceptance.\n"
        "FLEURS has sentence-group separation, NOT verified speaker separation.\n"
        "Native rates are preserved: Swedish 16 kHz; English VCTK 48 kHz.\n"
        "No noise, crops or resampling yet; augment only after splitting.\n")
    corpus = Corpus(args.output)
    if args.vctk_archive:
        if args.vctk_archive.stat().st_size != VCTK_SIZE:
            raise ValueError("VCTK archive size differs from pinned publisher metadata")
        md5 = hashlib.md5()
        with args.vctk_archive.open("rb") as stream:
            for chunk in iter(lambda: stream.read(8 * 1024 * 1024), b""):
                md5.update(chunk)
        if md5.hexdigest() != sources["vctk"]["publisher_md5"]:
            raise ValueError("VCTK publisher MD5 mismatch")
    if args.source in ("all", "fleurs"):
        corpus.fleurs(args.full or args.training)
    if args.source in ("all", "vctk"):
        corpus.vctk(args.full, args.training, args.vctk_archive)
    corpus.finish(args.full, args.training)


if __name__ == "__main__":
    main()
