#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Native Step2 perceptual diagnostics; emits legacy metrics plus band and MR-STFT terms."""
import argparse
from concurrent.futures import ProcessPoolExecutor
from functools import partial
import csv
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import numpy as np
from evaluate_loss import loss_frames
from evaluate_quality import intelligibility, spectrum, spectral_distance
from evaluate_speech import noisy

PATTERNS=("periodic1","periodic3","periodic6","independent5","burst40","burst80")
QUALITY_RNG_LOCK=threading.Lock()


def magnitude_error(reference, actual, native_rate):
    errors=[]
    for n in (128,256,512):
        def magnitude(values):
            # Same centered reflect padding and periodic Hann as torch.stft.
            padded=np.pad(values,(n//2,n//2),mode="reflect")
            frames=np.lib.stride_tricks.sliding_window_view(padded,n)[::n//4]
            window=0.5-0.5*np.cos(2*np.pi*np.arange(n)/n)
            result=np.abs(np.fft.rfft(frames*window,axis=1))
            return result[:,:int(8000*n/48000)+1] if native_rate==16000 else result
        target=magnitude(reference);prediction=magnitude(actual)
        scale=max(float(target.mean()),1e-5);target=target/scale;prediction=prediction/scale
        errors.append(float(np.mean(np.abs(prediction-target))+np.mean(np.abs(np.log(prediction+0.01)-np.log(target+0.01)))))
    return float(np.mean(errors))


def snr(reference, actual, selected):
    if not selected.any(): return None
    ref=reference[selected].astype(np.float64)
    error=ref-actual[selected]
    power=np.dot(ref,ref); mse=np.dot(error,error)
    return float(10*np.log10(power/mse)) if power>0 and mse>0 else None


def job(row, corpus, binary, bitrate, duration, harmonic, quality, baseline_probe, bands_only=False):
    source=corpus/row["path"]
    if hashlib.sha256(source.read_bytes()).hexdigest()!=row["flac_sha256"]:raise ValueError("source hash differs")
    pcm=subprocess.check_output(["ffmpeg","-v","error","-i",str(source),"-ar","48000","-ac","1","-f","f32le","pipe:1"])
    resampled=np.frombuffer(pcm,dtype="<f4")
    resampling_clipped=int((np.abs(resampled)>1).sum())
    pcm=np.clip(resampled,-1,1).astype("<f4").tobytes()
    clean=np.frombuffer(pcm,dtype="<f4").astype(np.float64)
    rows=[]
    with tempfile.TemporaryDirectory(prefix="snacka-plc-eval-") as temporary:
        root=Path(temporary); input_path=root/"input.f32"
        samples=48*duration
        frame=(np.arange(len(clean))+120)//samples
        offset=(np.arange(len(clean))+120)%samples
        for condition in ("clean","noise20","noise10"):
            data=pcm if condition=="clean" else noisy(clean,source.stem,int(condition[5:]))[0]
            input_path.write_bytes(data)
            original=np.frombuffer(data,dtype="<f4").astype(np.float64)
            masks={}
            for pattern in PATTERNS:
                dropped=loss_frames(source.stem,duration,(len(clean)+samples-1)//samples,pattern)
                path=root/(pattern+".txt");path.write_text("".join(f"{i}\n" for i in sorted(dropped)))
                masks[pattern]=(dropped,path)
            output=root/condition
            command=[str(binary),str(input_path),str(output),str(bitrate),str(duration),str(int(harmonic))]
            replay=subprocess.run(command+[str(v[1]) for v in masks.values()],capture_output=True,text=True)
            if replay.returncode:
                raise RuntimeError(f"native replay failed for {source.name}, {bitrate}/{duration}/{harmonic}/{condition}: {replay.stderr}")
            reference=np.fromfile(output/"uninterrupted.baseline.f32",dtype="<f4")
            current_uninterrupted=np.fromfile(output/"uninterrupted.current.f32",dtype="<f4")
            if reference.tobytes()!=current_uninterrupted.tobytes():raise ValueError("uninterrupted PCM changed")
            if baseline_probe:
                for label,exe in [("baseline",baseline_probe),("current",binary.parent/"cpu_probe")]:
                    subprocess.run([str(exe),"trace",str(input_path),str(root/(label+".trace")),str(bitrate),str(duration)]+(["--harmonic"] if harmonic else []),check=True,capture_output=True)
                if (root/"baseline.trace").read_bytes()!=(root/"current.trace").read_bytes():
                    raise ValueError("frozen main packet/uninterrupted trace differs")
                frozen_loss=root/"frozen-loss.f32"
                subprocess.run([str(baseline_probe.parent/"roundtrip"),str(input_path),str(frozen_loss),str(bitrate),str(duration),"--loss-frames",str(masks["periodic3"][1])]+(["--harmonic"] if harmonic else []),check=True,capture_output=True)
                if frozen_loss.read_bytes()!=(output/"periodic3.baseline.f32").read_bytes():
                    raise ValueError("baseline PLC differs from frozen main")
            with (output/"band-metrics.csv").open() as stream:
                band_rows=list(csv.DictReader(stream))
            spec=spectrum(original) if quality else None
            for pattern,(dropped,_) in masks.items():
                lost=np.isin(frame,list(dropped));previous=np.isin(frame-1,list(dropped))
                onset=lost & ~previous & (offset<120)
                recovery=~lost & previous & (offset<120)
                received=~lost & ~recovery
                result=dict(clip=source.stem,source=row["source"],group_id=row["group_id"],language=row["language"],
                            native_rate=row["native_rate"],condition=condition,bitrate=bitrate,frame_ms=duration,
                            harmonic=harmonic,pattern=pattern,lost_samples=int(lost.sum()),resampling_clipped_samples=resampling_clipped,
                            input_sha256=hashlib.sha256(data).hexdigest())
                outputs={}
                for label in ("baseline","current"):
                    values=np.fromfile(output/(pattern+"."+label+".f32"),dtype="<f4")
                    if len(values)!=len(reference) or not np.isfinite(values).all() or (np.abs(values)>1).any():raise ValueError("invalid decoded PCM")
                    if not np.array_equal(values.view("<u4")[received],reference.view("<u4")[received]):raise ValueError("loss error propagated beyond 120-sample recovery")
                    outputs[label]=values
                    stats=[r for r in band_rows if r["pattern"]==pattern and r["implementation"]==label]
                    if len(stats)!=16:raise ValueError("missing native band statistics")
                    result[f"{label}_lowband_mdct_error_energy"]=sum(float(r["error_energy"]) for r in stats if int(r["band"])<7)
                    last=13 if row["native_rate"]==16000 else 16
                    supported=[r for r in stats if int(r["band"])<last]
                    result[f"{label}_log_envelope_squared_sum"]=sum(float(r["log_energy_squared"]) for r in supported)
                    result[f"{label}_log_envelope_count"]=sum(float(r["count"]) for r in supported)
                    high=[r for r in supported if int(r["band"])>=7]
                    result[f"{label}_highband_log_envelope_squared_sum"]=sum(float(r["log_energy_squared"]) for r in high)
                    result[f"{label}_highband_log_envelope_count"]=sum(float(r["count"]) for r in high)
                    result[f"{label}_mrstft_magnitude_error"]=None if bands_only else magnitude_error(reference,values.astype(np.float64),row["native_rate"])
                    result[f"{label}_lost_error_energy"]=float(np.sum((reference[lost].astype(np.float64)-values[lost])**2))
                    for name,selected in [("lost",lost),("onset",onset),("recovery",recovery),("whole",np.ones_like(lost))]:
                        result[f"{label}_{name}_snr_db"]=snr(reference,values,selected)
                    if quality:
                        # The legacy scorer seeds NumPy's global RNG and changes
                        # warning filters for pystoi dithering; serialize it.
                        with QUALITY_RNG_LOCK:
                            scores=intelligibility(clean,values.astype(np.float64),source.stem+condition)
                        result.update({f"{label}_{name}":float(value) for name,value in scores.items()})
                        result[f"{label}_lsd_db"]=float(spectral_distance(spec,values.astype(np.float64)))
                result["changed_loss_pcm"]=outputs["baseline"].tobytes()!=outputs["current"].tobytes()
                for metric in ["lost_snr_db","onset_snr_db","recovery_snr_db","whole_snr_db"]+(["stoi","estoi","lsd_db"] if quality else []):
                    before,after=result[f"baseline_{metric}"],result[f"current_{metric}"]
                    result["delta_"+metric]=after-before if before is not None and after is not None else None
                rows.append(result)
    return rows


def execute_task(item,binary,quality,baseline_probe,patterns,bands_only=False):
    global PATTERNS
    PATTERNS=tuple(patterns)
    row,rate,duration,harmonic=item
    return job(row,Path(row["corpus_root"]),binary,rate,duration,harmonic,quality,baseline_probe,bands_only)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("selection",type=Path,help="JSON list of manifest rows, each with corpus_root")
    parser.add_argument("output",type=Path)
    parser.add_argument("--binary",type=Path,required=True)
    parser.add_argument("--baseline-probe",type=Path)
    parser.add_argument("--bitrates",type=int,nargs="+",default=[12000,24000,32000])
    parser.add_argument("--durations",type=int,nargs="+",default=[10,20])
    parser.add_argument("--modes",type=int,nargs="+",default=[0,1])
    parser.add_argument("--workers",type=int,default=4)
    parser.add_argument("--quality",action="store_true")
    parser.add_argument("--patterns",nargs="+",choices=PATTERNS,default=list(PATTERNS))
    parser.add_argument("--native-bands-only",action="store_true",help="separate native-band supplement; omit STOI/LSD/MR-STFT")
    args=parser.parse_args()
    if args.native_bands_only and args.quality:parser.error("native-band supplements omit the quality scorers")
    selection=json.loads(args.selection.read_text())
    tasks=[(r,rate,duration,bool(mode)) for r in selection for rate in args.bitrates for duration in args.durations for mode in args.modes]
    results=[]
    execute=partial(execute_task,binary=args.binary.resolve(),quality=args.quality,baseline_probe=args.baseline_probe,patterns=args.patterns,bands_only=args.native_bands_only)
    args.output.with_suffix(".json").write_text(json.dumps({"status":"INCOMPLETE"})+"\n")
    with args.output.open("w") as stream,ProcessPoolExecutor(max_workers=args.workers) as pool:
        writer=None
        for index,rows in enumerate(pool.map(execute,tasks)):
            if writer is None:
                writer=csv.DictWriter(stream,fieldnames=list(rows[0]));writer.writeheader()
            writer.writerows(rows);stream.flush();results.extend(rows)
            print(f"completed {index+1}/{len(tasks)}, rows={len(results)}",flush=True)
    summary={"status":"COMPLETE","rows":len(results),"changed_rows":sum(r["changed_loss_pcm"] for r in results),
             "selection_sha256":hashlib.sha256(args.selection.read_bytes()).hexdigest(),
             "quality_sha256":hashlib.sha256(args.output.read_bytes()).hexdigest(),
             "frozen_trace_checked":bool(args.baseline_probe),
             "baseline_probe_sha256":hashlib.sha256(args.baseline_probe.read_bytes()).hexdigest() if args.baseline_probe else None,
             "binary_sha256":hashlib.sha256(args.binary.read_bytes()).hexdigest(),"strata":[]}
    for key in sorted({(r["language"],r["harmonic"],r["frame_ms"]) for r in results}):
        subset=[r for r in results if (r["language"],r["harmonic"],r["frame_ms"])==key]
        row=dict(language=key[0],harmonic=key[1],frame_ms=key[2],rows=len(subset))
        for metric in ["lost_snr_db","onset_snr_db","recovery_snr_db","whole_snr_db"]+(["stoi","estoi","lsd_db"] if args.quality else []):
            values=[r["delta_"+metric] for r in subset if r["delta_"+metric] is not None]
            row["mean_delta_"+metric]=float(np.mean(values)) if values else None
        summary["strata"].append(row)
    args.output.with_suffix(".json").write_text(json.dumps(summary,indent=2)+"\n")
    print(json.dumps(summary,indent=2),flush=True)


if __name__=="__main__":main()
