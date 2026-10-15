// SPDX-License-Identifier: MIT

/// Standard public encoder call durations, expressed as 48 kHz samples/channel.
pub(super) fn valid(samples: usize) -> bool {
    matches!(
        samples,
        120 | 240 | 480 | 960 | 1920 | 2880 | 3840 | 4800 | 5760
    )
}
