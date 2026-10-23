// SPDX-License-Identifier: MIT
#[test]
fn cache_is_exact_and_all_old_features_remain_bit_identical() {
    use crate::plc_history_control::History as Control;
    use crate::{neural_plc::History, target_energy::log_power};
    assert_eq!(
        std::mem::size_of::<History>() - std::mem::size_of::<Control>(),
        64
    );
    for n in [480, 960] {
        let mut current = History::new();
        let mut control = Control::new();
        let mut seed = 6716u32;
        for frame in 0..40 {
            let coefficients: Vec<f32> = (0..n)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    if frame % 7 == 0 {
                        0.0
                    } else {
                        (seed as f32 / u32::MAX as f32 - 0.5) * 0.01f32.powi(frame % 4)
                    }
                })
                .collect();
            let f0 = if frame % 2 == 0 { 173 } else { 0 };
            current.observe(&coefficients, 8000, f0);
            control.observe(&coefficients, 8000, f0);
            assert_eq!(
                current.last_log_power().map(f32::to_bits),
                log_power(&coefficients).map(f32::to_bits)
            );
            assert_eq!(
                current.input(n, [0.1; 5]).map(f32::to_bits),
                control.input(n, [0.1; 5]).map(f32::to_bits)
            );
            assert_eq!(current.clone().last_log_power(), current.last_log_power());
            if frame % 9 == 0 {
                current.reset();
                control.reset();
            }
        }
        current.reset();
        assert_eq!(current.last_log_power(), [0.0; 16]);
        assert_eq!(
            current.input(n, [0.0; 5]),
            History::new().input(n, [0.0; 5])
        );
    }
}
