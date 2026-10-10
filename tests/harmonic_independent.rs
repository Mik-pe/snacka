// SPDX-License-Identifier: MIT
use snacka::{ALGORITHMIC_DELAY, Decoder, FrameDuration, Packet};
fn repair_crc(packet: &mut [u8]) {
    let end = packet.len() - 2;
    let mut crc = 0xffffu16;
    for &byte in &packet[..end] {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    packet[end..].copy_from_slice(&crc.to_le_bytes());
}
fn signed_class_model(dimensions: usize, pulses: usize) -> Option<(usize, Vec<u16>)> {
    if ![20,40,80,160].contains(&dimensions) || !(1..=80).contains(&pulses) {
        return None;
    }
    let choose = |n: usize, k: usize| {
        (0..k.min(n - k)).fold(1.0f64, |value, i| value * (n - i) as f64 / (i + 1) as f64)
    };
    let classes: Vec<_> = (1..=dimensions.min(pulses))
        .map(|occupied| {
            choose(dimensions, occupied)
                * choose(pulses - 1, occupied - 1)
                * 2.0f64.powi(occupied as i32)
        })
        .collect();
    let count: f64 = classes.iter().sum();
    let quotas: Vec<_> = classes
        .iter()
        .map(|&count_in_class| count_in_class * (65535 - classes.len()) as f64 / count)
        .collect();
    let mut widths: Vec<_> = quotas
        .iter()
        .map(|quota| 1 + quota.floor() as usize)
        .collect();
    let mut order: Vec<_> = (0..classes.len()).collect();
    order.sort_by(|&a, &b| {
        quotas[b]
            .fract()
            .total_cmp(&quotas[a].fract())
            .then(a.cmp(&b))
    });
    let remaining = 65535 - widths.iter().sum::<usize>();
    for &i in &order[..remaining] {
        widths[i] += 1;
    }
    let bound = classes
        .iter()
        .zip(&widths)
        .map(|(&count, &width)| count * 65535.0 / width as f64)
        .fold(0.0f64, f64::max);
    let bits = (bound.log2() - 1e-12).ceil() as usize + 1;
    let mut boundaries = vec![0u16];
    for width in widths {
        boundaries.push((usize::from(*boundaries.last().unwrap()) + width) as u16);
    }
    Some((bits, boundaries))
}
fn encode_subset(writer: &mut snacka::entropy::Encoder<'_>, slots: usize, positions: &[usize]) {
    let mut selected = 0;
    for position in 0..slots {
        let remaining = positions.len() - selected;
        if remaining == 0 {
            break;
        }
        let available = slots - position;
        let present = positions[selected] == position;
        if remaining != available {
            writer
                .symbol(
                    usize::from(present),
                    snacka::entropy::Model::new(&[
                        0,
                        (available - remaining) as u16,
                        available as u16,
                    ])
                    .unwrap(),
                )
                .unwrap();
        }
        selected += usize::from(present);
    }
}
fn harmonic_packet(
    gains: [u32; 4],
    profile: u8,
    length: usize,
    concentrated: bool,
    f0: usize,
) -> (Vec<u8>, Vec<f64>) {
    let samples = if profile & 1 == 0 { 960 } else { 480 };
    let gain_bits = ((length - 4) * 240 / samples).clamp(5, 8) as u32;
    let gain_step = 1 << (8 - gain_bits);
    assert!(gains.iter().all(|&gain| gain % gain_step == 0));
    let counts: Vec<usize> = [0usize,1000,2000,4000].into_iter().zip([1000usize,2000,4000,8001]).map(|(lo,hi)| {
        ((hi-1)/f0 + 1).saturating_sub(lo.div_ceil(f0).max(1))*2
    }).collect();
    let dimensions: Vec<usize> = counts.iter().map(|&count| [2,4,8,16,20,40,80,160].into_iter().find(|&d|d>=count).unwrap()).collect();
    static CACHE: std::sync::OnceLock<std::collections::BTreeMap<usize,(Vec<Option<(usize,Vec<u16>)>>,Vec<usize>)>>=std::sync::OnceLock::new();
    let cache=CACHE.get_or_init(|| [2usize,4,8,16,20,40,80,160].into_iter().map(|dimension| {
        let classes: Vec<_>=(0..388).map(|k|signed_class_model(dimension,k)).collect();
        let mut logarithm=0.0f64;let mut previous=0;
        let costs=(0..388).map(|pulses| {
            if pulses!=0 {logarithm+=((dimension+pulses-1) as f64/pulses as f64).log2();}
            let multiset=if dimension<=16 {
                let count=(0..pulses.min(dimension-1)).fold(1u128,|value,index|value*(dimension+pulses-1-index) as u128/(index+1) as u128);
                (128-(count-1).leading_zeros()) as usize+pulses.min(dimension)
            } else { logarithm.ceil() as usize+pulses.min(dimension) };
            let budget=classes[pulses].as_ref().map_or(multiset,|(cost,_)|multiset.min(*cost));
            previous=previous.max(budget);previous
        }).collect();
        (dimension,(classes,costs))
    }).collect());
    let classes: Vec<_>=dimensions.iter().map(|d| &cache[d].0).collect();
    let costs: Vec<_>=dimensions.iter().map(|d| &cache[d].1).collect();
    let weights: Vec<_> = gains
        .iter()
        .map(|&code| {
            if code == 0 {
                0u128
            } else {
                let exponent = if code <= 64 {
                    f64::from(code) / 4.0
                } else {
                    16.0 + f64::from(code - 64) / 8.0
                };
                2.0f64.powf(exponent).round() as u128
            }
        })
        .collect();
    let mut allocation = gains.map(|g| usize::from(g != 0));
    let mut used =
        4 * gain_bits as usize + 4 + (0..4).map(|i| costs[i][allocation[i]]).sum::<usize>();
    loop {
        let mut best = None;
        let mut priority = (0u128, 1u128);
        for group in 0..4 {
            if gains[group] == 0 || allocation[group] == 387 {
                continue;
            }
            let extra = costs[group][allocation[group] + 1] - costs[group][allocation[group]];
            if used + extra + 9 > (length - 4) * 8 {
                continue;
            }
            let offset = (dimensions[group] as f64).sqrt().ceil() as usize;
            let score = (
                weights[group] * dimensions[group] as u128,
                ((allocation[group] + offset) as u128).pow(3) * extra as u128,
            );
            if score.0 * priority.1 > priority.0 * score.1 {
                best = Some((group, extra));
                priority = score;
            }
        }
        let Some((group, extra)) = best else {
            break;
        };
        allocation[group] += 1;
        used += extra;
    }
    let mut packet = vec![0; length];
    packet[..2].copy_from_slice(&[224 | profile, 0]);
    let end = length - 2;
    let mut writer = snacka::entropy::Encoder::new(&mut packet[2..end]);
    writer.raw((f0-75) as u32,9).unwrap();
    for gain in gains {
        writer.raw(gain / gain_step, gain_bits).unwrap();
    }
    let mut coefficients = vec![0.0; 2*(8000/f0)];
    for group in 0..4 {
        let mut values = vec![0i32; dimensions[group]];
        for i in 0..allocation[group] {
            let index = if concentrated {
                0
            } else if i % 3 == 0 {
                counts[group] - 1
            } else {
                (i * 17) % counts[group]
            };
            values[index] += if index % 2 == 0 { 1 } else { -1 };
        }
        let positions: Vec<_> = values
            .iter()
            .enumerate()
            .flat_map(|(frequency, q)| std::iter::repeat_n(frequency, q.unsigned_abs() as usize))
            .enumerate()
            .map(|(ordinal, frequency)| frequency + ordinal)
            .collect();
        if allocation[group] != 0 {
            let raw_multiset_cost = {
                let logarithm = (1..=allocation[group])
                    .map(|k| ((dimensions[group] + k - 1) as f64 / k as f64).log2())
                    .sum::<f64>();
                logarithm.ceil() as usize + allocation[group].min(dimensions[group])
            };
            if let Some((_, boundaries)) = classes[group][allocation[group]]
                .as_ref()
                .filter(|(cost, _)| *cost < raw_multiset_cost)
            {
                let occupied: Vec<_> = values
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &v)| (v != 0).then_some(i))
                    .collect();
                writer
                    .symbol(
                        occupied.len() - 1,
                        snacka::entropy::Model::new(boundaries).unwrap(),
                    )
                    .unwrap();
                encode_subset(&mut writer, dimensions[group], &occupied);
                for &i in &occupied {
                    writer.raw(u32::from(values[i] < 0), 1).unwrap();
                }
                let extra_positions: Vec<_> = occupied
                    .iter()
                    .enumerate()
                    .flat_map(|(bin, &i)| {
                        std::iter::repeat_n(bin, values[i].unsigned_abs() as usize - 1)
                    })
                    .enumerate()
                    .map(|(ordinal, bin)| ordinal + bin)
                    .collect();
                encode_subset(&mut writer, allocation[group] - 1, &extra_positions);
            } else {
                let slots = dimensions[group] + allocation[group] - 1;
                let mut selected = 0;
                let mut previous = None;
                for position in 0..slots {
                    let remaining = positions.len() - selected;
                    if remaining == 0 {
                        break;
                    }
                    let available = slots - position;
                    let present = positions[selected] == position;
                    if remaining != available {
                        writer
                            .symbol(
                                usize::from(present),
                                snacka::entropy::Model::new(&[
                                    0,
                                    (available - remaining) as u16,
                                    available as u16,
                                ])
                                .unwrap(),
                            )
                            .unwrap();
                    }
                    if present {
                        let frequency = position - selected;
                        if previous != Some(frequency) {
                            writer.raw(u32::from(values[frequency] < 0), 1).unwrap();
                        }
                        previous = Some(frequency);
                        selected += 1;
                    }
                }
            }
            let norm = values
                .iter()
                .map(|&v| f64::from(v).powi(2))
                .sum::<f64>()
                .sqrt();
            let exponent = if gains[group] <= 64 {
                -16.0 + f64::from(gains[group]) / 8.0
            } else {
                -8.0 + f64::from(gains[group] - 64) / 16.0
            };
            let scale = 2.0f64.powf(exponent) / norm;
            for (index, value) in values.iter().enumerate() {
                if index < counts[group] {
                    let lo = [0usize,1000,2000,4000][group];
                    let first = lo.div_ceil(f0).max(1);
                    coefficients[2*(first-1)+index] = f64::from(*value) * scale;
                }
            }
        }
    }
    assert!(writer.tell() <= ((length-4)*8) as u64, "entropy over budget f0={f0} profile={profile} length={length} gains={gains:?}");
    writer.finish_padded().unwrap();
    repair_crc(&mut packet);
    (packet, coefficients)
}
#[test]
fn every_f0_payload_and_gain_grid_fits_actual_entropy_buffer() {
    let mut packets=0usize;
    for profile in [14,15] {
        let samples=if profile==14 {960} else {480};
        let (min,max)=if profile==14 {(30,80)} else {(15,40)};
        for f0 in 75..=400 {
            for length in min..=max {
                let step=1 << (8-((length-4)*240/samples).clamp(5,8));
                for gains in [[0;4],[208;4],[step,64,128,208],[208,0,0,0],[0,0,0,208],[0,208,0,208]] {
                    let (packet,_)=harmonic_packet(gains,profile,length,(f0+length)%2==0,f0);
                    assert_eq!(Packet::parse(&packet).unwrap().gain_bands(),4);
                    packets+=1;
                }
            }
        }
    }
    println!("Independent harmonic entropy fixtures: {packets}");
}

#[test]
fn all_f0_minimum_and_maximum_packets_decode_with_finite_bounded_audio() {
    for profile in [14,15] {
        let duration=if profile==14 {FrameDuration::Ms20} else {FrameDuration::Ms10};
        let samples=duration.samples();
        let lengths=if profile==14 {[30,80]} else {[15,40]};
        let mut decoder=Decoder::with_frame_duration(duration);
        let mut output=vec![0.0;samples];
        for f0 in 75..=400 {
            for length in lengths {
                let step=1 << (8-((length-4)*240/samples).clamp(5,8));
                for gains in [[0;4],[208;4],[step,64,128,208],[208,0,0,0],[0,0,0,208],[0,208,0,208]] {
                    let (packet,_)=harmonic_packet(gains,profile,length,(f0+length)%2==0,f0);
                    decoder.reset();
                    decoder.decode(&packet,&mut output).unwrap();
                    assert!(output.iter().all(|x| x.is_finite() && x.abs()<=1.0));
                }
            }
        }
    }
}

fn direct_window(samples: usize, index: usize) -> f64 {
    let start=(samples-ALGORITHMIC_DELAY)/2;
    let reflected=index.min(2*samples-1-index);
    if reflected<start {0.0}
    else if reflected>=start+ALGORITHMIC_DELAY {1.0}
    else {
        let angle=std::f64::consts::FRAC_PI_2*((reflected-start) as f64+0.5)/ALGORITHMIC_DELAY as f64;
        (std::f64::consts::FRAC_PI_2*angle.sin().powi(2)).sin()
    }
}

#[test]
fn independently_coded_harmonics_match_direct_double_cosine_transforms() {
    for profile in [14,15] {
        let duration=if profile==14 {FrameDuration::Ms20} else {FrameDuration::Ms10};
        let samples=duration.samples();
        let start=(samples-ALGORITHMIC_DELAY)/2;
        let lengths=if profile==14 {[30,80]} else {[15,40]};
        let basis:Vec<Vec<f64>>=(0..2*samples).map(|time|(0..samples).map(|bin| {
            let phase=std::f64::consts::PI/samples as f64*(time as f64+0.5+samples as f64/2.0)*(bin as f64+0.5);
            direct_window(samples,time)*phase.cos()
        }).collect()).collect();
        for f0 in [76,89,119,149,213,251,333,399] {
            for length in lengths {
                for gains in [[64;4],[208;4]] {
                    let (packet,amplitudes)=harmonic_packet(gains,profile,length,(f0+length)%2==0,f0);
                    let mut waveform=vec![0.0f64;2*samples];
                    for (time,value) in waveform[start..start+samples+ALGORITHMIC_DELAY].iter_mut().enumerate() {
                        for (h,pair) in amplitudes.chunks_exact(2).enumerate() {
                            let phase=std::f64::consts::TAU*((h+1)*f0) as f64*time as f64/48000.0;
                            *value+=pair[0]*phase.cos()+pair[1]*phase.sin();
                        }
                    }
                    let coefficients:Vec<f64>=(0..samples).map(|bin| {
                        waveform.iter().enumerate().map(|(time,&value)| value*basis[time][bin]).sum::<f64>()*2.0/samples as f64
                    }).collect();
                    let expected:Vec<f64>=(start..start+samples).map(|time| {
                        coefficients.iter().zip(&basis[time]).map(|(&a,&b)|a*b).sum::<f64>().clamp(-1.0,1.0)
                    }).collect();
                    let mut output=vec![0.0;samples];
                    Decoder::with_frame_duration(duration).decode(&packet,&mut output).unwrap();
                    let error=output.iter().zip(&expected).map(|(&a,&b)|(f64::from(a)-b).abs()).fold(0.0f64,f64::max);
                    assert!(error<2e-4,"profile={profile} F0={f0} length={length} gains={gains:?} max error={error}");
                }
            }
        }
    }
}
