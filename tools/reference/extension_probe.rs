// SPDX-License-Identifier: MIT
//! Inspect length-prefixed packets with the owned borrowed framing parser.
#![forbid(unsafe_code)]

#[allow(dead_code)]
#[path = "../../src/opus/extensions.rs"]
mod extensions;
#[allow(dead_code)]
#[path = "../../src/opus/packet.rs"]
mod packet;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: extension_probe RECORDS")?;
    let bytes = std::fs::read(path)?;
    let mut remaining = bytes.as_slice();
    let mut index = 0;
    while !remaining.is_empty() {
        let header = remaining.get(..4).ok_or("missing record length")?;
        let length = u32::from_le_bytes(header.try_into()?) as usize;
        let data = remaining.get(4..4 + length).ok_or("truncated record")?;
        remaining = &remaining[4 + length..];
        let packet = packet::Packet::parse(data)?;
        print!(
            "{{\"packet\":{index},\"frames\":{},\"padding\":{},\"toc\":{},\"mode\":\"{:?}\",\"bandwidth\":\"{:?}\",\"extensions\":[",
            packet.frame_count(),
            packet.padding_len(),
            packet.toc().byte(),
            packet.toc().mode(),
            packet.toc().bandwidth()
        );
        let mut first = true;
        let mut error = None;
        for item in packet.extensions() {
            match item {
                Ok(extension) => {
                    if !first {
                        print!(",");
                    }
                    first = false;
                    print!(
                        "{{\"frame\":{},\"id\":{},\"length\":{},\"prefix_hex\":\"",
                        extension.frame_index,
                        extension.id,
                        extension.payload.len()
                    );
                    for byte in extension.payload.iter().take(16) {
                        print!("{byte:02x}");
                    }
                    print!("\"}}");
                }
                Err(problem) => {
                    error = Some(problem);
                    break;
                }
            }
        }
        match error {
            Some(problem) => println!("],\"error\":\"{problem}\"}}"),
            None => println!("],\"error\":null}}"),
        }
        index += 1;
    }
    Ok(())
}
