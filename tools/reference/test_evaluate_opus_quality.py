# SPDX-License-Identifier: MIT
"""Controls for the optional offline quality acceptance driver."""

import struct
import tempfile
import unittest
from pathlib import Path

from evaluate_opus_quality import packet_archive, summarize


class QualityControls(unittest.TestCase):
    def test_packet_archive_preserves_bytes_and_intervals(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "packets.bin"
            path.write_bytes(b"SNKQ0001" + struct.pack("<II", 3, 0x12345678) + b"abc")
            self.assertEqual(packet_archive(path, 3), [(b"abc", 0x12345678)])
            for data in [b"", b"SNKQ0001", b"SNKQ0001x",
                         b"SNKQ0001" + struct.pack("<II", 3, 1) + b"ab"]:
                path.write_bytes(data)
                with self.assertRaises(ValueError):
                    packet_archive(path, 3)
            path.write_bytes(b"SNKQ0001" + struct.pack("<II", 3, 1) + b"abc")
            with self.assertRaises(ValueError):
                packet_archive(path, 4)

    @staticmethod
    def rows(deltas):
        return [dict(condition="clean", frame_ms=20, bitrate=12000, bandwidth="WB",
                     loss="none", speaker=str(index), reference_stoi=0.9,
                     current_stoi=0.9 + delta, reference_estoi=0.8,
                     current_estoi=0.8 + delta, reference_lsd_db=5.0,
                     current_lsd_db=5.0 - delta, reference_snr_db=12.0,
                     current_snr_db=12.0 + delta)
                for index, delta in enumerate(deltas)]

    def test_positive_average_cannot_hide_an_uncertain_comparison(self):
        result = summarize(self.rows([-0.08, 0.04, 0.04, 0.04]))[0]
        self.assertGreater(result["delta_stoi"], 0)
        self.assertLess(result["delta_stoi_ci_low"], 0)
        self.assertFalse(result["objective_parity"])
        self.assertEqual(result["stoi_regressions"], 1)

    def test_each_metric_and_condition_has_to_pass(self):
        rows = self.rows([0.01, 0.02, 0.03, 0.04])
        self.assertTrue(summarize(rows)[0]["objective_parity"])
        failing = [dict(row, condition="noise10", current_lsd_db=5.1) for row in rows]
        result = summarize(rows + failing)
        self.assertEqual([row["objective_parity"] for row in result], [True, False])

    def test_single_speaker_cannot_close_the_objective_gate(self):
        self.assertFalse(summarize(self.rows([0.1]))[0]["objective_parity"])

    def test_self_control_and_bootstrap_are_reproducible(self):
        rows = self.rows([0.0, 0.0, 0.0, 0.0])
        self.assertEqual(summarize(rows), summarize(rows))
        result = summarize(rows)[0]
        self.assertTrue(result["objective_parity"])
        for metric in ("stoi", "estoi", "lsd_db", "snr_db"):
            self.assertEqual(result[f"delta_{metric}"], 0)
            self.assertEqual(result[f"delta_{metric}_ci_low"], 0)
            self.assertEqual(result[f"delta_{metric}_ci_high"], 0)


if __name__ == "__main__":
    unittest.main()
