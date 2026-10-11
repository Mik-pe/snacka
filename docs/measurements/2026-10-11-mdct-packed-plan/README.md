# Packed MDCT measurements

This report records one fixed trial on an Apple M5 running macOS. Packing the
existing MDCT window and rotation coefficients into contiguous rows preserved
the tested packet, PCM and transform output bits. Median paired decoder process
CPU count differences decreased by 5.53–7.96% across harmonic/default modes and
10/20 ms frames. Harmonic encoding was approximately unchanged: its measured
medians were −0.49% and −0.58%.

| Phase | Frame | Median process CPU change | CPU regressions / 32 | Wall regressions / 32 |
| --- | --- | ---: | ---: | ---: |
| Decode, harmonic | 10 ms | −6.0126% | 7 | 8 |
| Decode, harmonic | 20 ms | −5.5278% | 7 | 7 |
| Decode, default | 10 ms | −7.9626% | 5 | 8 |
| Decode, default | 20 ms | −6.6611% | 2 | 6 |
| Encode, harmonic | 10 ms | −0.48785% | 12 | 11 |
| Encode, harmonic | 20 ms | −0.57715% | 10 | 11 |
| Encode, default | 10 ms | −2.0554% | 8 | 8 |
| Encode, default | 20 ms | −1.88945% | 10 | 10 |

All 256 paired results are retained, including 61 CPU and 69 wall regressions.
The frozen workload and completed helpers support 768 timed calls. Internal
per-call PID/stdout journals and raw encoder process totals are unavailable;
encoder results retain rounded paired ns/frame and percentage values. The largest CPU regression was +55.29%; the largest wall ratio was
+177.90%. Shared system/application load affected this trial. Two harmonic
pairs had a nonpositive wall count difference; those signed values and their
ratios remain in the tables. Wall results provide no latency guarantee.

The unchanged canonical decoder helper runs 262,144 and 524,288 frames, cycling
the first 256 prepared encoded frames. Difference-of-count process CPU includes
whole-process startup, constructors and packet preparation; approximate setup
cancellation is an assumption. Encoding runs 32,768 frames cycling the first 256
prepared PCM frames, with startup and construction included. These measurements
do not establish phone performance, battery use, real-time deadlines or tails.
Each mode uses eight already exposed development clips from four speaker-ID
prefixes, two durations and four repetitions, with balanced AB/BA order.

The prospective gate required decoder median 32 at each duration to be <= −5%,
using Decimal precision 80 on the lexical raw process-CPU count differences; every
CPU difference had to be finite and positive. Encoder median 32 had to be <= +2%,
using the canonical four-decimal percentage. All four phases passed. The trial
had no retries, filtered rows or gate changes. Independent CPU data review and final closure are approved; source/resource, API
and audio-control reviews are also recorded. Independent privacy/provenance review is also approved; its hash is bound in
`publication-provenance.json`.

Correctness evidence includes all 29,448 ordered API records: constructor/status
contracts, exact error-buffer sentinels, finite outputs, transform bits, TDAC,
codec reset, loss and drain behavior. The generated 363,602,645-byte API fixture
was byte-identical to the accepted baseline; its audio/binary payload is excluded
here. All 1,472 canonical trace configurations also matched complete packets and
aligned PCM. The additional 96 high-rate rows join those same trace observations;
they are not independent executions. No quality metrics were rerun.

The plan keeps 16N bytes of numerical payload, now in one row vector. Construction
can temporarily require 24N instead of 16N, adding at most 30,720 payload bytes over
the supported plans; allocator/FFT overhead is outside this payload bound.
Nominal 64-bit vector-descriptor storage falls by 48 bytes, without an object-size
or whole-process memory claim. Encode/decode processing adds no heap allocations.
The internal window view was checked against the literal canonical formula for
all 186 tested full-overlap/120-sample-overlap plan pairs; it is not a public API change.

Linked code grew by 512 text bytes. Conservative normal Rust stack-chain bounds
were 58,576 bytes for the encoder and 26,672 for the decoder, below the prospective
59,040/27,232 limits. These exclude callers, platform code, panic and unwinding.
The object-level numerical-pool audit found no added numerical payload; the mixed
linked constant section changed and is not claimed byte-identical.

The measured candidate source is `8a2a066bcc6743acec38748c1232b03ad610d06c` with parent
`2379d4a25897fba99b293c196fb6d6b8acc6e43b`. The immutable baseline executable was
compiled from `038a5b80a57aeb2ae3bb911fa752a7e63b332d78`; its runtime/build inputs are
identical to published `56a347c6` and `2379d4a2`. It was not recompiled at `2379d4a2`. Exact build39,
executable, library and helper hashes and compiler flags are in `source/`.

CSV CPU rows and the API record ledger retain their original bytes. Control CSV
and JSON projections substitute private strings with unique evidence IDs. The
private reverse map proves every selected field can be restored exactly; it is
excluded from publication. No raw audio, full trace/fixture payload, local path,
private application name or signed URL is included. `artifacts.sha256` binds the
public files. Projection IDs preserve owner-hash linkage without disclosing local
storage paths.
