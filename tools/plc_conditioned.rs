// SPDX-License-Identifier: MIT
//! Experimental baseline-preserving residual gains, conditioned after native MDCT.
use crate::{neural_plc_weights as base, plc_conditioned_weights as adapter};
const _: () = assert!(base::FORMAT_TAG == 1 && adapter::FORMAT_TAG == 3);
pub const RADIUS: f32 = 0.10;
pub fn latent(input: &[f32; 80]) -> [f32; 64] {
    let a = crate::neural_plc::layer(input, &base::W1, &base::S1, &base::B1, true);
    crate::neural_plc::layer(&a, &base::W2, &base::S2, &base::B2, true)
}
pub fn base_controls(h: &[f32; 64]) -> ([f32; 16], f32) {
    let y = crate::neural_plc::layer(h, &base::W3, &base::S3, &base::B3, false);
    (
        std::array::from_fn(|b| 1.0 + 0.2125 * y[b].tanh()),
        8.0 * y[16].tanh(),
    )
}
pub fn powers(c: &[f32]) -> [f32; 16] {
    let edges = crate::neural_plc::edges(c.len());
    std::array::from_fn(|b| {
        let s = &c[edges[b]..edges[b + 1]];
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32 + 1e-12).ln()
    })
}
pub fn condition(candidate: &[f32], last: &[f32; 16]) -> [f32; 16] {
    let now = powers(candidate);
    std::array::from_fn(|b| (0.5 * (now[b] - last[b])).clamp(-4.0, 4.0))
}
pub fn gains(h: &[f32; 64], current: &[f32; 16], original: &[f32; 16]) -> [f32; 16] {
    std::array::from_fn(|b| {
        let hist: f32 = adapter::WH[b]
            .iter()
            .zip(h)
            .map(|(&w, &x)| f32::from(w) * x)
            .sum();
        let energy: f32 = adapter::WC[b]
            .iter()
            .zip(current)
            .map(|(&w, &x)| f32::from(w) * x)
            .sum();
        let raw = hist * adapter::SH[b] + energy * adapter::SC[b] + adapter::BIAS[b];
        let residual = RADIUS * (raw / RADIUS).tanh();
        let delta = if adapter::DAMP_ONLY { residual.min(0.0) } else { residual } * adapter::STRENGTH;
        // Exact identity, not exp(ln(g)), for zero residual and disabled adapters.
        if !adapter::ENABLED || delta == 0.0 || !delta.is_finite() {
            original[b]
        } else {
            (original[b].ln() + delta)
                .clamp(0.7875f32.ln(), 1.2125f32.ln())
                .exp()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_absolute_power_matches_direct_calculation_and_reset() {
        let mut history=crate::neural_plc::History::new();
        for n in [480,960] {
            let c:Vec<f32>=(0..n).map(|i| if i%7==0 {0.25} else {-0.125}).collect();
            history.observe(&c,24000,0);
            assert_eq!(history.last_log_power().map(f32::to_bits),powers(&c).map(f32::to_bits));
            history.observe(&vec![0.;n],24000,0);
            assert_eq!(history.last_log_power().map(f32::to_bits),[1e-12f32.ln().to_bits();16]);
            history.reset();
            assert_eq!(history.last_log_power(),[0.;16]);
        }
    }
    #[test]
    fn exported_adapter_matches_quantized_pytorch_applied_gains() {
        let Ok(path)=std::env::var("SNACKA_ADAPTER_FIXTURE") else { return };
        let bytes=std::fs::read(path).unwrap();
        assert_eq!(bytes.len(),32*112*4);
        let mut maximum=0.0f32;
        for record in bytes.chunks_exact(448) {
            let f:Vec<f32>=record.chunks_exact(4).map(|b|f32::from_le_bytes(b.try_into().unwrap())).collect();
            let h:[f32;64]=f[..64].try_into().unwrap();
            let c:[f32;16]=f[64..80].try_into().unwrap();
            let base:[f32;16]=f[80..96].try_into().unwrap();
            for (i,actual) in gains(&h,&c,&base).into_iter().enumerate() {
                let expected=f[96+i];
                maximum=maximum.max((actual-expected).abs());
                assert!((0.78749..=1.21251).contains(&actual));
                if adapter::DAMP_ONLY { assert!(actual<=base[i]+2e-7); }
            }
        }
        assert!(maximum<5e-5,"native applied-gain parity {maximum}");
        println!("adapter max absolute parity error {maximum}");
    }
}
