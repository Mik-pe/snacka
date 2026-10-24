# Frozen target-energy × bandwidth distillation study

Execution of the [immutable method](neural-plc-target-energy-protocol.json) on current PR31, with the [pretraining execution amendment](neural-plc-target-energy-execution.json). The amendment requires the same audio guards per cell **and bitrate**, and actual online first-loss latency no worse than PR31 as well as ≤25µs. Pitch search and GRU remain excluded.

The separate data binding records the pretraining source commit, every code/data checksum, raw-power export, physical bandwidth assignment and reserved independent identities. The method file remains unchanged. Four fixed1920-update QAT fits; each has phase-off/on native applications. Every application is reported, guards precede ranking, and only one admitted winner may enter independent acceptance. No winner means retain PR31 and keep acceptance waveforms unopened.

Preparation runs on task-local CPU, with temporary copies of the existing344-clip Drive corpus. The320 training clips and 24 known validation clips are reused to isolate the factors. Exactly 4/8 training clips within each of 24 English speakers receive the fixed soxr 16→48 kHz transform before noise/codec augmentation. Source waveform hashes, effective rate and per-record masks accompany the raw native labels. No acceptance voice is used for fitting or validation.

`SPLE2` labels carry 80 causal inputs, 16 cached last-received log mean powers, 16 raw uninterrupted post-Phase1 target log powers, 16 teacher-phase candidate log powers, phase and eligibility. The teacher always synthesizes classical candidate0 into history. Target-energy inference measures the actual student-phase candidate; the fixed teacher-phase denominator in training is an explicitly approximate supervision proxy.

Both representations use the method's common clipped-target Huber loss and identity **gain** anchor. Energy outputs are not penalized toward zero. Exported kind and numeric format tags, source-builder checks, and a Rust compile-time assertion distinguish gain and energy heads despite their identical dimensions. Ineligible/disabled paths produce gains1 and phase0 before any energy conversion.

Preflight: native64-byte cache equality/reset/silence tests; quantized raw-head parity on an existing trained checkpoint; finite converter/autograd checks; zero-allocation inference; guard-first/per-bitrate failure tests;36-case exact scorer parity against the existing evaluator. The task-local FFmpeg7.1/libsoxr0.1.3 binary and numerical environment are sealed before fitting. See execution amendment for environment differences from the older PR31 export.

Rebuild study applications with `replay_plc_energy_build.py OUTPUT --model MODEL --phase off|on`. It restores the bound source commit before invoking the checked builder. This prevents later production format guards or other main changes from entering the historical study. The reproduced native source was checked byte-for-byte against the original application.

Reproduction tools:

- `build_plc_energy_teacher.py`, `prepare_plc_energy_study.py`: native raw labels and matched factor datasets.
- `freeze_plc_energy_acceptance.py`: metadata-only fresh 16 kHz identity reservation, excluding all 84 documented LibriSpeech and 50 RixVox exposures. The existing SLR83 reservation is unchanged.
- `train_plc_energy_study.py`: sealed-binding check, same initialized hidden layers, final checkpoint only, typed i8 export and diagnostics.
- `build_plc_energy_candidate.py`, `build_plc_energy_checked.py`, `replay_plc_energy_build.py`: isolated native applications, applied-gain parity, allocation checks and source-bound rebuilds. The [test-only parity correction](neural-plc-target-energy-parity-correction.json) records why the auxiliary raw-output threshold was replaced with the original gain-unit criterion.
- `evaluate_plc_energy_study.py`, `select_plc_energy_study.py`: complete paired factorial metrics and locked admission/ranking.
- `measure_plc_energy_candidate.py`: interleaved actual online timing against PR31, excluding offline capture/metric loops.

Long validation can resume only complete blocks from this same run after checking selection, executable and scorer hashes; every block must include all 10 implementations and 18 condition/pattern pairs. This changes worker scheduling only, never the scorer or criteria. The first checkpoint retained 75 complete tasks and discarded 0 torn rows.

The installed gain activation now has a compile-time format1 check, and generated metadata-disabled models follow the explicit identity path. Numeric PR31 arrays are byte-identical to the frozen model. Those safety checks do not install any experimental weights or the 64-byte cache.

**Decision: retain the installed PR31 model. None of the eight applications passed the frozen validation gate. No candidate was selected, no new acceptance waveform was requested, and the experimental latency and independent acceptance gates were not opened.** The four fixed fits and full 77,760-row decoded comparison are complete. The 64-byte cache remains experimental; only format safety and explicit metadata-disabled fallback enter the installed gain decoder. No numeric PR31 parameter changed.

## Locked validation results

Fixed fits: four, each 1,920 updates; total measured training time 4.645 s on CPU. Every candidate has 7,776 native cases. Fresh controls and identical input/uninterrupted/classical PCM were checked in every paired replay.

| Application | Failed guards | Admitted |
|---|---:|---|
| ratio_native_off | 40 | no |
| ratio_native_on | 37 | no |
| energy_native_off | 57 | no |
| energy_native_on | 67 | no |
| ratio_balanced_off | 45 | no |
| ratio_balanced_on | 43 | no |
| energy_balanced_off | 58 | no |
| energy_balanced_on | 58 | no |

The guard count includes aggregate cells, every cell×bitrate and the finer duration/mode ESTOI floor. A failed guard is a failed application; no score-based rescue.

Validation-selected candidate: **none**.

Error columns below show percent change versus classical PLC; negative is lower error. ESTOI is an absolute difference, higher is better. This is measured development-set behavior, not independent acceptance or a listening result.

| Application | Cell | Waveform error % | Envelope error % | MR-STFT error % | ESTOI delta |
|---|---|---:|---:|---:|---:|
| PR31 | en16k | -0.972 | -0.905 | -1.112 | +0.0004687 |
| PR31 | en48k | -0.955 | -0.472 | -2.757 | +0.0004690 |
| PR31 | sv16k | -1.013 | -1.040 | -0.904 | +0.0003697 |
| ratio_native_off | en16k | +2.852 | -3.177 | -1.120 | +0.0003954 |
| ratio_native_off | en48k | +2.860 | -2.969 | -1.912 | +0.0004070 |
| ratio_native_off | sv16k | +3.935 | -2.879 | -0.922 | +0.0004016 |
| ratio_native_on | en16k | +2.602 | -2.775 | -1.219 | +0.0004416 |
| ratio_native_on | en48k | +2.610 | -2.450 | -2.731 | +0.0004497 |
| ratio_native_on | sv16k | +3.119 | -2.592 | -1.001 | +0.0004411 |
| energy_native_off | en16k | -1.326 | -0.983 | -0.093 | -0.0005012 |
| energy_native_off | en48k | -1.218 | -0.649 | -0.960 | -0.0005595 |
| energy_native_off | sv16k | -1.554 | -1.231 | -0.174 | -0.0003284 |
| energy_native_on | en16k | -1.688 | -0.710 | -0.185 | -0.0004617 |
| energy_native_on | en48k | -1.582 | -0.225 | -1.656 | -0.0005218 |
| energy_native_on | sv16k | -2.297 | -1.029 | -0.239 | -0.0002839 |
| ratio_balanced_off | en16k | +4.123 | -3.049 | -0.931 | +0.0003322 |
| ratio_balanced_off | en48k | +4.147 | -2.896 | -1.880 | +0.0003479 |
| ratio_balanced_off | sv16k | +5.564 | -2.794 | -0.767 | +0.0003100 |
| ratio_balanced_on | en16k | +3.763 | -2.691 | -1.032 | +0.0003646 |
| ratio_balanced_on | en48k | +3.771 | -2.401 | -2.666 | +0.0003816 |
| ratio_balanced_on | sv16k | +4.887 | -2.516 | -0.842 | +0.0003333 |
| energy_balanced_off | en16k | -2.551 | -1.247 | -0.080 | -0.0003891 |
| energy_balanced_off | en48k | -2.421 | -0.882 | -0.862 | -0.0004376 |
| energy_balanced_off | sv16k | -3.534 | -0.642 | -0.054 | -0.0002746 |
| energy_balanced_on | en16k | -2.815 | -0.976 | -0.167 | -0.0003405 |
| energy_balanced_on | en48k | -2.699 | -0.458 | -1.553 | -0.0003929 |
| energy_balanced_on | sv16k | -4.188 | -0.442 | -0.116 | -0.0002317 |

Full per-bitrate guards, dual-reference results, phase-off/on differences, noise/pattern slices and2×2 contrasts are in the machine-readable results. The contrasts describe this fixed seed, update budget and teacher; they do not estimate variability across retrainings.

## Interpretation and delivery

No aggregate score rescued an unadmitted candidate. The eight applications failed 37–67 guard conditions each. In particular:

- Gain-ratio applications lowered envelope error but increased waveform error in every aggregate cell versus both classical PLC and PR31. `ratio_native_on` improved envelope error by 1.57–1.99% versus PR31, while increasing waveform error by 3.60–4.17%.
- Target-energy applications reduced waveform error but had negative ESTOI deltas in every aggregate cell versus both references. `energy_balanced_on` reduced waveform error by 1.76–3.21% versus PR31, but increased MR-STFT error by 0.80–1.24% and reduced ESTOI by 0.00060–0.00086.
- Bandwidth balancing helped the energy-head waveform result and partly reduced its ESTOI deficit, while worsening the gain-ratio waveform results. It was not a universal improvement under this fixed objective/budget.
- Phase-on improved waveform error and ESTOI relative to the corresponding phase-off application, while reducing the envelope improvement. It did not make any application admissible.

These are development-set measurements for one fixed seed and budget. They do not establish a fundamental limit on target-energy models, an audible preference, or an independent-test improvement. There was no extra training, seed/epoch search, replacement cohort, head swap or fallback candidate after selection.

The local training sum was 4.645 seconds. Most execution time was the full decoded CPU evaluation, approximately 34 minutes including checkpoint/resume. Colab consumed **0 CU**; verified balance before/after **1020.43 CU**, with no active session. The new raw-power datasets, masks, metadata and all four checkpoints/typed exports are archived in private Drive with a full SHA256 readback. Complete native sources/probes, raw metrics and receipts are archived separately; public results carry their checksums.

Safety/implementation evidence is distinct from quality admission: all eight applications passed 96 native release checks each, including zero allocations. Applied-gain parity retained the method's 5e-6 limit; separate analytic converter parity was 1.1921e-7 on 32 native fixtures. The installed format-only change passed 93 release checks, all-target Clippy and formatting. It preserved 504 paired PCM streams exactly, rejected an energy-tagged artifact in the gain loader at compile time, and produced identity in 84 metadata-disabled gain/energy streams. Packet/uninterrupted traces matched in 972 comparisons across 108 representative source/configuration inputs.

The initially attempted main build was rejected by the local mbx cache wrapper with “compiler input was modified during compilation”; the same compiler through the direct toolchain passed all 93 checks. This was not a recorded codec test failure. See private raw logs for both attempts.

The [machine-readable report](neural-plc-target-energy-study-results.json) includes every application, cell×bitrate guard, phase ablation, 2×2 contrast and noise/pattern slice. [Native checks](neural-plc-target-energy-native-checks.json), the immutable method, pretraining binding and the explicitly documented test-unit correction complete the audit trail. No independent ESTOI interval or new-model M5 non-regression is claimed because the validation admission failed.

Artifact sizes and full readback checksums are recorded in `neural-plc-target-energy-study-artifacts.json`. The storage index with private Drive file IDs remains in Drive, outside the public repository.
