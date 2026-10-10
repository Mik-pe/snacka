# Signed pulse classes, version 7

Pulse vectors previously budgeted a sign for every pulse and coded all unsigned
magnitude compositions uniformly. Version 7 bounds signs by the number of occupied
coefficients and, where profitable, codes that occupied count explicitly with a
small generated prior. Saved bits buy more pulses in the same fixed-size packet.
The [format](bitstream.md#deterministic-pulse-allocation) defines the exact counts,
CDF apportionment, deterministic representation selection and conservative bounds.

The gain grid, transform, model selection, bitrate quota and concealment are
unchanged. Both endpoints must use version 7; versions 0 through 6 are rejected.
There is no extra mode flag, predictive stream state or delay. Generated immutable
class tables occupy 43,560 bytes (42.54 KiB); per-band scratch uses at most eighty
usize entries. The default library still has no dependencies, C, FFI, unsafe Rust
or processing heap allocation. These are original implementations from mathematical
definitions, with no reused external implementation code.

## Speech evaluation

The frozen quality baseline is version 6 from main commit
`241dcd08350d75be37e11d797346e91a52707f15`. The subsequent
[encoder optimization](cpu.md#reusing-pulse-candidates-version-6) preserves its packet and PCM
output; CPU measurements below instead use that optimized implementation.
Quality is measured against frozen, separate native drivers
using Rust 1.99.0, optimization level 3, sixteen codegen units and overflow checks.
Both labels have identical input hashes, packet/drain budgets, aligned sample
counts and 120-sample transform delay. Their decoded PCM hashes are retained.

The algorithm was selected using eight development clips from four speakers.
The [previously evaluated selection](measurements/2026-10-10-quality-pulse-classes.csv)
contains 1,008 comparisons: 56 clips and 28 speakers, clean speech and deterministic
Gaussian noise at 20/10 dB, both durations and 12/24/32 kb/s.
The [new selection](measurements/2026-10-10-quality-pulse-classes-unseen.csv) contains
144 comparisons from eight clips and four new speakers. After freezing the
algorithm, select the first two FLAC members of the first four speakers in official
archive order absent from all 28 previous speakers: 1188, 2300, 6829 and 1580.
The [input manifest](measurements/2026-10-10-pulse-classes-unseen-input.sha256)
records their original hashes. No algorithm changes follow these scores.

All source audio is [LibriSpeech test-clean](https://www.openslr.org/12), CC BY 4.0.
Attribution: Vassil Panayotov, Daniel Povey, Guoguo Chen and Sanjeev Khudanpur,
*LibriSpeech: an ASR corpus based on public domain audio books*, ICASSP 2015.
Original files follow `LibriSpeech/test-clean/speaker/chapter/clip.flac` in
`https://openslr.trmal.net/resources/12/test-clean.tar.gz`. No audio or transcripts
are redistributed. Prior hashes are recorded in the
[48-clip manifest](measurements/2026-10-10-loss-seeded-input.sha256) and
[additional eight-clip manifest](measurements/2026-10-10-adaptive-gains-unseen-input.sha256).

The [shared quality method](quality.md#method), noise and paired metric-dither
seeds are unchanged: FFmpeg 9.0.2, Python 3.14.8, NumPy 2.5.3, SciPy 1.18.1 and
pystoi 0.4.1. STOI/ESTOI compare against clean speech; SNR and spectral distance
compare against encoder input. No noise samples clip. These are read-English
utterances with synthetic noise, not real microphones or multilingual calls.
Higher SNR/STOI/ESTOI and lower spectral distance are favorable. Means weight each
clip equally; they do not establish subjective quality or superiority over an
external codec. Raw measurements retain regressions alongside improvements.

| Selection | Input | Frame ms | SNR change dB | STOI change | ESTOI change | Spectral distance change dB |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Previous 56 clips | clean | 10 | +0.212254 | +0.015993 | +0.015715 | -0.512999 |
| Previous 56 clips | clean | 20 | +0.654934 | +0.015400 | +0.017968 | -0.528827 |
| Previous 56 clips | noise20 | 10 | +0.188690 | +0.015602 | +0.012356 | -0.384342 |
| Previous 56 clips | noise20 | 20 | +0.509696 | +0.014729 | +0.012762 | -0.506692 |
| Previous 56 clips | noise10 | 10 | +0.093920 | +0.012528 | +0.005838 | -0.386535 |
| Previous 56 clips | noise10 | 20 | +0.177113 | +0.014194 | +0.007272 | -0.736653 |
| New 8 clips | clean | 10 | +0.239400 | +0.015106 | +0.016382 | -0.498980 |
| New 8 clips | clean | 20 | +0.695512 | +0.016923 | +0.021372 | -0.501837 |
| New 8 clips | noise20 | 10 | +0.213298 | +0.014257 | +0.013272 | -0.352422 |
| New 8 clips | noise20 | 20 | +0.543368 | +0.014745 | +0.012418 | -0.440788 |
| New 8 clips | noise10 | 10 | +0.107017 | +0.012698 | +0.005191 | -0.360734 |
| New 8 clips | noise10 | 20 | +0.186260 | +0.012146 | +0.005272 | -0.553145 |

The table above covers 12 kb/s. At 24/32 kb/s, all duration/input-condition
mean changes are also favorable for all four metrics in both selections.
Across all 1,008 previous comparisons, two SNR, 34 STOI and 87 ESTOI cases
regress; worst changes are −0.184356 dB, −0.015996 and −0.042058 respectively.
No previous spectral-distance case regresses. Across the 144 new comparisons,
no SNR case regresses; six STOI, seventeen ESTOI and one spectral-distance case
regress, with worst changes −0.005668, −0.025195 and +0.023916 dB. All cases
remain in the raw tables.

For the 48 clean clips with a matched external reference at 12 kb/s/10 ms,
mean native STOI is 0.851978 versus 0.949883: a remaining deficit of
0.097905. For the 48 clean clips with a matched external reference at 12 kb/s/20 ms,
mean native STOI is 0.918326 versus 0.960611: a remaining deficit of
0.042285. These external numbers reuse the existing
[matched reference table](reference-quality.md); input hashes, byte budgets and
120-sample delay match each paired row. No new external-encoder run is claimed.
The quality gap remains substantial. Native improvement does not establish
competitive intelligibility, listening acceptance or a theoretical limit.

```sh
python tools/evaluate_quality.py corpus quality.csv \
  --roundtrip current-roundtrip --baseline baseline-roundtrip
```

## Loss and packet validation

The [seeded-loss table](measurements/2026-10-10-pulse-classes-loss-validation.csv)
contains 432 new-speaker cases: both durations, all three bitrates and input
conditions, independent 5% loss and 40/80 ms mean burst models. It compares
version 7 against its own uninterrupted output, using the same driver for both
labels. All packet/drain budgets match, decoded samples are finite and bounded,
and received audio after its 120-sample recovery prefix exactly matches the
uninterrupted output. Duplicate label outputs match. This establishes bounded
recovery, not improved concealment quality relative to version 6 or competitors.

```sh
python tools/evaluate_loss.py corpus loss.csv \
  --roundtrip current-roundtrip --baseline current-roundtrip \
  --patterns seeded --bitrates 12000 24000 32000 --require-identical
```

Independent pulse packets now compute their own class CDFs and budgets from
floating combinatorics, separately from the exact-integer table generator, and
write subsets/compositions without production pulse helpers. All bandwidths,
durations and byte budgets are decoded against defining inverse cosine sums.
Tests enumerate every K=1..80 and occupied count for seven dimensions, three
shape patterns and four consecutive bands, plus large fallback K values.
Arbitrary payloads produce bounded integer vectors with exact pulse sums;
truncated raw suffixes are rejected. Versions 0..6 are refused after nonzero
history for both durations, preserving output, the next packet and concealment.
The counting allocator observes zero processing allocations.

The final static-CDF/generator/test integration preserves the selected frozen
algorithm: [336 full packet/PCM traces](measurements/2026-10-10-pulse-classes-final-identity.csv)
match on eight development clips, three input conditions, both durations and
seven rates (12000/12345/12800/14400/16000/24000/32000). Runtime table values and
coding logic are unchanged after selection. All 47 ordinary debug/release tests
and thirteen optional monitor tests pass locally; hardware listening remains
unverified.

## CPU tradeoff

The [raw paired CPU table](measurements/2026-10-10-pulse-classes-cpu.csv) compares
version 7 against the optimized version 6 encoder from main
`c5b90cb27ddb8a25dc639073b46b8f47681e0d7f` (same core at `5262f84`).
On Apple M5, macOS 27.0 build 26A428, eight development clips, both durations,
three bitrates and encode/decode each receive three repetitions with alternating
order: 288 pairs. Each process encodes 10,000 or decodes 20,000 frames.
Both probes use identical cpu_probe.rs source and Rust 1.99.0 with optimization
level 3, sixteen codegen units and overflow checks; no LTO or native-CPU flags.
The [method](cpu.md#method) includes initialization and packet preparation.
No other codec evaluations or builds run concurrently with these timing pairs.

| kb/s | Frame ms | Operation | Baseline median microseconds | Current median microseconds | Median paired CPU change |
| ---: | ---: | --- | ---: | ---: | ---: |
| 12 | 10 | encode | 9.042 | 9.857 | +4.479% |
| 12 | 10 | decode | 3.719 | 3.696 | +0.334% |
| 12 | 20 | encode | 21.713 | 22.335 | +5.144% |
| 12 | 20 | decode | 8.348 | 8.813 | +4.995% |
| 24 | 10 | encode | 13.483 | 15.114 | +19.450% |
| 24 | 10 | decode | 5.086 | 6.724 | +24.765% |
| 24 | 20 | encode | 32.128 | 37.464 | +15.488% |
| 24 | 20 | decode | 11.969 | 14.639 | +20.818% |
| 32 | 10 | encode | 17.149 | 22.319 | +30.800% |
| 32 | 10 | decode | 5.972 | 9.321 | +50.689% |
| 32 | 20 | encode | 39.553 | 48.271 | +22.137% |
| 32 | 20 | decode | 13.674 | 18.115 | +31.845% |

Percentages are medians of paired ratios, not ratios of separately aggregated
medians. All repetitions, wall times and regressions are retained. More pulses
and class coding cost additional CPU, particularly at higher bitrate: the median
32 kb/s/10 ms decoder change is +50.689%, even though its measured process average
is 9.321 microseconds per frame. This release improves measured quality; it does
not claim a CPU improvement over version 6. Timing includes uncontrolled clock
and scheduling variation and is not callback-tail, mobile-power or battery proof.

Encode has 128 slower pairs out of 144, with changes
from -21.876% to +48.274%. Decode has 126 slower pairs out of 144, with changes
from -17.848% to +137.668%. Trace hashes change in every pair because version and audio coding change;
quality and frozen-to-final identity are evaluated separately above.

```sh
python tools/measure_cpu.py corpus cpu.csv \
  --current current-probe --baseline optimized-version-six-probe \
  --allow-trace-changes --bitrates 12000 24000 32000 --repetitions 3 \
  --encode-frames 10000 --decode-frames 20000
```

The [source manifest](measurements/2026-10-10-pulse-classes-source.sha256) records
runtime, tests, generator and evaluator sources. Physical mobile deadlines,
battery use, real conversation and blind listening remain unverified.
