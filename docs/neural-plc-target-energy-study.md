# Frozen target-energy × bandwidth distillation study

Execution of the [immutable method](neural-plc-target-energy-protocol.json) on current PR31, with the [pretraining execution amendment](neural-plc-target-energy-execution.json). The amendment requires the same audio guards per cell **and bitrate**, and actual online first-loss latency no worse than PR31 as well as ≤25µs. Pitch search and GRU remain excluded.

The separate data binding records the pretraining source commit, every code/data checksum, raw-power export, physical bandwidth assignment and reserved independent identities. The method file remains unchanged. Four fixed1920-update QAT fits; each has phase-off/on native applications. Every application is reported, guards precede ranking, and only one admitted winner may enter independent acceptance. No winner means retain PR31 and keep acceptance waveforms unopened.

Preparation runs on task-local CPU, with temporary copies of the existing344-clip Drive corpus. The320 training clips and24 known validation clips are reused to isolate the factors. Exactly4/8 training clips within each of24 English speakers receive the fixed soxr16→48k transform before noise/codec augmentation. Source waveform hashes, effective rate and per-record masks accompany the raw native labels. No acceptance voice is used for fitting or validation.

`SPLE2` labels carry80 causal inputs,16 cached last-received log mean powers,16 raw uninterrupted post-Phase1 target log powers,16 teacher-phase candidate log powers, phase and eligibility. The teacher always synthesizes classical candidate0 into history. Target-energy inference measures the actual student-phase candidate; the fixed teacher-phase denominator in training is an explicitly approximate supervision proxy.

Both representations use the method's common clipped-target Huber loss and identity **gain** anchor. Energy outputs are not penalized toward zero. Exported kind and numeric format tags, source-builder checks, and a Rust compile-time assertion distinguish gain and energy heads despite their identical dimensions. Ineligible/disabled paths produce gains1 and phase0 before any energy conversion.

Preflight: native64-byte cache equality/reset/silence tests; quantized raw-head parity on an existing trained checkpoint; finite converter/autograd checks; zero-allocation inference; guard-first/per-bitrate failure tests;36-case exact scorer parity against the existing evaluator. The task-local FFmpeg7.1/libsoxr0.1.3 binary and numerical environment are sealed before fitting. See execution amendment for environment differences from the older PR31 export.

Rebuild study applications with `replay_plc_energy_build.py OUTPUT --model MODEL --phase off|on`. It restores the bound source commit before invoking the checked builder. This prevents later production format guards or other main changes from entering the historical study. The reproduced native source was checked byte-for-byte against the original application.

Reproduction tools:

- `build_plc_energy_teacher.py`, `prepare_plc_energy_study.py`: native raw labels and matched factor datasets.
- `freeze_plc_energy_acceptance.py`: metadata-only fresh16k identity reservation, excluding all84 documented LibriSpeech and50 RixVox exposures. The existing SLR83 reservation is unchanged.
- `train_plc_energy_study.py`: sealed-binding check, same initialized hidden layers, final checkpoint only, typed i8 export and diagnostics.
- `build_plc_energy_candidate.py`, `build_plc_energy_checked.py`, `replay_plc_energy_build.py`: isolated native applications, applied-gain parity, allocation checks and source-bound rebuilds. The [test-only parity correction](neural-plc-target-energy-parity-correction.json) records why the auxiliary raw-output threshold was replaced with the original gain-unit criterion.
- `evaluate_plc_energy_study.py`, `select_plc_energy_study.py`: complete paired factorial metrics and locked admission/ranking.
- `measure_plc_energy_candidate.py`: interleaved actual online timing against PR31, excluding offline capture/metric loops.

Long validation can resume only complete blocks from this same run after checking selection, executable and scorer hashes; every block must include all10 implementations and18 condition/pattern pairs. This changes worker scheduling only, never the scorer or criteria. The first checkpoint retained75 complete tasks and discarded0 torn rows.

The installed gain activation now has a compile-time format1 check, and generated metadata-disabled models follow the explicit identity path. Numeric PR31 arrays are byte-identical to the frozen model. Those safety checks do not install any experimental weights or the64-byte cache.

Results and the activation decision will be appended after the frozen run. Installed production code and weights remain PR31 unless all independent audio and native gates pass.
