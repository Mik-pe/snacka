# Adaptive pulse gain precision, version 6

Low-rate packets previously spent 32 bits on four pulse-band gains before coding
their shapes. Version 6 derives each gain's five-to-eight-bit precision from
frame size and payload length. At 12 kb/s this frees 12 shape bits per 10 ms
packet and eight per 20 ms packet. There is no extra precision flag. The
[format](bitstream.md#normalized-pulse-spectral-model) specifies the exact grid,
budget, invalid values and allocation. Version 5 packets are rejected; both
ends must use version 6. The scalar model, transform, delay, bitrate quota and
concealment algorithm are unchanged.

More shape resolution improves low-rate speech reconstruction on the measured
corpus, at some additional CPU cost. This is still an experimental codec with
a substantial gap to the matched external reference, not production or blind
listening acceptance.

## Speech evaluation

The native baseline is commit `91f52da61f8b6e196b258a7aede83b5c81883503`.
Both executables use Rust 1.99.0, optimization level 3, 16 codegen units and
overflow checks. The shared evaluator adds aligned decoded-PCM SHA-256 to each
row. Byte budgets, 120-sample delay and sample count match in every pair.

- [Previously evaluated clips](measurements/2026-10-10-quality-adaptive-gains.csv):
  864 pairs from 48 clips and 24 speakers. The original
  [input manifest](measurements/2026-10-10-loss-seeded-input.sha256) remains valid.
  Eight clips from four speakers were used to select the gain-precision rule;
  the other 40 clips were then evaluated without changing the algorithm.
- [New speakers](measurements/2026-10-10-quality-adaptive-gains-unseen.csv):
  144 pairs from eight clips and four further speakers, selected after freezing
  the algorithm. In official archive order, select the first two FLAC members
  of each of the first four speakers absent from the 24-speaker selection.
  Their IDs are 672, 5105, 3729 and 1089. The
  [new input manifest](measurements/2026-10-10-adaptive-gains-unseen-input.sha256)
  records original-file hashes. No algorithm changes follow these scores.

The corpus is [LibriSpeech test-clean](https://www.openslr.org/12), CC BY 4.0.
Attribution: Vassil Panayotov, Daniel Povey, Guoguo Chen and Sanjeev Khudanpur,
*LibriSpeech: an ASR corpus based on public domain audio books*, ICASSP 2015.
Original files follow `LibriSpeech/test-clean/speaker/chapter/clip.flac`, from
`https://openslr.trmal.net/resources/12/test-clean.tar.gz`. No audio or transcripts
are redistributed. These are read-English utterances with synthetic noise,
not multilingual conversations or real microphones.

The [shared method](quality.md#method) uses clean speech and the same seeded
Gaussian noise at 20/10 dB, 10/20 ms and 12/24/32 kb/s. No noise samples clip.
FFmpeg 9.0.2, Python 3.14.8, NumPy 2.5.3, SciPy 1.18.1 and pystoi 0.4.1 are
unchanged. Metric dither uses the [paired seed control](loss.md#alignment-and-validation).
STOI/ESTOI compare against clean speech; waveform SNR and spectral distance
compare against encoder input. No packet loss is used in these quality tables.

At 24/32 kb/s, all 672 full decoded PCM outputs across both tables are
byte-identical to the baseline. Packet bytes differ because of version and CRC.
At 12 kb/s, mean changes below weight each clip equally. Higher SNR, STOI and
ESTOI are favorable; lower spectral distance is favorable.

| Selection | Input | Frame ms | SNR change dB | STOI change | ESTOI change | Spectral distance change dB |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Previous 48 clips | Clean | 10 | +0.272979 | +0.022077 | +0.028211 | -1.297931 |
| Previous 48 clips | Clean | 20 | +0.133385 | +0.002718 | +0.004440 | -0.233073 |
| Previous 48 clips | Noise 20 dB | 10 | +0.244010 | +0.028174 | +0.024892 | -0.932730 |
| Previous 48 clips | Noise 20 dB | 20 | +0.113220 | +0.003060 | +0.004233 | -0.590242 |
| Previous 48 clips | Noise 10 dB | 10 | +0.122054 | +0.035668 | +0.019114 | -0.776554 |
| Previous 48 clips | Noise 10 dB | 20 | +0.050587 | +0.001187 | +0.002196 | -0.561212 |
| New 8 clips | Clean | 10 | +0.303802 | +0.034250 | +0.029877 | -1.635176 |
| New 8 clips | Clean | 20 | +0.170959 | +0.002609 | +0.004462 | -0.141422 |
| New 8 clips | Noise 20 dB | 10 | +0.283052 | +0.039278 | +0.029789 | -1.229167 |
| New 8 clips | Noise 20 dB | 20 | +0.155251 | +0.002733 | +0.003783 | -0.391254 |
| New 8 clips | Noise 10 dB | 10 | +0.175505 | +0.038278 | +0.018300 | -1.101059 |
| New 8 clips | Noise 10 dB | 20 | +0.075525 | +0.002413 | +0.001774 | -0.653492 |

Among the 288 prior low-rate cases, 53 STOI and 40 ESTOI comparisons regress;
worst changes are −0.018007 and −0.016030. Three spectral-distance comparisons
regress, with a worst increase of 0.023230 dB. None regress in waveform SNR.
Among the 48 new-speaker low-rate cases, eight STOI and eight ESTOI comparisons
regress; worst changes are −0.003218 and −0.010899. One spectral-distance case
regresses by 0.044229 dB; none regress in waveform SNR. Raw tables retain every
case and both decoded hashes, including regressions.

On the previous 48 clean clips at 12 kb/s, mean native STOI is now
0.835148/0.901865 for 10/20 ms. The existing
[matched reference](reference-quality.md) has means 0.949883/0.960611.
This comparison reuses that reference table: every input hash and all four
native-baseline metrics match its corresponding row exactly. It is a calculation
from the existing reference measurement, not another external-encoder run.
The remaining deficits are 0.114735/0.058746. Improvements against an earlier
native codec do not establish competitive intelligibility or audible quality.

Reproduce using separately built native drivers:

```sh
python tools/evaluate_quality.py corpus gains.csv \
  --roundtrip current-roundtrip --baseline baseline-roundtrip
```

## Loss and malformed-packet validation

The [new-speaker loss table](measurements/2026-10-10-adaptive-gains-loss-validation.csv)
contains 144 seeded-loss cases at 12 kb/s: both durations, three noise conditions
and independent 5% loss plus 40/80 ms mean burst models. This validates version 6
against its own uninterrupted output, with the same version 6 executable used
for both labels. All normal/drain byte budgets match; rejected propagation checks
and nonfinite/out-of-range output would abort evaluation. Every received region
after its 120-sample recovery prefix matches uninterrupted PCM exactly. Duplicate
label outputs are identical. These checks establish bounded recovery behavior,
not a packet-loss quality improvement over version 5 or another codec.

```sh
python tools/evaluate_loss.py corpus loss.csv \
  --roundtrip current-roundtrip --baseline current-roundtrip \
  --patterns seeded --quality --bitrates 12000
```

Independent packets cover all supported pulse-profile byte budgets and all six
bandwidth/duration combinations. Gain fields use the smallest nonzero grid value,
zeros and maximum gain; inverse PCM is compared with double-precision cosine
sums. Invalid gain values at every precision and byte budget are tested after
priming nonzero stream history. Refusals preserve output, subsequent concealment
and the next received frame. Older format versions are rejected before history
mutation. The counting allocator exercises both spectral models, every gain
width available at each duration, fractional bitrate updates, concealment, finish
and reset; it observes zero processing allocations. All 46 ordinary tests pass.

## CPU cost

The [raw CPU table](measurements/2026-10-10-adaptive-gains-cpu.csv) contains 480
paired runs on Apple M5, macOS build 26A428, using the identical source and
compiler settings for both probes. Eight development clips, both durations,
12/24/32 kb/s and encode/decode each get five repetitions with alternating order.
Each process encodes 10,000 or decodes 20,000 frames. The
[process CPU method](cpu.md#method) includes initialization and decoding packet
preparation; it is not a callback-tail, mobile or battery measurement.

| kb/s | Frame ms | Operation | Baseline median microseconds | Current median microseconds | Median paired CPU change |
| ---: | ---: | --- | ---: | ---: | ---: |
| 12 | 10 | encode | 7.837 | 8.442 | +7.718% |
| 12 | 10 | decode | 2.961 | 3.103 | +3.546% |
| 12 | 20 | encode | 19.029 | 19.667 | +3.299% |
| 12 | 20 | decode | 7.160 | 7.182 | +0.727% |
| 24 | 10 | encode | 12.334 | 12.359 | +0.307% |
| 24 | 10 | decode | 4.211 | 4.300 | +0.113% |
| 24 | 20 | encode | 29.356 | 29.257 | +1.147% |
| 24 | 20 | decode | 10.064 | 10.163 | +0.291% |
| 32 | 10 | encode | 15.430 | 15.647 | +0.942% |
| 32 | 10 | decode | 5.199 | 5.250 | +0.110% |
| 32 | 20 | encode | 36.317 | 36.965 | +1.535% |
| 32 | 20 | decode | 12.116 | 12.153 | -0.073% |

Each cell has 40 runs. Percentages are medians of paired ratios, not the ratio
of the two separately aggregated medians. More low-rate pulses cost additional
CPU; high-rate differences are small and affected by uncontrolled scheduling and
clock state. Wall time and all repetitions remain in the raw table. Packet trace
hashes differ at every rate due to the new version; aligned high-rate PCM
identity is established by the quality-table hashes instead.

The CPU tool now accepts `--bitrates`, retaining 24 kb/s as its default:

```sh
python tools/measure_cpu.py corpus cpu.csv \
  --current current-probe --baseline baseline-probe --allow-trace-changes \
  --bitrates 12000 24000 32000 --repetitions 5 \
  --encode-frames 10000 --decode-frames 20000
```

Core, tests, probes and evaluation sources are recorded in the
[source manifest](measurements/2026-10-10-adaptive-gains-source.sha256).
No runtime dependencies, C, FFI, CMake, trained weights or reused external
implementation source are added. Physical mobile deadlines, battery use,
real conversation and blind listening remain unverified.
