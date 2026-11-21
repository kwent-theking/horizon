//! Asset-free packet observation for the headless probe. No world is retained.
use super::*;
use horizon_protocol::{Direction, State};
use horizon_world::nbt::Tag;
use serde::Serialize;
use std::collections::BTreeMap;

/// Counters are bounded by the protocol's packet names, rather than packet count.
#[derive(Default, Serialize)]
pub struct Summary {
    pub joined: bool,
    pub kicked: bool,
    pub reason: Option<String>,
    pub packets: BTreeMap<String, u64>,
    pub decode_errors: BTreeMap<String, DecodeErrors>,
    pub entity_movement_ok: u64,
    pub entity_movement_failed: u64,
    pub kinetic_speed_observations: u64,
    #[serde(skip)]
    block_entities: HashMap<i32, String>,
}

#[derive(Default, Serialize)]
pub struct DecodeErrors {
    pub count: u64,
    pub last_error: String,
}

fn packet_name(state: State, id: i32) -> String {
    let phase = match state {
        State::Login => "login",
        State::Configuration => "configuration",
        _ => "play",
    };
    format!(
        "{phase}/{}",
        horizon_protocol::ids::name(state, Direction::Clientbound, id).unwrap_or("unknown")
    )
}

fn movement(name: &str) -> bool {
    matches!(
        name,
        "play/move_entity_pos"
            | "play/move_entity_pos_rot"
            | "play/move_entity_rot"
            | "play/teleport_entity"
            | "play/entity_position_sync"
            | "play/set_entity_motion"
    )
}

pub(super) fn received(summary: Option<&Mutex<Summary>>, state: State, id: i32) {
    if let Some(summary) = summary {
        *summary
            .lock()
            .unwrap()
            .packets
            .entry(packet_name(state, id))
            .or_default() += 1;
    }
}

pub(super) fn decode_error(summary: Option<&Mutex<Summary>>, state: State, id: i32, error: &str) {
    if let Some(summary) = summary {
        let name = packet_name(state, id);
        // Preserve every diagnostic without retaining an unbounded history in RAM.
        eprintln!(
            "{}",
            serde_json::json!({"packet": name, "decode_error": error})
        );
        summary.lock().unwrap().error(&name, error);
    }
}

impl Summary {
    fn error(&mut self, name: &str, error: &str) {
        self.entity_movement_failed += u64::from(movement(name));
        let group = self.decode_errors.entry(name.into()).or_default();
        group.count += 1;
        group.last_error = error.chars().take(2048).collect();
    }

    pub(super) fn kick(&mut self, reason: String) {
        self.kicked = true;
        self.reason = Some(reason);
    }

    pub(super) fn play(&mut self, packet: &play::Clientbound) {
        self.entity_movement_ok += u64::from(movement(&packet_name(State::Play, packet.id())));
        match packet {
            play::Clientbound::Disconnect(d) => self.kick(plain(&d.reason, &Language::new())),
            play::Clientbound::BlockEntityData(p) => self.kinetic(p.pos, p.kind, &p.tag),
            play::Clientbound::LevelChunkWithLight(p) => {
                for entity in &p.chunk.block_entities {
                    self.kinetic(
                        [
                            p.x * 16 + i32::from((entity.packed_xz as u8) >> 4),
                            i32::from(entity.y),
                            p.z * 16 + i32::from(entity.packed_xz & 15),
                        ],
                        entity.kind,
                        &entity.tag,
                    );
                }
            }
            _ => {}
        }
    }

    fn kinetic(&mut self, pos: [i32; 3], kind: i32, tag: &Tag) {
        let name = tag
            .get("id")
            .and_then(|t| match t {
                Tag::String(s) => Some(s.to_string_lossy()),
                _ => None,
            })
            .or_else(|| self.block_entities.get(&kind).cloned());
        let speed = tag.get("Speed").and_then(|t| match t {
            Tag::Float(v) => Some(f64::from(f32::from_bits(*v))),
            Tag::Double(v) => Some(f64::from_bits(*v)),
            _ => None,
        });
        if let (Some(name), Some(speed)) = (name, speed)
            && name.starts_with("create:")
        {
            self.kinetic_speed_observations += 1;
            // Each observation is streamed, including repeated values and zero.
            println!(
                "{}",
                serde_json::json!({"block_entity": name, "position": pos, "Speed": speed})
            );
        }
    }
}

/// Uses the same connection, negotiation and translation as the game, without its resources.
pub fn connect(address: &str, name: &str, summary: Arc<Mutex<Summary>>) -> Session {
    let table = server_states_path().with_extension("registries.tsv");
    if let Ok(text) = std::fs::read_to_string(table) {
        horizon_protocol::translation::set_server_names(&text);
        let mut summary = summary.lock().unwrap();
        for line in text.lines() {
            let cols: Vec<_> = line.splitn(3, '\t').collect();
            if let [registry, id, name] = cols.as_slice()
                && *registry == "minecraft:block_entity_type"
                && let Ok(id) = id.parse()
            {
                summary.block_entities.insert(id, (*name).into());
            }
        }
    }
    Session::start(
        address,
        name,
        None,
        2,
        Arc::new(Language::new()),
        None,
        None,
        PathBuf::new(),
        Some(summary),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_include_failed_packets_and_distinguish_phases() {
        let summary = Mutex::new(Summary::default());
        received(
            Some(&summary),
            State::Play,
            play::Clientbound::KeepAlive(cfg::KeepAlive { id: 1 }).id(),
        );
        received(Some(&summary), State::Login, 0);
        let summary = summary.lock().unwrap();
        assert_eq!(summary.packets["play/keep_alive"], 1);
        assert_eq!(summary.packets["login/login_disconnect"], 1);
    }

    #[test]
    fn errors_group_by_packet_and_count_movement_failures() {
        let mut summary = Summary::default();
        summary.error("play/move_entity_pos", "short");
        summary.error("play/move_entity_pos", "invalid");
        summary.error("play/set_health", "short");
        assert_eq!(summary.decode_errors.len(), 2);
        assert_eq!(summary.decode_errors["play/move_entity_pos"].count, 2);
        assert_eq!(
            summary.decode_errors["play/move_entity_pos"].last_error,
            "invalid"
        );
        assert_eq!(summary.entity_movement_failed, 2);
        assert_eq!(summary.entity_movement_ok, 0);
    }

    #[test]
    fn successful_movement_does_not_count_unrelated_packets() {
        let mut summary = Summary::default();
        summary.play(&play::Clientbound::MoveEntityRot(play::MoveEntityRot(
            play::MoveEntity {
                id: 1,
                delta: None,
                rotation: Some((0, 0)),
                on_ground: true,
            },
        )));
        summary.play(&play::Clientbound::KeepAlive(cfg::KeepAlive { id: 1 }));
        assert_eq!(summary.entity_movement_ok, 1);
        assert_eq!(summary.entity_movement_failed, 0);
    }

    #[test]
    fn diagnostics_retain_bounded_latest_text() {
        let mut summary = Summary::default();
        summary.error("play/set_health", &"x".repeat(4096));
        assert_eq!(
            summary.decode_errors["play/set_health"].last_error.len(),
            2048
        );
    }

    #[test]
    fn disconnect_preserves_server_reason() {
        let mut summary = Summary::default();
        summary.kick("test kick".into());
        assert!(summary.kicked);
        assert_eq!(summary.reason.as_deref(), Some("test kick"));
    }

    #[test]
    fn kinetic_speed_accepts_zero_and_server_numbered_types() {
        let mut summary = Summary::default();
        summary.block_entities.insert(123, "create:shaft".into());
        let tag = Tag::Compound([("Speed".into(), Tag::Float(0.0_f32.to_bits()))].into());
        summary.kinetic([0, 64, 0], 123, &tag);
        summary.kinetic([0, 64, 0], 999, &tag);
        assert_eq!(summary.kinetic_speed_observations, 1);
        summary.block_entities.insert(123, "other:shaft".into());
        summary.kinetic([0, 64, 0], 123, &tag);
        assert_eq!(summary.kinetic_speed_observations, 1);
    }
}
