// SPDX-License-Identifier: MIT
#[path = "../call_common/relay.rs"]
mod relay;
#[path = "../call_common/transport.rs"]
mod transport;

use std::{
    error::Error,
    io,
    net::UdpSocket,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut bind = "0.0.0.0:5000".to_owned();
    let mut seconds: Option<f64> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Snacka two-client UDP relay\n\nSet SNACKA_CALL_KEY (64 hex digits) on server and clients.\n  --bind ADDRESS     Default 0.0.0.0:5000; IPv6: [::]:5000\n  --seconds SECONDS  Stop automatically\n  --generate-key     Print a random room key and exit\n\nThe relay is trusted with the key. One room, roles 1 and 2. Ctrl-C quits."
                );
                return Ok(());
            }
            "--generate-key" => {
                println!("{}", transport::generate_key()?);
                return Ok(());
            }
            "--bind" => {
                bind = args
                    .next()
                    .ok_or_else(|| io::Error::other("Missing --bind value"))?
            }
            "--seconds" => {
                seconds = Some(
                    args.next()
                        .ok_or_else(|| io::Error::other("Missing --seconds value"))?
                        .parse()?,
                )
            }
            _ => return Err(io::Error::other(format!("Unknown option {arg}; use --help")).into()),
        }
    }
    if seconds.is_some_and(|s| !s.is_finite() || s <= 0.0) {
        return Err(io::Error::other("--seconds must be finite and positive").into());
    }
    let socket = UdpSocket::bind(bind)?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut relay = relay::Relay::new(&transport::key_from_env()?)?;
    let running = Arc::new(AtomicBool::new(true));
    let signal = running.clone();
    ctrlc::set_handler(move || signal.store(false, Ordering::Relaxed))?;
    println!(
        "Relay listening on {} (UDP), one room, two clients",
        socket.local_addr()?
    );
    let start = Instant::now();
    let mut bytes = [0; transport::MAX_DATAGRAM + 1];
    while running.load(Ordering::Relaxed)
        && seconds.is_none_or(|s| start.elapsed().as_secs_f64() < s)
    {
        match socket.recv_from(&mut bytes) {
            Ok((length, source)) => {
                // A transient send failure to one client must not stop the room.
                if let Err(error) = relay.receive(&socket, &bytes[..length], source, Instant::now())
                {
                    eprintln!("Relay send: {error}");
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                relay.expire(Instant::now())
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
