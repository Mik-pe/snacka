# Causal envelope and phase PLC study

## Decision

**Adopt the selected 80→64→64→17 causal gain/phase model.** It passes all frozen independent quality conditions on 5,184 new cases, with positive speaker-bootstrap ESTOI intervals in both languages, unchanged packets/uninterrupted PCM, zero hot-path allocations and the M5 CPU budget. Its bounded gains and fractional phase offset replace the previous 16-output gain-only model; Phase1 and `src/opus/` remain unchanged. The preregistered validation rule selected the 80→64→64→17 phase student. Both gain-only and phase-on applications of the same fitted weights passed validation; both direct sequence-loss models failed. Only the selected phase student entered independent acceptance. No fallback, retuning or test-selected model was used.

The earlier representation oracle was a noncausal diagnostic. This study tests causal predictability with new speaker-grouped data and a separate unseen acceptance cohort. No GRU was trained. The complete numeric record is [neural-plc-causal-results.json](neural-plc-causal-results.json).

## Frozen design and data

Baseline main is `0908194` (PR30). The A training source was sealed at `40012f8`; combined candidate selection was frozen at `012cc67` before any causal training or decoded validation. B native/teacher implementation was sealed at `fb6d3e4`. The three original protocols remain unchanged: [A](neural-plc-causal-protocol.json), [phase controls](neural-plc-phase-protocol.json), [combined selection](neural-plc-causal-selection-protocol.json). Reader-format repairs accepted actual producer booleans and integer-valued float counts; they changed no raw measurements or thresholds.

Training uses 192 [VCTK0.92](https://doi.org/10.7488/ds/2645) utterances from 24 English speakers (625.995 s, native 48 kHz) and 128 [RixVox-v2](https://huggingface.co/datasets/KBLab/rixvox-v2) crops from 32 Swedish speakers (1536 s, native 16 kHz): **36.03 minutes total**. Validation has 12 utterances/six speakers per language (37.156 s EN, 144 s SV). One deterministic bitrate/duration/mode/noise/level augmentation is assigned per utterance. Swedish crops are the middle 12 s, selected from metadata before waveform evaluation. RixVox revision is `1f5f37f5ec8740eae318eeae7bf190074454d0d1`; canonical `speaker_id == riksdagen_id` separates 32/6/12 train/validation/test voices. Speaker separation is by published identities, not biometric matching.

The initial English test-clean choice was invalidated: 11 of 12 voices had already appeared in older Phase1 quality screens. The audit found 72 documented legacy English identities. **After training, before opening decoded validation scores or test audio**, we froze 12 [LibriSpeech test-other](https://www.openslr.org/12/) voices absent from that inventory with the same hash/word-count selection rule. See the explicit [holdout amendment](neural-plc-causal-holdout-amendment.json) and [legacy inventory](neural-plc-legacy-speaker-inventory.json). This changes English recording conditions; it is not an unchanged preregistration or a direct repeat of the old test-clean cohort. Amended metadata SHA256: `1629cad9629ac05b359b351dbeb9a65be077d7407d5f1dfa02408368fdc03b21`. Older Phase1/FLEURS Swedish speaker exposure cannot be certified.

Both acceptance languages are native 16 kHz. The study masks complete native bands 13–15 for Swedish training/envelope scoring and caps 16 kHz MR-STFT at 8 kHz. The 6.8–9 kHz crossing band is excluded from the supported-band envelope metric. English 48 kHz VCTK validates high bands, but the new independent cohort does not establish unseen full-band English or Swedish quality.

Every validation variant is freshly natively decoded at 24 kbps, 10/20 ms, both harmonic modes, three noise conditions and six loss patterns: 1,728 cases per variant. Five causal/installed runs plus seven representation variants produce 20,736 validation metric rows; repeated configurations are not independent speakers.

## A: full context and direct sequence losses

SPLS2 adds the complete last received frame before loss. Exact native inverse-MDCT bases and 120-sample overlap/add cover that prefix, 1/2/4/6 lost frames and the first recovery frame. The target remains uninterrupted decoded Phase1 audio. Native/PyTorch identity reconstruction was verified with finite QAT gradients. Envelope+MR-STFT uses weights 1/.1 and a small coherent low-band term .01; control uses coherent/envelope/MR-STFT weights 1/.1/.1. Gains stay [0.7875, 1.2125].

Each fit uses **60 fixed minibatch rounds, 32 balanced updates per round: 1,920 optimizer steps**, with ten float warmup rounds and i8 weight QAT thereafter. These are not 60 full-corpus passes. A draws eight sequences per language/update. B draws 64 eligible frames per language/update (122,880 draws/language, about 26.5 EN/9.3 SV eligible-frame equivalents). This bounded screen does not show data or optimization saturation. No best-epoch search was used.

All percentage changes below are errors relative to freshly decoded classical PLC; negative is better. Waveform error covers **lost samples**, not the whole clip and not a percentage improvement in audible quality. Envelope/MR-STFT reference uninterrupted decoded audio; whole-clip STOI/ESTOI reference the recorded source.

|Causal variant|Language|Lost waveform error|Envelope error|MR-STFT error|ESTOI delta|
|---|---|---:|---:|---:|---:|
|installed|en|-7.25%|+0.59%|-1.06%|+0.000244|
|installed|sv|-7.79%|+1.03%|-0.29%|-0.000161|
|control|en|-7.66%|+2.58%|-1.65%|+0.000317|
|control|sv|-7.73%|+2.51%|+0.32%|-0.000850|
|envelope|en|+3.45%|-3.36%|+0.57%|-0.000214|
|envelope|sv|+6.04%|-3.18%|+0.26%|-0.000153|

Control fails the envelope guard in both languages and ESTOI in Swedish. Envelope improves supported-band energy error about 3%, but increases lost-waveform error 3.45% EN/6.04% SV and has negative ESTOI. **Neither A model is admitted.** The installed model also misses the new envelope/Swedish ESTOI guards on this validation cohort; this does not itself disable the installed release.

## B: matched representations and causal distillation

The wide phase grid contains the old ±3-sample grid and additional ±4/5/6/7/8 controls. Phase is selected by active-band correlation below 4 kHz. In the noise ablation, the same unmixed phase selector runs first; only then is high-band noise mix selected by four-bin envelope error. Noise is deterministic, energy-normalized and orthogonalized; no seed search is allowed. Each variant retains its own actual recovery-history feedback, so later states need not match between variants. The finite future-informed grid is not a continuous optimum or a causal ceiling.

|Representation diagnostic|Language|Lost waveform error|Envelope error|MR-STFT error|ESTOI delta|
|---|---|---:|---:|---:|---:|
|gain_env|en|-2.55%|-9.95%|-4.87%|+0.003651|
|gain_env|sv|-0.74%|-10.14%|-4.00%|+0.004241|
|phase_narrow|en|-7.28%|-10.03%|-5.30%|+0.003841|
|phase_narrow|sv|-5.37%|-10.36%|-4.41%|+0.004522|
|phase_wide|en|-11.88%|-10.11%|-5.39%|+0.004187|
|phase_wide|sv|-11.05%|-10.49%|-4.63%|+0.004874|
|phase_noise|en|-11.53%|-10.11%|-1.18%|+0.004589|
|phase_noise|sv|-10.65%|-10.49%|-5.67%|+0.005330|
|trajectory|en|+0.00%|+0.00%|+0.00%|+0.000000|
|trajectory|sv|+0.00%|+0.00%|+0.00%|+0.000000|

The deterministic three-received-f0 trajectory produces **identical PCM** and zero offsets throughout this cohort. It therefore fails the strict lower-error guard. Main emits harmonic pitch only at bitrates <=16 kbps (`Encoder::encode`); at the 24 kbps validation rate, f0 is zero and the two harmonic flags encode identically. All 864 phase-student mode pairs have identical inputs and metric values. This is an inactive control result, not evidence that an active pitch trajectory cannot help. A separately frozen [12 kbps supplement](neural-plc-trajectory-activation-protocol.json) therefore compares the unchanged trajectory and the already selected student on the same known validation clips. It was added after validation selection and cannot rerank candidates, alter acceptance, introduce a test fallback or be described as part of the original preregistration. Independently estimated received-frame pitch remains a future alternative. The independent test includes 12 kbps for the selected model, but does not introduce another candidate or fallback. It also has gains 1 and an intraframe warp, whereas the student has 16 learned gains and one static frame offset. The phase-off/phase-on student pair is the matched learned-phase ablation.

Wide-grid boundary selections fall from 72.0% to 44.1% EN and 70.5% to 42.0% SV. The larger range exposes more noncausal waveform headroom. Boundary hits alone do not establish pitch-period drift or predictability. Matched noise increases ESTOI but worsens English MR-STFT versus phase-wide, so it is not uniformly better and no causal noise head is trained here. Full per-band correlation quartiles and control histograms are in the JSON record.

Teacher labels use PhaseWide targets, but the teacher always synthesizes **classical candidate 0** into its causal state. Every eligible 80-feature row matches the A export, and lost/recovery PCM matches bit-for-bit for all 344 utterances. The 17-output student uses 16 bounded gains and `8*tanh` samples. It distills log gains and a small Huber phase term, with identity/phase regularization; no target or future frame enters inference. The gain-only and phase-on modes share the same fitted i8 weights.

|Application of same fitted weights|Language|Lost waveform error|Envelope error|MR-STFT error|ESTOI delta [speaker 95% interval]|
|---|---|---:|---:|---:|---|
|distill_gain|en|-0.74%|-0.84%|-1.67%|+0.000398 [+0.000241, +0.000616]|
|distill_gain|sv|-0.22%|-1.31%|-0.57%|+0.000347 [+0.000276, +0.000419]|
|distill_phase|en|-1.13%|-0.32%|-2.53%|+0.000441 [+0.000280, +0.000659]|
|distill_phase|sv|-1.07%|-0.98%|-0.69%|+0.000369 [+0.000301, +0.000440]|

Both applications pass the per-language guard: waveform ratio<1, envelope ratio<=1, ESTOI delta>=0. The locked ranking maximizes the worse language’s ESTOI, then envelope/MR-STFT and smaller heads on exact ties; it selects **distill_phase**. It is not reranked after independent testing.

The matched phase increment is small: ESTOI +0.000042 EN (speaker 95% interval [+.000007, +.000083]) and +0.000022 SV ([-.000003, +.000045]). Six validation speakers per language limit inference, and the Swedish interval includes zero. Scalar phase prediction is also weak: validation RMSE 6.218/6.100 samples versus 6.209/6.063 for predicting zero, with mean absolute outputs 0.93/0.96 samples versus target 5.51/5.36. This does not prove that a GRU is needed, that phase is unlearnable, or that the current model has reached a ceiling.

## Nonselecting12kbps pitch activation supplement

This supplement was frozen after the initial validation revealed the bitrate cap, before opening independent test scores. It uses the same known 24 validation clips, no new training and no change to the candidate pool or acceptance rule. Both fixed implementations are reported completely (1,728 cases each).

|Known12kbps validation|Language|Lost waveform error|Envelope error|MR-STFT error|ESTOI delta|
|---|---|---:|---:|---:|---:|
|trajectory|en|-0.61%|-0.01%|-0.01%|+0.000019|
|trajectory|sv|-0.29%|-0.01%|-0.01%|+0.000026|
|distill_phase|en|-0.78%|-0.25%|-3.31%|+0.000430|
|distill_phase|sv|-1.21%|-0.81%|-1.32%|+0.000390|

Trajectory is exact identity with harmonic mode 0. In mode 1 it changes 66/432 English and 245/432 Swedish cases, reduces waveform error 1.23%/0.58% and raises ESTOI+.000038/+.000052. Thus the registered rule can help when pitch is available, but its average perceptual contribution is small. The selected MLP has larger ESTOI gains; it also applies 16 learned gains, so this is not a gain-matched phase-only comparison.

Native feature coverage confirms sparse three-received-frame pitch information even in the training augmentation:17/4635 eligible English rows and 186/13261 Swedish rows are three-frame voiced;16/170 have nonzero native trajectory offsets. Corresponding validation counts are10/251 and 10/870. This is input coverage, not a causal capacity ceiling.

## Independent acceptance and native gates

The single selected model was evaluated on all 24 frozen new clips, 12/24/32 kbps, 10/20 ms, both harmonic modes, three conditions and six patterns:5,184 paired cases. Mode partitions use the exact same production candidate and frozen-main probe executables. Complete finite coverage, per-language waveform/envelope/ESTOI guards and eight language/mode/duration ESTOI floors(-.001) are required. Packet traces and uninterrupted PCM are checked against frozen main; classical loss replay explicitly disables neural PLC via the public switch while keeping Phase1 enabled. No test-based fallback is permitted.

|Independent cohort|Lost waveform error|Envelope error|MR-STFT error|STOI delta|ESTOI delta [speaker95% interval]|
|---|---:|---:|---:|---:|---|
|en, 12 new speakers|-1.09%|-0.85%|-1.02%|+0.000394|+0.000285 [+0.000196, +0.000389]|
|sv, 12 new speakers|-0.58%|-1.08%|-0.92%|+0.000484|+0.000394 [+0.000331, +0.000459]|

All eight language/mode/duration ESTOI strata are positive (+.000231 to+.000425), above the frozen−.001 floor. Complete finite coverage, exact executable/probe/cohort digests and packet/uninterrupted/classical replay checks pass. The acceptance reference is **classical PLC**. Comparison with the previous installed model is validation-only: this study does not claim a new independent paired win over that model or passage of its earlier0.25dB waveform threshold. This is a small objective-metric gain; no blinded listening was performed. The chosen objective trades some of the old model’s coherent MSE reduction for lower envelope error and positive intelligibility proxies.

The proposed production online conceal body is copied from the validated student. Before acceptance it reproduces all validation PCM bit-for-bit (288 native configuration replays, 4,032 paired streams). The staged candidate passes 91 Rust release tests including allocation tests. The delivered source passes 93 after adding two phase read/wrap/fade contract tests; the selected weight file and all three runtime function bodies are identical to the accepted candidate. Documentation, test-only helper scoping and a generated-metadata lint attribute change no inference computation. Trained PyTorch/native control parity is below 6e-7, and M5/Linux allocation tests pass. No external inference dependency, packet bit, recurrent state or Decoder field is added; `src/opus/` is untouched.

The shared export header’s `strength=1` is serialization metadata, not the legacy 16-output gain scale. The delivered head explicitly applies gain radius 0.2125 and phase radius 8 samples; no extra post-hoc scaling was selected.

The 17-output model has 11,464 logical bytes of i8 weights/scales/biases, 72 bytes above the 16-output model; this is not a linked-binary size claim. M5 inference median is 1.586µs. Production-body first-loss medians are 16.75/16.90µs(10 ms, modes 0/1) and 22.04/22.04µs(20 ms), over 7,020 first-loss calls and 286,120 total timed calls. A separate candidate timing run also passes 18/23.4µs. Scheduler tails exceed 25µs; the gate concerns medians, not worst-case deadlines. The retained older English clips are used only for CPU timing, not fresh quality acceptance.

## Artifacts, resources and limits

All owned sessions are stopped: active assignments 0, rate 0 CU/hour. Account balance moved from 1021.06 to 1020.43 CU: **0.63 CU** across this study, measured after shutdown. These are rounded account snapshots, not a per-job invoice. A control/envelope GPU fits take 46.91/47.02 s on L4; the GPU lifecycle including staging/training is 118.86 s and was stopped immediately after downloading checked weights. B student training takes 11.66 s on CPU. CPU extraction/native replay/scoring dominate the elapsed study.

The 344-clip licensed raw corpus, full SPLS2 exports, checkpoints, exact generated Rust sources/executables, protocols and complete raw metric/statistic matrices are retained privately on Drive. Bulk archives pass full SHA256 readback; split archives also pass ordered reassembled hashes. Upload transport briefly uses a bounded local file, removed after verification; the new corpus is not retained offline on the Mac. Unrelated pre-existing Phase1 data remains untouched. Private credentials, signed URL caches and bootstrap scripts are excluded.

Verified raw corpus archive: 69294080 bytes, SHA256 `7e571abc7af1411dacc3846b2d844854b17c3c6336f926e54e63755acd42e2de`. SPLS2 archive: 239482880 bytes, SHA256 `7fb3394c1cba5364dea97d6449a25b90425d47a4ab4748097c490f8ac0b2be2b`. A evidence archive: 8715899 bytes, SHA256 `04acbbd42d03683fc848809d6f639bee0abe8f35ef80eb52c84d321c4485897a`. B evidence archive: 249620286 bytes, SHA256 `409804eaa2ed0b315c8e311fae847971eeb3691549a290756aefb751ee0541f2`. Private Drive IDs are in the private manifest, not this public report. Earlier test-clean locators inside the initial data packages are explicitly superseded by the amendment archived alongside A evidence.

No blinded listening or subjective quality claim is established. These are native-PLC comparisons, not an Opus quality comparison. The causal input contains three received frames and PLC state, but not current predicted band energies. Oracle gain targets depend on those energies, so adding causally available prediction features or predicting target energy is a concrete future ablation; these results do not establish it as the cause of the remaining gap. More epochs, a GRU or a noise head should be separate controlled experiments rather than assumed remedies.

Independent/supplement evidence:24271707 bytes, SHA256 `df34bdeff3e4eef49b81c408ca0aaaa8553c97034941cfd9c90f3ddf86085def`; M5 evidence:4832354 bytes, SHA256 `8ca1abce0dbfefa2c24291f397852e33dc83cfcec625dd89e944c605c2c90134`. Source snapshot `83134d6`:10695992 bytes, SHA256 `d37fc94686527fcb0392a8c83d5b336e2ab1bcdd89a3d23ac2a6928a560a2fdc`. Use the frozen A/B source payloads for historical fits; later main has a different head. The source snapshot plus checkpoint, student receipt and `build_plc_phase_candidate.py` reproduce the accepted 17-output deployment. Runtime receipts record Rust 1.99.1, Python 3.13.16, FFmpeg 6.1.1 and the NumPy/SciPy/PyTorch versions. Local Rust and data/model gates were completed; CI was not used as the adoption gate, as requested.

## Follow-up questions for the next controlled study

Could the 16-gain head predict target log energy and use the causally available current PLC band energy to form gains, rather than infer that denominator from received history? Can received-frame pitch features be supplied at 24/32kbps without exceeding the CPU budget? How much does matched source-rate augmentation help, given EN48kHz/SV16kHz training and 16kHz independent acceptance? These are hypotheses for a new protocol and fresh holdout, not reasons to retune on this consumed test cohort. No result here requires a GRU.
