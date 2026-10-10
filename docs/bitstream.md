# Experimental native bitstream, version 7

The format is intentionally independent of legacy audio packet formats.
Incompatible changes require another version or defined profile. There are no
required neural weights in the current profiles.

## Packet framing

Packets are supplied as complete slices. There is no stream container or
transport framing in the library. The transport negotiates the codec identity;
packets are not scanned for a magic marker. Version, sequence and checksum occupy
four fixed bytes. All remaining bytes are available to spectral coding.

| Bytes | Meaning |
| --- | --- |
| 0 | Bits 7–5: version 7. Bit 4: final drain. Bits 3–0: required profile |
| 1 | Frame sequence modulo 256, initially zero |
| 2 through length−3 | Entropy payload, padded to the allocated byte budget |
| Last two | Little-endian CRC-16/CCITT-FALSE over all preceding bytes |

CRC uses polynomial `0x1021`, initial state `0xffff`, no reflection, no final
XOR. It is not authentication. All profiles produce 48 kHz mono PCM. The profile
combines frame duration and coded bandwidth:

| Scalar profile | Pulse profile | Duration | Coded bandwidth | Transform size N | Coded coefficients M |
| --- | --- | --- | --- | --- | --- |
| 0 | 8 | 20 ms | 24 kHz | 960 | 960 |
| 1 | 9 | 10 ms | 24 kHz | 480 | 480 |
| 2 | 10 | 20 ms | 12 kHz | 960 | 480 |
| 3 | 11 | 10 ms | 12 kHz | 480 | 240 |
| 4 | 12 | 20 ms | 8 kHz | 960 | 320 |
| 5 | 13 | 10 ms | 8 kHz | 480 | 160 |

Bit 0 identifies duration. Bits 2–1 identify bandwidth, with 0/1/2 for
24/12/8 kHz. Bit 3 selects one shared scalar gain (zero) or four normalized
pulse bands (one). Bandwidth and spectral model may change per packet; duration
is negotiated before constructing the decoder. Profiles 6, 7, 14 and 15 are
rejected. Any future profile must specify all required decoding behavior,
including necessary model weights and their identity.

20 ms packets occupy 30–80 bytes; 10 ms packets occupy 15–40 bytes. These sizes
include framing and checksum. For target bitrate B, accumulate B into a byte
quota and emit floor(quota/D) bytes, retaining quota modulo D. D is 400 for
20 ms, 800 for 10 ms. Changing bitrate retains the remainder. The decoder
infers its spectral budget from packet length, independently of target bitrate.

## Transform and gain

For N=960 or 480 and overlap O=120, transform a 2N block with a symmetric window.
Let p=(N−O)/2. Window entries before p and after 2N−p are zero; the middle is
one except an O-sample rise and fall. The rise is
`sin(pi/2 * sin(pi/2 * (i+0.5)/O)^2)` for i=0 through O−1; the fall is its reverse.
The active input consists of the previous O samples followed by the current N
samples, beginning at p. All other entries are zero.

Coefficient k is `2/N * sum_i(input[i]*window[i]*cos(pi/N*(i+0.5+N/2)*(k+0.5)))`.
The inverse uses the same cosine without the `2/N` scale, then the window.
Output is inverse[p..p+N] plus the preceding inverse block's O-sample tail on
its first O samples. Save inverse[p+N..p+N+O] for the next output.

Both spectral models use the same decoded gain mapping. Scalar gains use eight
raw bits; pulse gain precision is specified below. Code zero means all selected
amplitudes are zero.
Codes 1–64 represent `2^(-16+code/8)`; codes 65–208 represent
`2^(-8+(code−64)/16)`. Higher codes are invalid. Final PCM is clipped to [−1,1]. Versions other than
7 are rejected; there is no older-format compatibility path.

## Scalar spectral model

A one-bit raw quantizer flag
follows gain: zero selects four-bit amplitudes, one selects six-bit amplitudes.

| Amplitude width | Quantized integer q | Stored value | Invalid value | Reconstruction |
| --- | --- | --- | --- | --- |
| 4 bits | −7 through 7 | q+7 | 15 | q*abs(q)*gain/49 |
| 6 bits | −31 through 31 | q+31 | 63 | q*gain/31 |

Within each coded spectral domain, the encoder compares both quantizers at the smallest
gain covering the greatest absolute coefficient and the adjacent lower nonzero gain. Peaks at or below
`2^-16` use code zero. Choose the candidate with least squared coefficient error,
including coefficients omitted by its budget. Quantize to the nearest
reconstructed level, saturating at the greatest magnitude. Four-bit squared
levels spend more precision near zero and retain more coefficients at a given
packet size; six-bit linear levels suit sparse spectra.

### Scalar subset coding

Choose the greatest K absolute coefficients within M positions, with ascending index as the tie
break. K is the greatest integer satisfying
`ceil(log2(binomial(M,K))) + amplitude_width*K + 12 <= 8*payload_bytes`.
The budget includes eight gain bits, one quantizer bit and three bits for
interval rounding and termination. Profiles use exact integer cardinality
tables for K for M=160/240/320/480/960; the profile and quantizer flag determine
which budget the decoder uses. Coefficients M through N−1 are zero.

After writing gain and the quantizer flag, visit indices in order. If R selections
remain among A positions, code absence/presence with cumulative frequencies
`[0, A−R, A]`.
The first visit starts with A=M and R=K.
If R=A, presence is implicit. If R=0, stop. On each presence, write its four- or
six-bit amplitude and decrement R. The decoder reconstructs this same subset;
duplicate or out-of-order indices cannot be represented.

## Normalized pulse spectral model

Let P be payload bytes excluding framing, and N be transform size. Gain width
is `G = clamp(floor(240*P/N), 5, 8)` bits. Write four G-bit raw values in
increasing band order, with no scalar quantizer flag. The gain step is
`S = 2^(8-G)`; a raw value r reconstructs decoded gain code `r*S`. Reject codes
above 208 before consuming stream history. At 12 kb/s, G is five bits for 10 ms
and six for 20 ms. At 24/32 kb/s both durations use eight bits. Fractional byte
budgets can cross a precision boundary; the packet length fully determines G.
There is no additional precision flag or dependence on preceding packets.

Band edges in coefficient positions are
`[0, N/24, N/12, N/6, M]`: approximately 0–1, 1–2, 2–4 kHz and the remainder
of the coded spectrum. Let D be a band's number of coefficients. The gain is
the L2 norm of its coefficient vector. The encoder caps the measured norm at
2.0, rounds the covering gain code upward to a multiple of S and compares it
with the adjacent lower nonzero multiple. Choose the closer gain; ties choose
the lower code. Peaks at or below `2^-16` use zero as in the scalar model.

Each band has a signed integer vector q with `sum(abs(q))=K`. Its decoded
coefficients are `gain * q / sqrt(sum(q^2))`. A zero gain receives zero pulses
and reconstructs exact zeros. Coefficients M through N−1 are zero.
The mathematical separation of energy and unit-length shape is described in
[RFC 6716 sections 4.3.2 and 5.3.8](https://www.rfc-editor.org/rfc/rfc6716.html).
The band boundaries, allocation and signed-multiset representation below define
Snacka's independent packet format.

### Deterministic pulse allocation

For K pulses in D coefficients, the multiset representation has cost
`U(D,K)=ceil(log2(binomial(D+K−1,K)))+min(D,K)`, with U(D,0)=0.
Only occupied coefficients need signs, so there can never be more than min(D,K).
For K≤80, a second representation first codes the occupied coefficient count J.
Its exact signed class cardinality is
`V(D,K,J)=binomial(D,J)*binomial(K−1,J−1)*2^J`, for 1≤J≤min(D,K).

Let T=sum(V), L=min(D,K), and Q=65535. Allocate each class a positive integer
CDF width `w_J=1+floor(V*(Q−L)/T)`. Distribute the remaining units by descending
remainder `V*(Q−L) mod T`, breaking ties toward the smaller J.
The certified class cost is
`H(D,K)=1+max_J(ceil(log2(V*Q/w_J)))`.
Compute logarithm ceilings with exact integers, including the rational quotient.
The additional bit covers finite arithmetic interval rounding in this representation.
Choose classes only when H<U; otherwise use the multiset representation.
For K>80 always use multisets. Selection depends only on D and K.

The allocator uses the monotone upper envelope of these chosen costs:
`C(D,K)=max_{0≤k≤K}(min(U(D,k),H(D,k)))`, omitting H where classes are unavailable.
K is capped at 387. Exact tables cover dimensions 20/40/80/160/320/400/800.
Four gains and four bits for arithmetic rounding/termination leave the constraint
`4*G + 4 + sum(C(D,K)) <= 8*payload_bytes`.
There is no side flag, temporal prediction or extra frame delay.

Allocation uses integer arithmetic in both encoder and decoder. Start with
one pulse per nonzero gain, zero otherwise. For every eligible band, compute
the additional bit cost `C(D,K+1)−C(D,K)`. Among candidates that fit the remaining
budget and have K<387, add a pulse to the greatest priority
`W(code)*D / ((K+ceil(sqrt(D)))^3 * additional_bits)`.
Compare priorities by exact u128 cross products. A zero-cost increment outranks
all positive-cost increments; ties, including two zero-cost increments, choose
the earlier band.
Repeat until no additional pulse fits. The bounded dimensions, gain codes and
K cap keep these products within u128.

W is squared gain in units of 2^−32, rounded to the nearest integer: zero for
code zero, `round(2^(code/4))` for codes 1–64, and
`round(2^(16+(code−64)/8))` for codes 65–208. The published constants in
[src/pulse.rs](../src/pulse.rs) are normative. The independent standard-library
script checks them against the exact combinatorial definition and the gain
formula; it is not used at build time:

```sh
python3 tools/generate_pulse_tables.py --check
```

### Finite interval and termination bound

Before each arithmetic symbol the normalized range R is greater than 2^23.
For total frequency a, integer division retains at least the ideal interval
fraction times `(1−a/2^23)`. The zero-lower interval receives the division
remainder and fares better. A class band has one prior with a≤65535, at most
D position symbols with a≤D, and at most K−1 composition symbols with a≤K−1.
For D≤800 and K≤80 the sum of `−log2(1−a/2^23)` is less than 0.123 bits.
The additional bit in H covers this entire class-band excess.

Fallback multiset bands have at most D+K−1 symbols with a≤D+K−1.
At K≤387, summing the conservative excess for the largest four 20 ms band
dimensions 40/40/80/800 is less than 0.342 bits; the 10 ms bound is smaller.
With m normalized byte shifts, arithmetic information is
`31+8*m−log2(R)`; integer tell is `32+8*m−floor(log2(R))` plus raw bits,
less than information plus two. Termination requires at most ceil(tell/8)
bytes, sharing leftover arithmetic-byte bits with the raw suffix. The joint
four-bit allocation reserve therefore covers fallback rounding, initial state
and termination after the class costs have covered their own rounding.

### Signed class coding

When the derived class mask selects this representation, count the J occupied
coefficients and code J−1 with the generated Q=65535 CDF. Code their strictly
increasing positions as a J-subset of D using presence/absence `[0,A−R,A]`.
On each presence write one raw sign bit (zero positive, one negative).
All occupied magnitudes start at one.

The remaining K−J magnitude units form a multiset over J ordered occupied
coefficients. Represent it as a subset of K−1 slots: visit with R=K−J,
A=K−1 and the same binary CDF, decrementing both on each visit and R only on
presence. Each absence advances to the next occupied coefficient; each presence
increments its signed magnitude. If R=A presence is implicit; stop when R=0.
The decoded vector has exactly K total absolute magnitude and J nonzero bins.
Normalize by the square root of its integer squared norm as for multisets.

### Signed multiset coding

Expand each vector magnitude into K sorted, possibly repeated frequency
positions `p_j`. Map them to the strictly increasing subset `p_j+j` in
D+K−1 positions. Visit this subset in order using the same presence/absence
CDF `[0,A−R,A]` as scalar subsets, starting with A=D+K−1 and R=K.
If R=A, presence is implicit; if R=0, stop. On each presence, its frequency is
`position−selected_pulses`. The first pulse at a frequency carries one raw sign
bit (zero positive, one negative); subsequent pulses reuse that sign. Increment
the signed magnitude and selected pulse count. Visit bands in increasing order.
The decoder normalizes each completed vector using its integer squared norm.
This representation cannot encode contradictory signs at one frequency.

The encoder seeds pulse magnitudes with
`floor(K*abs(x)/sum(abs(x)))`, then greedily assigns remaining pulses to maximize
squared correlation divided by squared vector norm. Ties choose the lower
frequency index. Only the greatest min(K,D) absolute input coefficients need
be candidates; an unused larger coefficient always outranks an unused smaller
one. Seeding saturates at the remaining pulse count to tolerate floating-point
rounding. For K>1, perform at most two pulse exchanges per band. Choose an
occupied coefficient whose removal leaves the greatest normalized correlation,
then choose another coefficient to receive that pulse. Commit only if the
completed exchange strictly improves squared correlation divided by squared
norm; equal improving candidates prefer the lower frequency index. Correlation
comparisons use double precision and integer squared norms. Exchanges preserve
the sum of absolute pulse magnitudes and input-aligned signs. No exchange search
is needed for K=0 or K=1. This remains a bounded heuristic, not a globally optimal
vector search, and does not change decoder rules or packet capacity.

## Encoder model selection

Evaluate scalar candidates at all three bandwidths, lower gain before covering
gain, four-bit before six-bit amplitudes. Then evaluate pulse candidates at
24/12/8 kHz. Compare full-spectrum squared-error reduction
`sum(x^2−(x−reconstruction)^2)` relative to an all-zero spectrum. This counts
omitted coefficients inside and outside the coded domain. Choose the greatest
reduction; ties retain the earlier candidate. Pulse allocation is saved with
the winning candidate. Profile identity consumes no payload bits.

## Entropy primitive

The entropy primitive follows the independently implemented 31-bit interval,
byte normalization and reverse raw-bit suffix mathematics of
[RFC 6716 sections 4.1 and 5.1](https://www.rfc-editor.org/rfc/rfc6716.html).
The arithmetic prefix zero-extends; raw reads remain physically bounded. Gain codes,
the scalar quantizer flag/amplitudes and pulse signs use raw suffix bits, first bit least significant,
from the end backwards. Termination pads the unused gap with zero. The decoder additionally
rejects logical bit usage beyond the payload budget and invalid spectral values.
This shared primitive does not make native audio packets Opus-compatible.

## Stream lifetime

Each track starts at sequence zero. Ordinary output has N valid samples, with
O initial priming samples discarded once. Encode an all-zero input frame with
the final-drain flag to recover the last O real samples. That output has only O
valid samples. Reset after finish. Packet loss is handled by an explicit
concealment call advancing one sequence and synthesizing one N-sample frame.
Concealment is decoder behavior, independent of the packet format. The current
decoder retains 40 ms of decoded PCM. On the first loss it estimates a period
from normalized autocorrelation at 8 kHz, refines it at 48 kHz and interpolates
the last cycle. Low-confidence or short history repeats the last 10 ms instead.
The continuation fades linearly over 80 ms for periodic signals and 40 ms
otherwise. Transform analysis/synthesis of the continuation preserves the same
short overlap, including a predicted 120-sample tail. No future packet is needed;
prediction does not add buffering delay. Silence is exact after the fade and
overlap drain. A received packet replaces the transform tail; any loss-induced
reconstruction error is confined to its first 120 samples. Waveform history is
discarded across the missing interval before collecting newly received PCM.

Frame buffering is 20 or 10 ms. Additional transform delay is 2.5 ms. Thus the
codec accumulation plus transform delay is 22.5 or 12.5 ms, excluding processing,
transport, device buffering and scheduling. A complete packet must be received
before its audio can be decoded.
