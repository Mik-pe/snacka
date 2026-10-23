#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Stage matched accepted/cache-only sources. Never replace installed controls."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
ALLOCATION_TEST = '''
#[test]
fn target_energy_conversion_never_allocates() {
    let candidate = [0.125; FRAME_SAMPLES];
    let last = [-4.0; 16];
    let delta = [-0.1; 16];
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    for samples in [480, 960] {
        for _ in 0..64 {
            let gains = snacka::diagnostic_target_energy(black_box(&candidate[..samples]), black_box(&last), black_box(&delta));
            assert!(black_box(gains).iter().all(|g| g.is_finite() && *g >= 0.7875 - 1e-6 && *g <= 1.2125 + 1e-6));
        }
    }
    assert_eq!(ALLOCATIONS.with(|counter| counter.replace(None).unwrap()), 0);
}
'''


def replace_once(text, old, new):
    if text.count(old) != 1:
        raise ValueError(f"source anchor must occur once: {old[:60]}")
    return text.replace(old, new)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    if args.output.exists(): parser.error("fresh output required")
    args.output.mkdir(parents=True)
    receipt = dict(schema=1, kind="cache-only; installed gain/phase controls unchanged", sources={})
    original = (ROOT / "src/neural_plc.rs").read_text()
    for variant in ("accepted", "cache"):
        out = args.output / variant
        shutil.copytree(ROOT / "src", out / "src")
        shutil.copytree(ROOT / "tests", out / "tests")
        path = out / "tests/allocations.rs"
        path.write_text(path.read_text() + ALLOCATION_TEST)
        (out / "examples").mkdir()
        for name in ("plc_footprint", "cpu_probe"):
            shutil.copy2(ROOT / f"examples/{name}.rs", out / f"examples/{name}.rs")
        shutil.copy2(ROOT / "tools/plc_target_energy_probe.rs", out / "examples/plc_target_energy_probe.rs")
        shutil.copy2(ROOT / "tools/plc_target_energy.rs", out / "src/target_energy.rs")
        lib = (out / "src/lib.rs").read_text()
        lib += '\n#[allow(dead_code)]\nmod target_energy;\n'
        lib += '''
/// Offline analytic-conversion diagnostics; not model activation.
pub fn diagnostic_target_energy(candidate: &[f32], last: &[f32;16], delta: &[f32;16]) -> [f32;16] {
    target_energy::gains(candidate, last, delta)
}
'''
        if variant == "cache":
            text = replace_once(original, "    count: usize,", "    count: usize,\n    last_log_power: [f32; 16],")
            text = replace_once(text, "            count: 0,", "            count: 0,\n            last_log_power: [0.0; 16],")
            text = replace_once(text, "            frame[band] = (energy / values.len() as f32 + 1e-12).ln() * 0.5;",
                                "            let log_power = (energy / values.len() as f32 + 1e-12).ln();\n            self.last_log_power[band] = log_power;\n            frame[band] = log_power * 0.5;")
            text = replace_once(text, "    pub(crate) fn reset(&mut self) {", "    #[allow(dead_code)]\n    pub(crate) fn last_log_power(&self) -> [f32; 16] { self.last_log_power }\n\n    pub(crate) fn reset(&mut self) {")
            (out / "src/neural_plc.rs").write_text(text)
            # Same native feature implementation before insertion, not a Python facsimile.
            control = original[:original.index("fn layer<")]
            (out / "src/plc_history_control.rs").write_text(control.replace("neural_plc_weights as weights", "neural_plc_weights as _weights"))
            shutil.copy2(ROOT / "tools/plc_target_history_tests.rs", out / "src/plc_target_history_tests.rs")
            lib += '\n#[cfg(test)]\n#[allow(dead_code, unused_imports)]\nmod plc_history_control;\n#[cfg(test)]\nmod plc_target_history_tests;\n'
        (out / "src/lib.rs").write_text(lib)
        (out / "Cargo.toml").write_text('''[package]
name="snacka-target-energy-preflight"
version="0.0.0"
edition="2024"
[lib]
name="snacka"
[features]
opus=[]
reference-tests=[]
[profile.release]
overflow-checks=true
''')
        subprocess.run(["cargo", "fmt"], cwd=out, check=True)
        receipt["sources"][variant] = {str(p.relative_to(out)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted((out / "src").glob("*.rs"))}
        receipt.setdefault("allocation_test_sha256", {})[variant] = hashlib.sha256((out / "tests/allocations.rs").read_bytes()).hexdigest()
        # Changing the live PLC body, weights, or Opus is forbidden in this cache preflight.
        for name in ("codec.rs", "concealment.rs", "neural_plc_weights.rs"):
            if (out / "src" / name).read_bytes() != (ROOT / "src" / name).read_bytes():
                raise ValueError(f"unexpected production mutation: {name}")
    (args.output / "source-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__": main()
