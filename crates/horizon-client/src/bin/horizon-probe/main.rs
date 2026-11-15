//! Standalone network probe: deliberately never constructs Game or Resources.
use horizon_client::net::{
    Event,
    probe::{self, Summary},
};
use horizon_protocol::packets::{Empty, play};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: horizon-probe HOST:PORT NAME [--seconds N] [--walk BLOCKS] [--chat TEXT]";
    let address = args.next().ok_or(usage)?;
    let name = args.next().ok_or(usage)?;
    if name.is_empty()
        || name.len() > 16
        || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err("offline NAME must contain 1..16 ASCII letters, digits or underscores".into());
    }
    let mut seconds = 10_u64;
    let mut walk = 0.0_f64;
    let mut chat = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(usage)?;
        match flag.as_str() {
            "--seconds" => seconds = value.parse().map_err(|_| usage)?,
            "--walk" => walk = value.parse().map_err(|_| usage)?,
            "--chat" => chat = Some(value),
            _ => return Err(usage.into()),
        }
    }
    if seconds == 0 || seconds > 3600 || !walk.is_finite() || !(0.0..=16.0).contains(&walk) {
        return Err("seconds must be 1..3600; walk must be 0..16 blocks".into());
    }
    if chat
        .as_ref()
        .is_some_and(|s: &String| s.chars().count() > 256)
    {
        return Err("chat must be at most 256 characters".into());
    }
    let summary = Arc::new(Mutex::new(Summary::default()));
    let mut session = probe::connect(&address, &name, summary.clone());
    let started = Instant::now();
    let mut joined = None;
    let mut position = [0.0; 3];
    let mut yaw = 0.0;
    let mut pitch = 0.0;
    let mut placed = false;
    let mut walked = 0.0;
    let mut next_step = Instant::now();
    let mut failure = None;
    'receive: loop {
        for event in session.poll() {
            match event {
                Event::Joined { .. } => {
                    summary.lock().unwrap().joined = true;
                    joined = Some(Instant::now());
                }
                Event::Packet(packet) => match *packet {
                    play::Clientbound::PlayerPosition(p) => {
                        for (axis, value) in position.iter_mut().enumerate() {
                            *value = p.position[axis]
                                + if p.relatives & (1 << axis) != 0 {
                                    *value
                                } else {
                                    0.0
                                };
                        }
                        yaw = p.yaw + if p.relatives & 8 != 0 { yaw } else { 0.0 };
                        pitch = p.pitch + if p.relatives & 16 != 0 { pitch } else { 0.0 };
                        session.send(play::Serverbound::AcceptTeleportation(
                            play::AcceptTeleportation {
                                id: p.teleport,
                                x: position[0],
                                y: position[1],
                                z: position[2],
                                yaw,
                                pitch,
                            },
                        ));
                        session.send(play::Serverbound::PlayerLoaded(Empty));
                        placed = true;
                    }
                    play::Clientbound::Ping(p) => session.send(play::Serverbound::Pong(p)),
                    _ => {}
                },
                Event::PackPush(p) => session.send_pack_status(p.id, 1),
                Event::CodeOfConduct(_) => {
                    failure = Some(
                        "server requires code-of-conduct acceptance; probe does not accept it"
                            .into(),
                    );
                    break 'receive;
                }
                Event::Transfer { host, port } => {
                    failure = Some(format!("server requested transfer to {host}:{port}"));
                    break 'receive;
                }
                Event::Disconnected { reason, .. } => {
                    failure = Some(reason);
                    break 'receive;
                }
                _ => {}
            }
        }
        if placed {
            if let Some(message) = chat.take() {
                session.send(play::Serverbound::Chat(play::Chat {
                    message,
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as i64),
                    salt: 0,
                    signature: None,
                    last_seen: play::LastSeenUpdate {
                        offset: 0,
                        acknowledged: [0; 3],
                        checksum: 0,
                    },
                }));
            }
            if Instant::now() >= next_step {
                let step = (walk - walked).min(0.1);
                position[0] += step;
                walked += step;
                session.send(play::Serverbound::MovePlayerPosRot(
                    play::MovePlayerPosRot {
                        x: position[0],
                        y: position[1],
                        z: position[2],
                        yaw,
                        pitch,
                        flags: 0,
                    },
                ));
                next_step = Instant::now() + Duration::from_millis(50);
            }
        }
        if joined.is_some_and(|t: Instant| t.elapsed() >= Duration::from_secs(seconds)) {
            break;
        }
        if joined.is_none() && started.elapsed() >= Duration::from_secs(60) {
            failure = Some("login/configuration timed out after 60 seconds".into());
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    session.disconnect();
    let mut summary = summary.lock().unwrap();
    if summary.reason.is_none() {
        summary.reason = failure.clone();
    }
    println!(
        "{}",
        serde_json::to_string(&*summary).map_err(|e| e.to_string())?
    );
    if failure.is_some() || summary.kicked {
        return Err("probe ended before its observation deadline; see summary".into());
    }
    Ok(())
}
