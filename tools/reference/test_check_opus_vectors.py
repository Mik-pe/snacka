# SPDX-License-Identifier: MIT
"""Offline tests for complete-suite and selected-reference integrity checks."""

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "check_opus_vectors", Path(__file__).with_name("check_opus_vectors.py")
)
VECTORS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VECTORS)


class OfficialSuiteIntegrity(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        hashes = {"bit": [], "normal": [], "no-phase": []}
        for index in range(1, 13):
            stem = f"testvector{index:02}"
            for kind, suffix in (("bit", ".bit"), ("normal", ".dec"), ("no-phase", "m.dec")):
                content = f"{kind}:{index}".encode()
                (self.directory / (stem + suffix)).write_bytes(content)
                hashes[kind].append(hashlib.sha1(content).hexdigest())
        for patch in (
            mock.patch.object(VECTORS, "BIT_SHA1", tuple(hashes["bit"])),
            mock.patch.object(VECTORS, "DECODED_SHA1", {
                "normal": tuple(hashes["normal"]), "no-phase": tuple(hashes["no-phase"]),
            }),
        ):
            patch.start()
            self.addCleanup(patch.stop)

    def test_complete_sets_are_ordered_and_all_selected_hashes_verified(self):
        for variant, suffix in (("normal", ".dec"), ("no-phase", "m.dec")):
            vectors, references, manifest = VECTORS.verify_vectors(self.directory, variant)
            self.assertEqual([path.name for path in vectors], [f"testvector{index:02}.bit" for index in range(1, 13)])
            self.assertEqual(len(manifest), 24)
            self.assertEqual(references[vectors[0]].name, "testvector01" + suffix)

    def test_partial_bitstream_suite_is_rejected(self):
        (self.directory / "testvector12.bit").unlink()
        with self.assertRaisesRegex(ValueError, "missing=.*testvector12.bit"):
            VECTORS.verify_vectors(self.directory, "normal")

    def test_additional_bitstream_is_rejected(self):
        (self.directory / "testvector13.bit").write_bytes(b"unexpected")
        with self.assertRaisesRegex(ValueError, "extra=.*testvector13.bit"):
            VECTORS.verify_vectors(self.directory, "normal")

    def test_missing_selected_pcm_reference_is_rejected(self):
        (self.directory / "testvector01m.dec").unlink()
        with self.assertRaisesRegex(ValueError, "missing=.*testvector01m.dec"):
            VECTORS.verify_vectors(self.directory, "no-phase")
        VECTORS.verify_vectors(self.directory, "normal")

    def test_changed_bitstream_is_rejected(self):
        (self.directory / "testvector06.bit").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "SHA-1 mismatch for testvector06.bit"):
            VECTORS.verify_vectors(self.directory, "normal")

    def test_changed_selected_pcm_reference_is_rejected(self):
        for variant, suffix in (("normal", ".dec"), ("no-phase", "m.dec")):
            with self.subTest(variant=variant):
                file = self.directory / ("testvector09" + suffix)
                original = file.read_bytes()
                file.write_bytes(b"changed")
                with self.assertRaisesRegex(ValueError, "SHA-1 mismatch for testvector09"):
                    VECTORS.verify_vectors(self.directory, variant)
                file.write_bytes(original)

    def test_additional_selected_pcm_is_rejected(self):
        (self.directory / "testvector13m.dec").write_bytes(b"unexpected")
        with self.assertRaisesRegex(ValueError, "extra=.*testvector13m.dec"):
            VECTORS.verify_vectors(self.directory, "no-phase")

    def test_runner_selects_matching_phase_policy_for_every_reference(self):
        for variant in ("normal", "no-phase"):
            destination = self.directory / variant
            arguments = ["check_opus_vectors.py", str(self.directory),
                         "--opus-compare", "/unused/opus_compare", "--probe", "/unused/probe",
                         "--output-dir", str(destination), "--reference-set", variant]
            response = SimpleNamespace(returncode=0, stderr="", stdout=json.dumps({
                "complete": True, "phase_inversion": variant == "normal",
            }))
            with mock.patch.object(VECTORS.sys, "argv", arguments), \
                    mock.patch.object(VECTORS.sys, "stdout", io.StringIO()), \
                    mock.patch.object(VECTORS.subprocess, "run", return_value=response) as run, \
                    mock.patch.object(VECTORS, "compare", return_value={"passed": True, "quality": 99}):
                self.assertEqual(VECTORS.main(), 0)
            self.assertEqual(len(run.call_args_list), 24)
            self.assertTrue(all(call.args[0][-1] == variant for call in run.call_args_list))
            self.assertTrue(json.loads((destination / "summary.json").read_text())["passed"])

    def test_runner_rejects_probe_with_wrong_phase_policy(self):
        destination = self.directory / "mismatch"
        arguments = ["check_opus_vectors.py", str(self.directory),
                     "--opus-compare", "/unused/opus_compare", "--probe", "/unused/probe",
                     "--output-dir", str(destination)]
        response = SimpleNamespace(returncode=0, stderr="", stdout=json.dumps({
            "complete": True, "phase_inversion": False,
        }))
        with mock.patch.object(VECTORS.sys, "argv", arguments), \
                mock.patch.object(VECTORS.sys, "stdout", io.StringIO()), \
                mock.patch.object(VECTORS.subprocess, "run", return_value=response), \
                mock.patch.object(VECTORS, "compare", return_value={"passed": True, "quality": 99}):
            self.assertEqual(VECTORS.main(), 1)
        self.assertFalse(json.loads((destination / "summary.json").read_text())["passed"])


if __name__ == "__main__":
    unittest.main()
