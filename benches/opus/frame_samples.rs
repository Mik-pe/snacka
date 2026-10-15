// SPDX-License-Identifier: MIT

/// Standard public encoder call durations, expressed as 48 kHz samples/channel.
pub(super) fn valid(samples: usize) -> bool {
    matches!(
        samples,
        120 | 240 | 480 | 960 | 1920 | 2880 | 3840 | 4800 | 5760
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_nine_standard_durations_are_accepted() {
        let durations_us = [
            2500usize, 5000, 10000, 20000, 40000, 60000, 80000, 100000, 120000,
        ];
        for samples in 0..=6000 {
            let expected = durations_us
                .iter()
                .any(|&duration| duration as u64 * 48_000 == samples as u64 * 1_000_000);
            assert_eq!(super::valid(samples), expected, "samples={samples}");
        }
        assert!(!super::valid(usize::MAX));
    }
}
