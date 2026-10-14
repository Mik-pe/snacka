// SPDX-License-Identifier: MIT
//! Fixed-20-ms Opus over plain RTP/UDP for a negotiated local test peer.
//! Session negotiation, SRTP, RTCP and device capture/playback belong to the app.

use std::{
    collections::HashMap,
    env,
    fs::File,
    io::{self, BufWriter, Write},
    net::{SocketAddr, UdpSocket},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use snacka::opus::{
    Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, Packet, Recovery,
};

const SAMPLES: usize = 960;
const FRAME: Duration = Duration::from_millis(20);
const PREBUFFER: Duration = Duration::from_millis(60);
const MAX_AHEAD: u16 = 16;
const MAX_DATAGRAM: usize = 1_500;
const RTP_HEADER: usize = 12;

#[derive(Clone, Copy, Debug)]
struct Rtp<'a> {
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
    marker: bool,
    payload: &'a [u8],
}

impl<'a> Rtp<'a> {
    fn parse(bytes: &'a [u8], payload_type: u8) -> Option<Self> {
        if bytes.len() < RTP_HEADER || bytes[0] >> 6 != 2 || bytes[1] & 127 != payload_type {
            return None;
        }
        let mut start = RTP_HEADER + usize::from(bytes[0] & 15) * 4;
        if start > bytes.len() {
            return None;
        }
        if bytes[0] & 16 != 0 {
            let extension = bytes.get(start..start + 4)?;
            let words = u16::from_be_bytes(extension[2..4].try_into().ok()?);
            start += 4 + usize::from(words) * 4;
        }
        let mut end = bytes.len();
        if bytes[0] & 32 != 0 {
            let padding = usize::from(*bytes.last()?);
            if padding == 0 || padding > end.saturating_sub(start) {
                return None;
            }
            end -= padding;
        }
        let payload = bytes.get(start..end)?;
        if payload.is_empty() {
            return None;
        }
        Some(Self {
            sequence: u16::from_be_bytes(bytes[2..4].try_into().ok()?),
            timestamp: u32::from_be_bytes(bytes[4..8].try_into().ok()?),
            ssrc: u32::from_be_bytes(bytes[8..12].try_into().ok()?),
            marker: bytes[1] & 128 != 0,
            payload,
        })
    }
}

fn packetize(frame: Rtp<'_>, payload_type: u8, output: &mut [u8]) -> io::Result<usize> {
    let length = RTP_HEADER + frame.payload.len();
    if payload_type > 127 || frame.payload.is_empty() || length > output.len() {
        return Err(io::Error::other(
            "RTP output capacity or payload type invalid",
        ));
    }
    output[0] = 0x80;
    output[1] = payload_type | (u8::from(frame.marker) << 7);
    output[2..4].copy_from_slice(&frame.sequence.to_be_bytes());
    output[4..8].copy_from_slice(&frame.timestamp.to_be_bytes());
    output[8..12].copy_from_slice(&frame.ssrc.to_be_bytes());
    output[12..length].copy_from_slice(frame.payload);
    Ok(length)
}

struct Options {
    send: bool,
    bind: SocketAddr,
    peer: SocketAddr,
    frames: usize,
    channels: usize,
    payload_type: u8,
    bitrate: u32,
    drop_every: usize,
    fec: bool,
    dtx: bool,
    quiet_first: usize,
    output: PathBuf,
}

fn options() -> Result<Options, Box<dyn std::error::Error>> {
    let mut arguments = env::args().skip(1);
    let command = arguments.next().unwrap_or_default();
    if !matches!(command.as_str(), "send" | "recv") {
        return Err("usage: opus_call send|recv --bind IP:PORT --peer IP:PORT [--frames N --channels 1|2 --pt 111 --bitrate 32000 --drop-every N --fec true|false --dtx true|false --quiet-first N --output received.f32]".into());
    }
    let mut result = Options {
        send: command == "send",
        bind: if command == "send" {
            "127.0.0.1:5000"
        } else {
            "127.0.0.1:5002"
        }
        .parse()?,
        peer: if command == "send" {
            "127.0.0.1:5002"
        } else {
            "127.0.0.1:5000"
        }
        .parse()?,
        frames: 100,
        channels: 1,
        payload_type: 111,
        bitrate: 32_000,
        drop_every: 0,
        fec: false,
        dtx: false,
        quiet_first: 0,
        output: "received.f32".into(),
    };
    while let Some(option) = arguments.next() {
        let value = arguments.next().ok_or("option needs a value")?;
        match option.as_str() {
            "--bind" => result.bind = value.parse()?,
            "--peer" => result.peer = value.parse()?,
            "--frames" => result.frames = value.parse()?,
            "--channels" => result.channels = value.parse()?,
            "--pt" => result.payload_type = value.parse()?,
            "--bitrate" => result.bitrate = value.parse()?,
            "--drop-every" => result.drop_every = value.parse()?,
            "--fec" => result.fec = value.parse()?,
            "--dtx" => result.dtx = value.parse()?,
            "--quiet-first" => result.quiet_first = value.parse()?,
            "--output" => result.output = value.into(),
            _ => return Err(format!("unknown option {option}").into()),
        }
    }
    if !(1..=3_000).contains(&result.frames)
        || !(1..=2).contains(&result.channels)
        || !(96..=127).contains(&result.payload_type)
        || result.quiet_first > result.frames
    {
        return Err("frames must be 1..3000, channels 1 or 2, dynamic payload type 96..127".into());
    }
    Ok(result)
}

fn voiced_audio(output: &mut [f32], channels: usize, frame: usize) {
    for (index, sample) in output.iter_mut().enumerate() {
        let position = frame * SAMPLES + index / channels;
        let time = position as f32 / 48_000.0;
        let envelope = (std::f32::consts::PI * (time % 0.7) / 0.7).sin().powi(2);
        let mut voice = 0.0;
        for harmonic in 1..=6 {
            voice += (std::f32::consts::TAU * 137.0 * harmonic as f32 * time).sin()
                / (harmonic * harmonic) as f32;
        }
        *sample = 0.2 * envelope * voice;
    }
}

fn send(socket: &UdpSocket, options: &Options) -> Result<(), Box<dyn std::error::Error>> {
    let config = EncoderConfig::new(
        options.bitrate,
        options.channels,
        FrameDuration::Ms20,
        Bandwidth::Wideband,
        Application::Voice,
    )?
    .with_fec(options.fec)?
    .with_dtx(options.dtx)?;
    let mut encoder = Encoder::new(config)?;
    let mut input = [0.0; SAMPLES * 2];
    let mut payload = [0; 1_276];
    let mut datagram = [0; MAX_DATAGRAM];
    let mut deadline = Instant::now();
    let mut sent = 0;
    let mut fec_packets = 0;
    let mut dtx_packets = 0;
    for index in 0..options.frames {
        voiced_audio(
            &mut input[..SAMPLES * options.channels],
            options.channels,
            index,
        );
        if index < options.quiet_first {
            input.fill(0.0);
        }
        let length = encoder.encode(&input[..SAMPLES * options.channels], &mut payload)?;
        fec_packets += usize::from(encoder.has_fec());
        dtx_packets += usize::from(encoder.was_dtx());
        // Deliberately cross sequence and timestamp wrap during a short run.
        let frame = Rtp {
            sequence: 65_530u16.wrapping_add(index as u16),
            timestamp: (u32::MAX - 3 * SAMPLES as u32).wrapping_add(index as u32 * SAMPLES as u32),
            ssrc: 0x5350_4b41,
            marker: index == 0,
            payload: &payload[..length],
        };
        let bytes = packetize(frame, options.payload_type, &mut datagram)?;
        if options.drop_every == 0 || (index + 1) % options.drop_every != 0 {
            socket.send(&datagram[..bytes])?;
            sent += 1;
        }
        deadline += FRAME;
        thread::sleep(deadline.saturating_duration_since(Instant::now()));
    }
    println!(
        "RTP Opus sent={sent}, simulated_loss={}, clock=48000, samples_per_packet={SAMPLES}, channels={}, fec_packets={fec_packets}, dtx_packets={dtx_packets}, delay_samples={}, final_range={}",
        options.frames - sent,
        options.channels,
        encoder.algorithmic_delay_samples(),
        encoder.final_range()
    );
    Ok(())
}

struct Queued {
    timestamp: u32,
    payload: Vec<u8>,
}

fn receive(socket: &UdpSocket, options: &Options) -> Result<(), Box<dyn std::error::Error>> {
    socket.set_nonblocking(true)?;
    let mut decoder = Decoder::new(options.channels)?;
    let mut output = BufWriter::new(File::create(&options.output)?);
    let mut queued = HashMap::<u16, Queued>::new();
    let mut pcm = [0.0; SAMPLES * 2];
    let mut datagram = [0; MAX_DATAGRAM + 1];
    let mut next = None;
    let mut timestamp = 0u32;
    let mut ssrc = 0u32;
    let mut deadline = Instant::now();
    let mut last_contact = Instant::now();
    let mut played = 0;
    let mut normal = 0;
    let mut fec = 0;
    let mut plc = 0;
    let mut rejected = 0;
    while played < options.frames {
        for _ in 0..64 {
            let length = match socket.recv(&mut datagram) {
                Ok(length) => length,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            };
            if length > MAX_DATAGRAM {
                rejected += 1;
                continue;
            }
            let Some(packet) = Rtp::parse(&datagram[..length], options.payload_type) else {
                rejected += 1;
                continue;
            };
            if Packet::parse(packet.payload)
                .and_then(|packet| packet.samples(48_000))
                .ok()
                != Some(SAMPLES)
            {
                rejected += 1;
                continue;
            }
            if next.is_none() {
                next = Some(packet.sequence);
                timestamp = packet.timestamp;
                ssrc = packet.ssrc;
                deadline = Instant::now() + PREBUFFER;
            }
            let mut sequence = next.unwrap();
            let behind = sequence.wrapping_sub(packet.sequence);
            if played == 0
                && behind > 0
                && behind < MAX_AHEAD
                && packet.ssrc == ssrc
                && packet
                    .timestamp
                    .wrapping_add(u32::from(behind) * SAMPLES as u32)
                    == timestamp
            {
                sequence = packet.sequence;
                next = Some(sequence);
                timestamp = packet.timestamp;
                queued.retain(|&queued, _| queued.wrapping_sub(sequence) < MAX_AHEAD);
            }
            let ahead = packet.sequence.wrapping_sub(sequence);
            if packet.ssrc != ssrc
                || ahead >= MAX_AHEAD
                || packet.timestamp != timestamp.wrapping_add(u32::from(ahead) * SAMPLES as u32)
                || queued.contains_key(&packet.sequence)
            {
                rejected += 1;
                continue;
            }
            queued.insert(
                packet.sequence,
                Queued {
                    timestamp: packet.timestamp,
                    payload: packet.payload.to_vec(),
                },
            );
            last_contact = Instant::now();
        }
        let now = Instant::now();
        if now.duration_since(last_contact) > Duration::from_secs(3) {
            return Err("RTP peer timeout".into());
        }
        if let Some(sequence) = next
            && now >= deadline
        {
            let frame = &mut pcm[..SAMPLES * options.channels];
            if let Some(packet) = queued.remove(&sequence) {
                debug_assert_eq!(packet.timestamp, timestamp);
                if decoder.decode(&packet.payload, frame)? != SAMPLES {
                    return Err("negotiated 20 ms packet duration changed".into());
                }
                normal += 1;
            } else if let Some(packet) = queued.get(&sequence.wrapping_add(1)) {
                // Keep the following packet queued: recover the lost interval
                // now, then decode this same payload normally at its deadline.
                match decoder.recover(&packet.payload, SAMPLES, frame)? {
                    Recovery::Fec => fec += 1,
                    Recovery::Concealed => plc += 1,
                }
            } else {
                decoder.conceal(SAMPLES, frame)?;
                plc += 1;
            }
            if frame.iter().any(|sample| !sample.is_finite()) {
                return Err("nonfinite decoded RTP PCM".into());
            }
            for sample in frame {
                output.write_all(&sample.to_le_bytes())?;
            }
            played += 1;
            next = Some(sequence.wrapping_add(1));
            timestamp = timestamp.wrapping_add(SAMPLES as u32);
            deadline += FRAME;
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    }
    output.flush()?;
    println!(
        "RTP Opus played={played}, decoded={normal}, fec={fec}, plc={plc}, rejected={rejected}, channels={}, final_range={}",
        options.channels,
        decoder.final_range()
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = options()?;
    let socket = UdpSocket::bind(options.bind)?;
    socket.connect(options.peer)?;
    println!("RTP local={}, peer={}", socket.local_addr()?, options.peer);
    if options.send {
        send(&socket, &options)
    } else {
        receive(&socket, &options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtp_preserves_payload_sequence_timestamp_wrap_and_marker() {
        let payload = [0xf8, 0xff, 0xfe];
        let mut bytes = [0; 64];
        let frame = Rtp {
            sequence: u16::MAX,
            timestamp: u32::MAX - 10,
            ssrc: 7,
            marker: true,
            payload: &payload,
        };
        let length = packetize(frame, 111, &mut bytes).unwrap();
        let parsed = Rtp::parse(&bytes[..length], 111).unwrap();
        assert_eq!(
            (
                parsed.sequence,
                parsed.timestamp,
                parsed.ssrc,
                parsed.marker
            ),
            (frame.sequence, frame.timestamp, 7, true)
        );
        assert_eq!(parsed.payload, payload);
        assert_eq!(parsed.sequence.wrapping_add(1), 0);
        assert_eq!(parsed.timestamp.wrapping_add(11), 0);
        assert!(Rtp::parse(&bytes[..length], 112).is_none());
    }

    #[test]
    fn rtp_bounds_csrc_extensions_and_padding_before_reading_payload() {
        let mut bytes = [0; 20];
        bytes[0] = 0x91; // Version 2, extension, one CSRC.
        bytes[1] = 111;
        bytes[18..20].copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(Rtp::parse(&bytes, 111).is_none());
        bytes[0] = 0xa0; // Padding with zero count is invalid.
        bytes[19] = 0;
        assert!(Rtp::parse(&bytes, 111).is_none());
        bytes[19] = 20;
        assert!(Rtp::parse(&bytes, 111).is_none());
        assert!(Rtp::parse(&bytes[..11], 111).is_none());
    }
}
