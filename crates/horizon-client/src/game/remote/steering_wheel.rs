//! Simulated 1.3.2 wheel input. The hit position is already in server space,
//! including for a wheel on a Sable plot.
use super::{Game, Remote, S, Screen, Tag};
use horizon_client::net::STEERING_WHEEL_CHANNEL;
use horizon_core::geometry::Direction;
use horizon_protocol::{
    buf::{Writer, pack_block_pos},
    packets::configuration::CustomPayload,
};
use winit::keyboard::KeyCode;

pub(super) const BLOCK: &str = "simulated:steering_wheel";

#[derive(Default)]
pub(super) struct Input {
    pub(super) clicked: bool,
    pub(super) drag: Option<Drag>,
}

#[derive(Clone, Copy)]
pub(super) struct Drag {
    pos: [i32; 3],
    facing: Direction,
    // Keep sub-snap motion so a series of small deltas crosses a detent.
    angle: f32,
    target: f32,
}

impl Drag {
    fn packet(self, stop: bool) -> CustomPayload {
        let mut wire = Writer::new();
        wire.bool(stop);
        wire.f32(self.target);
        wire.i64(pack_block_pos(self.pos));
        CustomPayload {
            channel: STEERING_WHEEL_CHANNEL.into(),
            data: wire.data,
        }
    }

    fn turn(&mut self, dx: f64, sneaking: bool) -> Option<CustomPayload> {
        let sign: i32 = self.facing.normal().into_iter().sum();
        let angle = self.angle + (dx / 10.0) as f32 * sign as f32;
        if !angle.is_finite() {
            return None;
        }
        self.angle = angle;
        let target = if sneaking {
            (angle / 45.0).round() * 45.0
        } else {
            angle
        };
        if self.target == target {
            return None;
        }
        self.target = target;
        Some(self.packet(false))
    }
}

impl Input {
    fn begin(&mut self, pos: [i32; 3], facing: Direction, angle: f32) -> CustomPayload {
        let drag = Drag { pos, facing, angle, target: angle };
        self.clicked = false;
        self.drag = Some(drag);
        drag.packet(false)
    }

    fn stop(&mut self) -> Option<CustomPayload> {
        Some(self.drag.take()?.packet(true))
    }

    fn motion(&mut self, hit: Option<([i32; 3], Direction)>, dx: f64, sneak: bool)
        -> Option<CustomPayload> {
        let drag = self.drag.as_mut()?;
        if hit != Some((drag.pos, drag.facing)) {
            return self.stop();
        }
        drag.turn(dx, sneak)
    }
}

fn wheel_facing(enabled: bool, block: &str, facing: Option<&str>) -> Option<Direction> {
    if !enabled || block != BLOCK {
        return None;
    }
    crate::game::machines::dir_from_name(facing?)
}

fn observed_held(tag: &Tag) -> Option<bool> {
    match tag.get("Held")? {
        Tag::Byte(flag) => Some(*flag != 0),
        _ => None,
    }
}

impl Remote {
    pub(in crate::game) fn observe_wheel(&mut self, pos: [i32; 3], kind: i32, tag: &Tag) {
        if self.wheel_enabled
            && self.builtin.get("minecraft:block_entity_type").and_then(|ids| ids.name(kind)) == Some(BLOCK)
            && let Some(held) = observed_held(tag)
        {
            self.wheel_server = Some((pos, held));
        }
    }
}

impl Game<'_> {
    pub(super) fn steering_wheel_at(&self, pos: [i32; 3]) -> Option<Direction> {
        let r = self.remote.as_ref()?;
        if !r.wheel_enabled {
            return None;
        }
        let state = self.pack.assets.registry.state(self.target_block(pos))?;
        wheel_facing(r.wheel_enabled, &state.name, state.properties.get("facing").map(String::as_str))
    }

    pub(super) fn start_steering_wheel(&mut self, pos: [i32; 3], facing: Direction) {
        let saved;
        let tag = if let Some(level) = self.plot_level(pos) {
            level.block_entity(pos).map(|(_, tag)| tag)
        } else {
            saved = crate::game::spill::saved_entity(&self.world, pos);
            saved.as_ref()
        };
        let angle = match tag.and_then(|tag| tag.get("TargetAngle")) {
            Some(Tag::Float(bits)) if f32::from_bits(*bits).is_finite() => f32::from_bits(*bits),
            _ => 0.0,
        };
        let packet = self.remote.as_mut().unwrap().wheel.begin(pos, facing, angle);
        self.send(S::CustomPayload(packet));
        if !self.right {
            // Both button edges may have arrived before this tick.
            self.stop_steering_wheel();
        }
    }

    pub(super) fn stop_steering_wheel(&mut self) {
        if let Some(packet) = self.remote.as_mut().and_then(|r| r.wheel.stop()) {
            self.send(S::CustomPayload(packet));
        }
    }

    /// Both grabbed pointer input and scripted raw motion arrive here before
    /// sensitivity, inversion and camera rotation.
    pub(in crate::game) fn steering_wheel_motion(&mut self, dx: f64) -> bool {
        if self.remote.as_ref().and_then(|r| r.wheel.drag).is_none() {
            return false;
        }
        let hit = if self.right && self.screen == Screen::Playing && self.sleeping.is_none()
            && self.held().is_none() && !self.spectator() && !self.entity_targeted()
        {
            self.pick().and_then(|h| Some((h.pos, self.steering_wheel_at(h.pos)?)))
        } else {
            None
        };
        let sneaking = self.keys.contains(&KeyCode::ShiftLeft);
        let wheel = &mut self.remote.as_mut().unwrap().wheel;
        let packet = wheel.motion(hit, dx, sneaking);
        let grabbed = wheel.drag.is_some();
        if let Some(packet) = packet {
            self.send(S::CustomPayload(packet));
        }
        grabbed
    }

    pub(in crate::game) fn steering_wheel_server(&self) -> serde_json::Value {
        self.remote.as_ref().and_then(|r| r.wheel_server)
            .map_or(serde_json::Value::Null, |(pos, held)| serde_json::json!({"pos": pos, "held": held}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::buf::{Reader, unpack_block_pos};

    const POS: [i32; 3] = [20_482_064, -23, 20_484_128];

    fn input() -> Input {
        let mut input = Input { clicked: true, drag: None };
        check(input.begin(POS, Direction::North, 0.0), false, 0.0);
        assert!(!input.clicked);
        input
    }

    fn check(packet: CustomPayload, stop: bool, angle: f32) {
        assert_eq!(packet.channel, "simulated:steering_wheel_update");
        let mut wire = Reader::new(&packet.data);
        assert_eq!(wire.u8().unwrap(), u8::from(stop));
        assert_eq!(wire.f32().unwrap(), angle);
        assert_eq!(unpack_block_pos(wire.i64().unwrap()), POS);
        assert_eq!(wire.remaining(), 0);
    }

    #[test]
    fn steering_wheel_raw_motion_and_button_release() {
        let mut input = input();
        let hit = Some((POS, Direction::North));
        check(input.motion(hit, 56.0, false).unwrap(), false, -5.6);
        check(input.motion(hit, 100.0, true).unwrap(), false, 0.0);
        check(input.motion(hit, 100.0, true).unwrap(), false, -45.0);
        check(input.stop().unwrap(), true, -45.0);
        assert!(input.stop().is_none());
        assert!(input.motion(hit, 20.0, false).is_none());
    }

    #[test]
    fn steering_wheel_lost_targets_release_at_original_address() {
        for hit in [None, Some(([0; 3], Direction::North)), Some((POS, Direction::Up))] {
            let mut input = input();
            check(input.motion(hit, 90.0, false).unwrap(), true, 0.0);
            assert!(input.drag.is_none());
        }
    }

    #[test]
    fn steering_wheel_normal_connections_and_other_blocks_pass() {
        assert_eq!(wheel_facing(false, BLOCK, Some("east")), None);
        assert_eq!(wheel_facing(true, "minecraft:stone", Some("east")), None);
        assert_eq!(wheel_facing(true, BLOCK, Some("east")), Some(Direction::East));
    }

    #[test]
    fn steering_wheel_server_reads_only_explicit_held() {
        assert_eq!(observed_held(&Tag::compound([("InUse", Tag::Byte(1))])), None);
        for held in [false, true] {
            assert_eq!(observed_held(&Tag::compound([("Held", Tag::Byte(i8::from(held)))])), Some(held));
        }
    }
}
