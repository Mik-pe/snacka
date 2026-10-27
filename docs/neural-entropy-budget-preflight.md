# Entropy-prior readiness: certify the budget before fitting

This CPU-only source and exact-integer preflight accompanies the candidate-conditioned PLC study. It does not install a new CDF, change packet syntax, or train an entropy model.

The current native format allocates pulses from **certified worst-case costs** before coding their shapes. For all 560 supported dimension/pulse pairs (seven dimensions, K=1…80), the installed occupied-count prior already attains the smallest possible integer class bound among every positive-width, full-support CDF with total 65,535. The exact check found **0 lower class bounds**, hence no lower allocation-table entry from changing this prior alone.

This is a result about the existing budget, not the entropy of speech. It does not say that the installed prior minimizes average coded bits, or that a future adaptive format cannot benefit from learned probabilities.

## Exact feasibility certificate

For a class cardinality V_j, CDF total Q, and positive integer width w_j, the existing rule is:

H = 1 + max_j ceil(log2(V_j Q / w_j)).

For a proposed integer B=H−1, this bound is feasible exactly when:

sum_j ceil(V_j Q / 2^B) <= Q.

These are the smallest legal widths for that bound. If their sum fits, remaining units can be assigned without increasing the bound. If it exceeds Q, no full-support integer prior can attain that bound.

The [machine-readable result](neural-entropy-budget-preflight.json) records a feasible width-sum witness at the optimum and an infeasible witness one bit below for each pair. All 560 installed H values match these optima. The existing pulse/harmonic tables also pass their independent definition check.

## Implication for the next training experiment

A learned prior can improve average coding cost while leaving the certified H unchanged. In the current format, packet length determines the pulse budget; reduced arithmetic usage can become additional padding. It does not automatically create more pulses, lower the configured packet size, or improve decoded speech. There is no measured 10–15% bitrate saving here.

Before a neural context prior is justified:

1. Capture actual decoded class events (D, K, occupied count J), arithmetic tell and packet padding on the existing speaker-separated training/validation corpus. Observe the accepted packet decoding path; encoder trial encodes are not transmitted-symbol statistics.
2. Establish a smoothed static empirical prior as the first control. Compare held-out average class cost, full packet tell and the original certified bound. Any candidate CDF must retain positive widths and exact integer totals.
3. Define how saved average bits become a user benefit under the allocation and packet-length rules. A changed probability model or allocation rule requires a negotiated native version/profile and matching deterministic encoder/decoder behavior.
4. Start with context already decoded within the same packet, such as gains and previously decoded bands. Cross-packet state requires an explicit loss/reset synchronization design before training.
5. Require independent fixed-point CDF parity, malformed-stream checks, zero steady-state allocations and end-to-end packet/quality evidence before promotion.

The current source paths are [class selection and coding](../src/pulse.rs), [generated CDFs](../src/pulse_classes.rs), and the normative [allocation and interval bound](bitstream.md#deterministic-pulse-allocation). Standard Opus is outside this work.

Reproduction:

```sh
python3 tools/generate_pulse_tables.py --check
python3 tools/check_neural_entropy_budget.py --output /tmp/snacka-entropy-budget.json
```

No new speech data or Colab compute was needed for this preflight.
