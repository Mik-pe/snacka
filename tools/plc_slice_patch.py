# SPDX-License-Identifier: MIT
"""Preserve scalar addition order while removing per-sample lag indexing."""
import hashlib
BASE_SHA="bdcb137329dbdd97d0e29144a8659e906b44754fc310b4667ef14e236d1b52b6"
def apply(path,root):
 assert hashlib.sha256(path.read_bytes()).hexdigest()==BASE_SHA
 s=path.read_text()
 old="""    for index in (start..samples.len()).step_by(step) {
        let current = samples[index];
        let previous = samples[index - lag];
        product += current * previous;
        previous_energy += previous * previous;
    }"""
 new="""    for (&current, &previous) in samples[start..].iter().step_by(step)
        .zip(samples[start-lag..samples.len()-lag].iter().step_by(step)) {
        product += current * previous;
        previous_energy += previous * previous;
    }"""
 assert s.count(old)==1
 path.write_text(s.replace(old,new))
