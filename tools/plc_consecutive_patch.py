# SPDX-License-Identifier: MIT
"""Use only on the exact source-registered scalar pitch search."""
import hashlib
BASE_SHA='bdcb137329dbdd97d0e29144a8659e906b44754fc310b4667ef14e236d1b52b6'
def apply(path,root):
 assert hashlib.sha256(path.read_bytes()).hexdigest()==BASE_SHA
 s=path.read_text()
 old="""        for candidate in 14..=160 {
            let score = correlation(&downsampled, WINDOW, candidate, 1, current_energy);
            // Near-equivalent harmonic peaks should not extend the repeated cycle.
            if score > best + 0.01 {
                best = score;
                lag = candidate;
            }
        }"""
 new="""        for first in (14..=160).step_by(4) {
            let scores=if first+3<=160 {correlations_consecutive4(&downsampled,WINDOW,first,1,current_energy)}
                else {std::array::from_fn(|lane|correlation(&downsampled,WINDOW,(first+lane).min(160),1,current_energy))};
            for lane in 0..(161-first).min(4) {
                if scores[lane]>best+0.01 {best=scores[lane];lag=first+lane;}
            }
        }"""
 assert s.count(old)==1;s=s.replace(old,new)
 old="""        for candidate in
            best_lag.saturating_sub(DECIMATION).max(80)..=(best_lag + DECIMATION).min(960)
        {
            let current = correlation(&self.history, 960, candidate, 3, current_energy);
            if current > score {
                score = current;
                best_lag = candidate;
            }
        }"""
 new="""        let first=best_lag.saturating_sub(DECIMATION).max(80);
        let last=(best_lag+DECIMATION).min(960);
        for group in (first..=last).step_by(4) {
            let scores=if group+3<=last {correlations_consecutive4(&self.history,960,group,3,current_energy)}
                else {std::array::from_fn(|lane|correlation(&self.history,960,(group+lane).min(last),3,current_energy))};
            for lane in 0..(last-group+1).min(4) {
                if scores[lane]>score {score=scores[lane];best_lag=group+lane;}
            }
        }"""
 assert s.count(old)==1;s=s.replace(old,new)
 path.write_text(s+'\n'+(root/'tools/plc_consecutive.rs').read_text())
