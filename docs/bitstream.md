# Experimental native bitstream, version 2

The format is intentionally independent of legacy audio packet formats.
Incompatible changes require another version or defined profile. There are no
required neural weights in the current profiles.

## Packet framing

Packets are supplied as complete slices. There is no stream container or
transport framing in the library.

| Bytes | Meaning |
| --- | --- |
| 0–1 | ASCII `SN` |
| 2 | Bits 7–5: version 2. Bit 4: final drain. Bits 3–0: required profile |
| 3 | Frame sequence modulo 256, initially zero |
| 4 through length−3 | Entropy payload, padded to the allocated byte budget |
| Last two | Little-endian CRC-16/CCITT-FALSE over all preceding bytes |

CRC uses polynomial `0x1021`, initial state `0xffff`, no reflection, no final
XOR. It is not authentication. Profile 0 is 48 kHz mono / 20 ms, profile 1 is
48 kHz mono / 10 ms. Other profile identities are rejected. Any future profile
must specify all required decoding behavior, including necessary model weights
and their identity.

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

Gain is an eight-bit raw code. Code zero means all selected amplitudes are zero.
Codes 1–64 represent `2^(-16+code/8)`; codes 65–208 represent
`2^(-8+(code−64)/16)`. Higher codes are invalid. A one-bit raw quantizer flag
follows gain: zero selects four-bit amplitudes, one selects six-bit amplitudes.

| Amplitude width | Quantized integer q | Stored value | Invalid value | Reconstruction |
| --- | --- | --- | --- | --- |
| 4 bits | −7 through 7 | q+7 | 15 | q*abs(q)*gain/49 |
| 6 bits | −31 through 31 | q+31 | 63 | q*gain/31 |

The encoder compares both quantizers at the smallest gain covering the greatest
absolute coefficient and the adjacent lower nonzero gain. Peaks at or below
`2^-16` use code zero. Choose the candidate with least squared coefficient error,
including coefficients omitted by its budget. Quantize to the nearest
reconstructed level, saturating at the greatest magnitude. Four-bit squared
levels spend more precision near zero and retain more coefficients at a given
packet size; six-bit linear levels suit sparse spectra. Final PCM is clipped to
[−1,1]. Versions other than 2 are rejected; version 1 is not a compatibility path.

## Subset and entropy coding

Choose the greatest K absolute coefficients, with ascending index as the tie
break. K is the greatest integer satisfying
`ceil(log2(binomial(N,K))) + amplitude_width*K + 12 <= 8*payload_bytes`.
The budget includes eight gain bits, one quantizer bit and three bits for
interval rounding and termination. Both profiles use exact integer cardinality
tables for K; the quantizer flag determines which budget the decoder uses.

After writing gain and the quantizer flag, visit indices in order. If R selections
remain among A positions, code absence/presence with cumulative frequencies
`[0, A−R, A]`.
If R=A, presence is implicit. If R=0, stop. On each presence, write its four- or
six-bit amplitude and decrement R. The decoder reconstructs this same subset;
duplicate or out-of-order indices cannot be represented.

The entropy primitive follows the independently implemented 31-bit interval,
byte normalization and reverse raw-bit suffix mathematics of
[RFC 6716 sections 4.1 and 5.1](https://www.rfc-editor.org/rfc/rfc6716.html).
The arithmetic prefix zero-extends; raw reads remain physically bounded. Gain,
the quantizer flag and amplitudes use raw suffix bits, first bit least significant,
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
