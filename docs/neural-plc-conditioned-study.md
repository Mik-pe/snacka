# Candidate-conditioned PLC distillation: retain PR31

The frozen six-fit study selected **no model**. All 18 applications failed at least one predeclared validation guard. PR31's learned gains and phase parameters remain installed. No independent acceptance waveform was opened, and no experimental model entered the M5 promotion gate.

A separate **bit-exact pitch-search optimization** is ready for production: first-loss medians improved by 11.2–16.3% across the measured 36 M5 strata. It preserves PR31's output, model footprint and packet format. See [search measurements](neural-plc-search.md).

## Question and frozen design

The [earlier 2×2 target-energy study](neural-plc-target-energy-study.md) retained PR31. This follow-up asks whether an identity-anchored residual adapter benefits from the **actual synthesized candidate's band energy**, and whether the oracle denominator should use that same causal phase rather than a future-informed PhaseWide phase.

The [protocol](neural-plc-conditioned-protocol.json) was frozen before fitting at commit a7210d1. The final source/data binding was sealed before the first fit; a parity-fixture filename correction (`1` versus `1.0`) was included before fitting. Protocol SHA-256 is `0917f6c0d7c29a850107eaec5a9906b899af30464c5118688a40ac96873b6525`; binding SHA-256 is `667b9fbf76dd84485e00de62c85183c2c781f0999d725de82e4edfd5b8435404`.

PR31's 80→64→64→17 network, gain head and phase head are frozen. A new adapter maps its native 64-value hidden activation plus 16 candidate-energy features to 16 log-gain residuals. History-only controls omit the candidate features. The candidate feature is `clamp(0.5 * (L_pred_actual_phase - L_last), -4, 4)`, using the same band means and epsilon (`1e-12`) as native decoding.

The adapter uses independent row-wise i8 scales for its history and candidate blocks. The residual is bounded to ±0.10 log-amplitude before the fixed application strength. Final gains retain `[0.7875, 1.2125]`. A zero, disabled or nonfinite residual returns the original PR31 gain exactly. Damping-only variants permit only nonpositive residuals. The experimental format tag is 3; these weights cannot be mistaken for installed tag 1 or target-energy tag 2.

A 64-byte raw log-power cache reuses the existing `History::observe` logarithms in experimental builds. Its cache/reset/silence and identity behavior were checked before fitting. Experimental weights total 12,936 logical bytes (1,472 extra); experimental decoder size is 16,352 bytes. These experimental caches and adapters are **not installed in the production decoder**.

| Fit | Inputs | Teacher denominator / restriction |
|---|---|---|
| history_wide | PR31 hidden state | Future-informed PhaseWide |
| history_aligned | PR31 hidden state | Actual frozen PR31 phase |
| conditioned_wide | Hidden state + actual candidate energy | PhaseWide |
| conditioned_aligned | Hidden state + actual candidate energy | Actual PR31 phase |
| history_damping | PR31 hidden state | Actual PR31 phase; damping only |
| conditioned_damping | Hidden state + actual candidate energy | Actual PR31 phase; damping only |

Every fit receives 12,000 updates, with 1,000 floating-point warmup updates followed by QAT. Equal English/Swedish batches contain 2,048 frames per language. The seed is 20261027; the fixed final checkpoint is used without seed/epoch selection. AdamW follows the frozen cosine schedule from 0.001 to 0.0001. The objective supervises the teacher's clamped log-gain **before the student's final clamp**, preserving a corrective gradient at a wrong bound, with a 0.1 residual-to-identity penalty. It does not regress unbounded raw target-energy extremes.

Each model is applied at strengths 0.25, 0.50 and 1.00. No new strength, objective or checkpoint was tried after validation. The teacher advances history with actual PR31 controls, never with its future-informed phase choices.

## Data and compute

There are 320 training and 24 known validation utterances. Training uses **36.03 minutes of natural source speech** from existing VCTK/RixVox selections, not newly acquired speakers. Native and 16 kHz-filtered English views, three bitrates, both frame durations, both modes and deterministic noise/gain cycles yield **557.60 augmented training minutes** (9.29 hours). Augmentations are not additional independent speech.

Export produced 583,867 eligible records, of which 540,420 are training records (231,473 EN and 308,947 SV). CPU preparation took 2,121.06 seconds elapsed, including 2,108.98 seconds exporting 6,576 source/profile jobs. Physical source bandwidth is recorded per item; 16 kHz records use 13 supported training bands, 48 kHz records use 16. Raw post-Phase1 target powers and both denominators are re-exported, rather than inferred from old clamped labels.

The 18 applications' decoded validation contains **155,520 rows**: 7,776 per application plus both references. The 36 views cover English 48 kHz, matched English 16 kHz and Swedish 16 kHz, at 12/24/32 kbps, 10/20 ms, both modes, three noise conditions and six loss patterns. This is the known development cohort. Its train/validation speaker groups are disjoint within this study; it does not establish that the frozen PR31/Phase1 weights have never seen these speakers.

Fitting ran on an A100-SXM4-80GB with PyTorch 2.11.0+cu130 and deterministic CUDA settings (TF32 disabled). Six fits took **307.38 seconds** in total for 72,000 updates. The CPU preparation and decoded scoring dominate elapsed work.

After fitting and native parity, scoring moved off the GPU to CPU hosts. A further execution-only amendment distributed complete paired cases across four 8-core CPU hosts. Scorer code, binaries, input selection and selection rules remained frozen. The merger verifies all 432 case keys and the complete row grid, source/PCM/binary/scorer hashes, then runs the original selector. The interrupted GPU prefix is diagnostic only. On 2,880 cross-host rows, input/decoded PCM, waveform and envelope metrics were exact; maximum floating-point differences were `5.56e-17` in MR-STFT and `3.34e-16` in STOI/ESTOI. See the [execution](neural-plc-conditioned-execution.json) and [sharding](neural-plc-conditioned-distributed.json) receipts.

All task Colab sessions are stopped. The final balance is **1,015.45 CU**, approximately **4.98 CU** below the starting balance. This is an account-balance delta, not an itemized invoice for individual components; [compute receipt](neural-plc-conditioned-compute.json).

## Locked admission outcome

Before ranking, each language/bandwidth cell and each cell×bitrate must improve lost-sample waveform error versus classical PLC, preserve envelope/MR-STFT and have nonnegative ESTOI. All four metrics must also be nonregressing versus PR31. Fine cell×bitrate×duration×mode ESTOI must stay within −0.001 of either reference, and the minimum aggregate-cell ESTOI improvement versus PR31 must be strictly positive.

| Fit | Failed guards at 0.25 | At 0.50 | At 1.00 |
|---|---:|---:|---:|
| history_wide | 21 | 21 | 22 |
| history_aligned | 21 | 22 | 26 |
| conditioned_wide | 9 | 10 | 10 |
| conditioned_aligned | 9 | 9 | 11 |
| history_damping | 19 | 22 | 34 |
| conditioned_damping | 19 | 22 | 32 |

The full [machine-readable decision](neural-plc-conditioned-results.json) includes every failed condition and all core slices. None is admitted; no ranking fallback is allowed.

For comparison, the following table fixes strength at **0.25 for every fit**. Changes are relative to PR31; lower error percentages and higher ESTOI are better. These are descriptive comparisons of rejected models, not a new selection rule.

| Fit | Cell | Waveform error | Envelope error | MR-STFT | ESTOI delta |
|---|---|---:|---:|---:|---:|
| history_wide | en16k | +0.1043% | -0.0636% | +0.0054% | +0.00000352 |
| history_wide | en48k | +0.1099% | -0.0957% | +0.0299% | +0.00000387 |
| history_wide | sv16k | +0.0949% | -0.0546% | +0.0032% | +0.00001295 |
| history_aligned | en16k | +0.1122% | -0.0722% | +0.0070% | +0.00000338 |
| history_aligned | en48k | +0.1186% | -0.1049% | +0.0385% | +0.00000381 |
| history_aligned | sv16k | +0.1174% | -0.0723% | +0.0042% | +0.00001471 |
| conditioned_wide | en16k | +0.0199% | -0.0764% | -0.0121% | +0.00000882 |
| conditioned_wide | en48k | +0.0230% | -0.1073% | -0.0249% | +0.00000727 |
| conditioned_wide | sv16k | +0.0123% | -0.0638% | -0.0130% | +0.00001651 |
| conditioned_aligned | en16k | +0.0173% | -0.0764% | -0.0160% | +0.00000856 |
| conditioned_aligned | en48k | +0.0212% | -0.1078% | -0.0215% | +0.00000688 |
| conditioned_aligned | sv16k | +0.0126% | -0.0698% | -0.0165% | +0.00001594 |
| history_damping | en16k | -0.8684% | +0.2690% | -0.0841% | +0.00000756 |
| history_damping | en48k | -0.8658% | +0.2850% | -0.1501% | +0.00000551 |
| history_damping | sv16k | -0.9203% | +0.2575% | -0.0700% | -0.00001719 |
| conditioned_damping | en16k | -0.8818% | +0.2577% | -0.0914% | +0.00000984 |
| conditioned_damping | en48k | -0.8811% | +0.2759% | -0.1635% | +0.00000725 |
| conditioned_damping | sv16k | -0.9350% | +0.2509% | -0.0755% | -0.00001597 |

Three observations guide further work:

1. Actual-candidate conditioning helps: at strength 0.25, the aligned history-only control fails 21 guards (12 waveform and 9 MR-STFT), while its conditioned counterpart fails 9, all waveform nonregression guards versus PR31. The latter improves aggregate envelope/MR-STFT/ESTOI in all three cells, but increases waveform error by 0.0126–0.0212%. The largest remaining cell×bitrate waveform regression is about 0.0642%. The frozen guard has no post-hoc tolerance for these small regressions.
2. Stronger conditioning is not an admitted solution. At strength 1.0 the aligned conditioned model improves aggregate envelope error by 0.266–0.422%, but increases waveform error by 0.0770–0.1115%; its aggregate ESTOI deltas remain small (`+0.0000199` to `+0.0000543`). There are no independent confidence intervals or listening claims for these rejected candidates.
3. Damping recovers waveform error at the cost of envelope preservation. At strength 0.25, conditioned damping reduces waveform error by about 0.88–0.94% versus PR31, but increases envelope error by 0.251–0.276% and makes Swedish ESTOI negative. Its failures cannot be repaired by choosing only a favorable language or metric. Changing the teacher denominator from wide to aligned alone does not remove the tradeoff.

The data supports candidate energy as useful causal information. It does **not** prove that more epochs, a GRU, a new pitch estimator or simply more compute would meet the joint constraints. A new model study would need a newly frozen objective/data/selection design, not another pass through this candidate pool.

## Native verification and production change

All 18 exported applications passed Rust/PyTorch **applied-gain** parity on 32 fixture records each; maximum absolute error was `1.1920929e-7` against the frozen `5e-5` limit. Independent preflight tests cover exact zero-adapter identity, damping bounds and corrective gradients. The native regression/zero-allocation checks passed; [native receipt](neural-plc-conditioned-native-checks.json).

The only production DSP change evaluates four consecutive pitch-search correlations with contiguous sample windows while preserving each scalar accumulation and candidate tie order. Primitive score-bit comparisons pass on 200 signal cases. Scalar and optimized PR31 builds produce identical PCM hashes for 216 streams across 36 configuration cases. All **374 release tests** with the pure-Rust Opus feature and all-target Clippy with warnings denied pass.

Installed decoder size remains 16,288 bytes, encoder size 12,128 bytes, learned constants 11,464 bytes, format tag 1. All learned weights, Phase1, `src/opus/`, pulse tables, dependencies and packet syntax remain unchanged. See [paired M5 timing](neural-plc-search.md); this speed result is separate from the rejected neural candidates.

## Next phase: measure the packet opportunity before training a prior

The accompanying [exact entropy-budget preflight](neural-entropy-budget-preflight.md) shows all 560 current occupied-count CDF bounds already attain their integer minimax optimum. A neural probability prior alone cannot lower those existing certified allocation entries. Lower average arithmetic usage could become padding, so no bitrate or quality win is established by fitting probabilities alone.

A separate shadow representation calculation extends the existing class path from K=80 through K=127 for D=20/40/80. All 141 proposed entries have lower certified costs (up to 35 bits, e.g. D=80/K=81), with approximately 14,758 additional table bytes and no larger position/magnitude scratch arrays. No production profile or CDF is changed. These conditional bounds are not measured packet savings.

The next bounded experiment should capture **accepted packet** class events, tell/padding and K=80 plateau/K>80 usage; compare a smoothed static control and shadow allocation before deciding whether a learned prior is useful. Any actual changed allocation/CDF must be specified in a separately negotiated native profile with deterministic encoder/decoder and loss/reset behavior. No GPU is required for this measurement. Independent acoustic acceptance remains reserved for an admitted candidate.

## Reproduction and artifacts

The private Drive index holds source audio, frozen fitting source, sharded labels, six checkpoints, exports, fixtures, Linux binaries, the full decoded CSV and proof logs. Every uploaded archive was read back fully and SHA-256 checked. Public hashes and sizes are in [artifact receipts](neural-plc-conditioned-artifacts.json); private file IDs/URLs are deliberately outside the repository.

Historical fitting uses the frozen source archive for commit 3f7ea52 and its binding. Later builder/sharding/search changes are execution and production tooling; they do not rewrite that source or binding. The builder now restores the explicit historical source commit rather than inheriting a future main.

```sh
# From the frozen fitting-source archive, with registered labels/binding:
python3 tools/train_plc_conditioned.py LABELS MODELS \
  --protocol docs/neural-plc-conditioned-protocol.json \
  --binding docs/neural-plc-conditioned-data-binding.json
# With complete, hash-matched decoded validation and its proof JSON:
python3 tools/select_plc_conditioned.py validation.csv validation-selection.json decision.json
# Pure CPU exact preflights (no training):
python3 tools/check_neural_entropy_budget.py --output entropy-budget.json
python3 tools/preflight_pulse_class_extension.py --output class-extension.json
```

Local release and Clippy checks use the direct Rust 1.99.1 toolchain to bypass the developer's Cargo wrapper, with a task-specific target directory. CI is not the final gate for this change, following the user's explicit instruction. No independent listening or Opus quality-superiority claim is made.
