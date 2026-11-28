//! Autocannon recoil-spring animation of CBC 5.11.7. Big cannon firing visuals
//! are the `big_cannon_plume` particle; a mounted big cannon never moves.

use super::packet;
use horizon_protocol::buf::Reader;
use horizon_world::nbt::Tag;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CannonKind {
    Big,
    Auto,
}

pub(super) fn animation_entity(data: &[u8]) -> Option<i32> {
    let mut wire = Reader::new(data);
    if wire.varint().ok()? != 2 {
        return None;
    }
    let id = wire.varint().ok()?;
    (id >= 0 && wire.remaining() == 0).then_some(id)
}

// Moving states are relative to the recoil spring, not the entity.
pub(super) fn spring_blocks(data: &Tag) -> impl Iterator<Item = ([i32; 3], &Tag)> {
    let list = match data.get("RenderedBlocks") {
        Some(Tag::List(list)) => list.as_slice(),
        _ => &[],
    };
    list.iter()
        .filter_map(|entry| Some((packet::decode_pos(entry.get("Pos")?)?, entry.get("Block")?)))
}

/// `AutocannonRecoilSpringBlockEntity.animateTicks` after
/// `ClientboundAnimateCannonContraptionPacket` set it to zero.
#[derive(Clone, Copy, Default)]
pub(super) struct Recoil {
    ticks: u8,
}

impl Recoil {
    pub(super) fn tick(&mut self) -> bool {
        self.ticks += 1;
        self.ticks < 5
    }

    /// `AutocannonRecoilSpringBlockEntity.getAnimateOffset`: the ejector
    /// scale, 1 at rest and 0 at the deepest point of the stroke.
    pub(super) fn scale(self, alpha: f64) -> f32 {
        let t = (f32::from(self.ticks) + alpha.clamp(0.0, 1.0) as f32) * 1.2;
        if t <= 0.0 || t >= 4.8 {
            return 1.0;
        }
        let f = if t < 1.0 { t } else { (4.8 - t) / 3.8 };
        (f * std::f32::consts::FRAC_PI_2).cos()
    }

    /// `AutocannonRecoilSpringRenderer`: the moving blocks slide this far
    /// back against the spring's facing.
    pub(super) fn offset(self, alpha: f64) -> f32 {
        (1.0 - self.scale(alpha)) * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_accepts_the_autocannon_variant_only() {
        assert_eq!(animation_entity(&[2, 0xAC, 2]), Some(300));
        assert_eq!(animation_entity(&[2, 8, 0]), None);
        assert_eq!(animation_entity(&[3, 8]), None);
        assert_eq!(animation_entity(&[2]), None);
    }

    #[test]
    fn the_recoil_follows_the_spring_block_entity_curve() {
        let mut shot = Recoil::default();
        assert_eq!(shot.scale(0.0), 1.0);
        assert_eq!(shot.offset(0.0), 0.0);
        // t = 1.2 * (0 + 0.8) = 0.96 on the rising edge.
        let rising = (0.96f32 * std::f32::consts::FRAC_PI_2).cos();
        assert!((shot.scale(0.8) - rising).abs() < 1e-6);
        assert!((shot.offset(0.8) - (1.0 - rising) * 0.5).abs() < 1e-6);
        // t = 1.2 * 1.5 = 1.8 on the falling edge: (4.8 - 1.8) / 3.8.
        assert!(shot.tick());
        let falling = ((3.0f32 / 3.8) * std::f32::consts::FRAC_PI_2).cos();
        assert!((shot.scale(0.5) - falling).abs() < 1e-6);
        let mut deepest = 0.0f32;
        for step in 0..=50 {
            deepest = deepest.max(shot.offset(f64::from(step) / 50.0));
        }
        assert!(deepest > 0.45 && deepest <= 0.5);
        for _ in 0..3 {
            assert!(shot.tick());
        }
        assert!(!shot.tick());
        assert_eq!(shot.offset(0.0), 0.0);
    }
}
