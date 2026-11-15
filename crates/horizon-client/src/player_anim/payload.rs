//! Decoding of the custom payloads that start player animations.
//!
//! The layouts are the data bytes after the channel identifier: fixed-width
//! big-endian integers and floats, VarInt-prefixed UTF-8 strings, UUIDs as two
//! big-endian halves, enums as VarInt ordinals.

use horizon_protocol::buf::{Reader, Writer};

pub const IRONS_UPDATE_CASTING_STATE: &str = "irons_spellbooks:update_casting_state";
pub const IRONS_CAST_STARTED: &str = "irons_spellbooks:on_cast_started";
pub const IRONS_CAST_FINISHED: &str = "irons_spellbooks:on_cast_finished";
pub const IRONS_PLAY_PLAYER_ANIMATION: &str = "irons_spellbooks:play_player_animation";
pub const IRONS_SYNC_PLAYER_DATA: &str = "irons_spellbooks:sync_player_data";
pub const BETTER_COMBAT_ATTACK: &str = "bettercombat:attack_animation";
pub const BETTER_COMBAT_CONFIG: &str = "bettercombat:config_sync";
pub const BETTER_COMBAT_REGISTRY: &str = "bettercombat:weapon_registry";

/// `Packets.AttackAnimation.STOP`: the animation name that stops the attack.
pub const BETTER_COMBAT_STOP: &str = "!STOP!";

const MAX_ID: usize = 32767;
const MAX_JSON: usize = 1 << 20;
const MAX_REGISTRY_CHUNKS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimatedHand {
    MainHand,
    OffHand,
    TwoHanded,
}

impl AnimatedHand {
    fn from_ordinal(ordinal: i32) -> Option<Self> {
        match ordinal {
            0 => Some(Self::MainHand),
            1 => Some(Self::OffHand),
            2 => Some(Self::TwoHanded),
            _ => None,
        }
    }

    fn ordinal(self) -> i32 {
        match self {
            Self::MainHand => 0,
            Self::OffHand => 1,
            Self::TwoHanded => 2,
        }
    }
}

/// `bettercombat:attack_animation`, in both directions.
#[derive(Clone, Debug, PartialEq)]
pub struct BetterCombatAttack {
    pub player_id: i32,
    pub hand: AnimatedHand,
    pub animation: String,
    /// Ticks the attack lasts; the fade length in a stop packet.
    pub length: f32,
    /// Fraction of `length` before the hit.
    pub upswing: f32,
    pub weapon_range: f32,
    pub upswing_ticks: i32,
    pub particles_json: String,
}

impl BetterCombatAttack {
    pub fn is_stop(&self) -> bool {
        self.animation == BETTER_COMBAT_STOP
    }

    /// The data bytes `Packets.AttackAnimation.write` produces.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.i32(self.player_id);
        w.i32(self.hand.ordinal());
        w.string(&self.animation);
        w.f32(self.length);
        w.f32(self.upswing);
        w.f32(self.weapon_range);
        w.i32(self.upswing_ticks);
        w.string(&self.particles_json);
        w.data
    }
}

/// What a player-animation payload tells the client.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationNetEvent {
    /// To the casting player only, before `IronsCastStarted`.
    IronsUpdateCastingState {
        spell_id: String,
        spell_level: i32,
        cast_time: i32,
        cast_source: i32,
        equipment_slot: String,
    },
    IronsCastStarted {
        spell_id: String,
        spell_level: i32,
        caster: u128,
    },
    IronsCastFinished {
        spell_id: String,
        caster: u128,
        cancelled: bool,
    },
    IronsPlayAnimation {
        player: u128,
        animation: String,
    },
    /// The leading fields of `SyncedSpellData`; the learned-spell tail is not read.
    IronsSyncPlayerData {
        entity_id: i32,
        is_casting: bool,
        casting_spell_id: String,
        casting_spell_level: i32,
        casting_equipment_slot: String,
    },
    BetterCombatAttack(BetterCombatAttack),
}

/// Whether `channel` carries a payload [`decode`] understands.
pub fn is_animation_channel(channel: &str) -> bool {
    matches!(
        channel,
        IRONS_UPDATE_CASTING_STATE
            | IRONS_CAST_STARTED
            | IRONS_CAST_FINISHED
            | IRONS_PLAY_PLAYER_ANIMATION
            | IRONS_SYNC_PLAYER_DATA
            | BETTER_COMBAT_ATTACK
    )
}

fn finite(value: f32, what: &str) -> Result<f32, String> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{what} is not a finite number"))
    }
}

/// Decodes the payload of an animation channel; `Ok(None)` for other channels.
pub fn decode(channel: &str, data: &[u8]) -> Result<Option<AnimationNetEvent>, String> {
    let mut r = Reader::new(data);
    let err = |e: horizon_protocol::Error| format!("{channel}: {e}");
    let event = match channel {
        IRONS_UPDATE_CASTING_STATE => AnimationNetEvent::IronsUpdateCastingState {
            spell_id: r.string(MAX_ID).map_err(err)?,
            spell_level: r.i32().map_err(err)?,
            cast_time: r.i32().map_err(err)?,
            cast_source: r.varint().map_err(err)?,
            equipment_slot: r.string(MAX_ID).map_err(err)?,
        },
        IRONS_CAST_STARTED => AnimationNetEvent::IronsCastStarted {
            spell_id: r.string(MAX_ID).map_err(err)?,
            spell_level: r.i32().map_err(err)?,
            caster: r.uuid().map_err(err)?,
        },
        IRONS_CAST_FINISHED => AnimationNetEvent::IronsCastFinished {
            spell_id: r.string(MAX_ID).map_err(err)?,
            caster: r.uuid().map_err(err)?,
            cancelled: r.bool().map_err(err)?,
        },
        IRONS_PLAY_PLAYER_ANIMATION => AnimationNetEvent::IronsPlayAnimation {
            player: r.uuid().map_err(err)?,
            animation: r.string(MAX_ID).map_err(err)?,
        },
        IRONS_SYNC_PLAYER_DATA => {
            let entity_id = r.i32().map_err(err)?;
            let is_casting = r.bool().map_err(err)?;
            let casting_spell_id = r.string(MAX_ID).map_err(err)?;
            let casting_spell_level = r.i32().map_err(err)?;
            finite(r.f32().map_err(err)?, "heartStopAccumulatedDamage")?;
            r.i32().map_err(err)?;
            r.string(MAX_ID).map_err(err)?;
            r.bool().map_err(err)?;
            AnimationNetEvent::IronsSyncPlayerData {
                entity_id,
                is_casting,
                casting_spell_id,
                casting_spell_level,
                casting_equipment_slot: r.string(MAX_ID).map_err(err)?,
            }
        }
        BETTER_COMBAT_ATTACK => {
            let player_id = r.i32().map_err(err)?;
            let ordinal = r.i32().map_err(err)?;
            let hand = AnimatedHand::from_ordinal(ordinal)
                .ok_or_else(|| format!("{channel}: hand ordinal {ordinal}"))?;
            AnimationNetEvent::BetterCombatAttack(BetterCombatAttack {
                player_id,
                hand,
                animation: r.string(MAX_ID).map_err(err)?,
                length: finite(r.f32().map_err(err)?, "length")?,
                upswing: finite(r.f32().map_err(err)?, "upswing")?,
                weapon_range: finite(r.f32().map_err(err)?, "weaponRange")?,
                upswing_ticks: r.i32().map_err(err)?,
                particles_json: r.string(MAX_JSON).map_err(err)?,
            })
        }
        _ => return Ok(None),
    };
    Ok(Some(event))
}

/// `bettercombat:config_sync` and `bettercombat:weapon_registry`, kept for
/// local animation selection and timing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BetterCombatConfig {
    pub server_config_json: Option<String>,
    pub registry_compressed: bool,
    pub registry_chunks: Vec<String>,
}

impl BetterCombatConfig {
    /// The server's `upswing_multiplier`, when the config carries it.
    pub fn upswing_multiplier(&self) -> Option<f32> {
        let json = self.server_config_json.as_deref()?;
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        value.get("upswing_multiplier")?.as_f64().map(|v| v as f32)
    }

    /// Folds one configuration payload in; other channels change nothing.
    pub fn absorb(&mut self, channel: &str, data: &[u8]) -> Result<(), String> {
        let mut r = Reader::new(data);
        let err = |e: horizon_protocol::Error| format!("{channel}: {e}");
        match channel {
            BETTER_COMBAT_CONFIG => {
                self.server_config_json = Some(r.string(MAX_JSON).map_err(err)?);
            }
            BETTER_COMBAT_REGISTRY => {
                let compressed = r.bool().map_err(err)?;
                let count = r.i32().map_err(err)?;
                let count = usize::try_from(count)
                    .ok()
                    .filter(|n| *n <= MAX_REGISTRY_CHUNKS)
                    .ok_or_else(|| format!("{channel}: {count} chunks"))?;
                let mut chunks = Vec::with_capacity(count);
                for _ in 0..count {
                    chunks.push(r.string(MAX_JSON).map_err(err)?);
                }
                self.registry_compressed = compressed;
                self.registry_chunks = chunks;
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: u128 = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210;

    fn utf(w: &mut Writer, s: &str) {
        w.string(s);
    }

    #[test]
    fn decodes_cast_started_with_uuid_order() {
        let mut w = Writer::new();
        utf(&mut w, "irons_spellbooks:fireball");
        w.i32(3);
        w.uuid(UUID);
        // The UUID travels most significant half first.
        let bytes = &w.data[w.data.len() - 16..];
        assert_eq!(bytes[0], 0x01);
        assert_eq!(bytes[15], 0x10);
        let event = decode(IRONS_CAST_STARTED, &w.data).unwrap().unwrap();
        assert_eq!(
            event,
            AnimationNetEvent::IronsCastStarted {
                spell_id: "irons_spellbooks:fireball".into(),
                spell_level: 3,
                caster: UUID,
            }
        );
    }

    #[test]
    fn decodes_cast_finished_cancellation() {
        let mut w = Writer::new();
        utf(&mut w, "irons_spellbooks:fireball");
        w.uuid(UUID);
        w.bool(true);
        let event = decode(IRONS_CAST_FINISHED, &w.data).unwrap().unwrap();
        assert_eq!(
            event,
            AnimationNetEvent::IronsCastFinished {
                spell_id: "irons_spellbooks:fireball".into(),
                caster: UUID,
                cancelled: true,
            }
        );
    }

    #[test]
    fn decodes_update_casting_state_with_enum_ordinal() {
        let mut w = Writer::new();
        utf(&mut w, "irons_spellbooks:lightning_lance");
        w.i32(2);
        w.i32(40);
        w.varint(1);
        utf(&mut w, "offhand");
        let event = decode(IRONS_UPDATE_CASTING_STATE, &w.data).unwrap().unwrap();
        assert_eq!(
            event,
            AnimationNetEvent::IronsUpdateCastingState {
                spell_id: "irons_spellbooks:lightning_lance".into(),
                spell_level: 2,
                cast_time: 40,
                cast_source: 1,
                equipment_slot: "offhand".into(),
            }
        );
    }

    #[test]
    fn decodes_the_debug_play_command() {
        let mut w = Writer::new();
        w.uuid(UUID);
        utf(&mut w, "irons_spellbooks:charged_throw");
        let event = decode(IRONS_PLAY_PLAYER_ANIMATION, &w.data).unwrap().unwrap();
        assert_eq!(
            event,
            AnimationNetEvent::IronsPlayAnimation {
                player: UUID,
                animation: "irons_spellbooks:charged_throw".into(),
            }
        );
    }

    #[test]
    fn decodes_the_synced_spell_data_prefix() {
        let mut w = Writer::new();
        w.i32(77);
        w.bool(true);
        utf(&mut w, "irons_spellbooks:fireball");
        w.i32(5);
        w.f32(0.0);
        w.i32(0);
        utf(&mut w, "");
        w.bool(false);
        utf(&mut w, "offhand");
        // The learned-spell tail follows and is ignored.
        w.i32(1);
        utf(&mut w, "irons_spellbooks:fireball");
        let event = decode(IRONS_SYNC_PLAYER_DATA, &w.data).unwrap().unwrap();
        assert_eq!(
            event,
            AnimationNetEvent::IronsSyncPlayerData {
                entity_id: 77,
                is_casting: true,
                casting_spell_id: "irons_spellbooks:fireball".into(),
                casting_spell_level: 5,
                casting_equipment_slot: "offhand".into(),
            }
        );
    }

    fn attack(animation: &str) -> BetterCombatAttack {
        BetterCombatAttack {
            player_id: 1234,
            hand: AnimatedHand::OffHand,
            animation: animation.into(),
            length: 12.5,
            upswing: 0.4,
            weapon_range: 3.0,
            upswing_ticks: 5,
            particles_json: "{\"particles\":[]}".into(),
        }
    }

    #[test]
    fn better_combat_attack_round_trips_and_keeps_alignment() {
        let original = attack("bettercombat:one_handed_slash_horizontal_right");
        let bytes = original.encode();
        // playerId, then the hand ordinal, both four-byte big-endian.
        assert_eq!(&bytes[..8], &[0, 0, 4, 210, 0, 0, 0, 1]);
        let event = decode(BETTER_COMBAT_ATTACK, &bytes).unwrap().unwrap();
        assert_eq!(event, AnimationNetEvent::BetterCombatAttack(original));
    }

    #[test]
    fn better_combat_stop_sentinel() {
        let bytes = attack(BETTER_COMBAT_STOP).encode();
        let Some(AnimationNetEvent::BetterCombatAttack(stop)) =
            decode(BETTER_COMBAT_ATTACK, &bytes).unwrap()
        else {
            panic!("not an attack");
        };
        assert!(stop.is_stop());
        assert_eq!(stop.length, 12.5);
    }

    #[test]
    fn rejects_truncated_and_invalid_payloads() {
        let bytes = attack("x").encode();
        assert!(decode(BETTER_COMBAT_ATTACK, &bytes[..bytes.len() - 3]).is_err());
        let mut bad_hand = bytes.clone();
        bad_hand[7] = 9;
        assert!(decode(BETTER_COMBAT_ATTACK, &bad_hand).is_err());
        let mut nan = attack("x");
        nan.length = f32::NAN;
        assert!(decode(BETTER_COMBAT_ATTACK, &nan.encode()).is_err());
        assert!(decode("minecraft:brand", &[]).unwrap().is_none());
        assert!(decode(IRONS_CAST_STARTED, &[]).is_err());
    }

    #[test]
    fn retains_better_combat_configuration() {
        let mut config = BetterCombatConfig::default();
        let mut w = Writer::new();
        utf(&mut w, r#"{"upswing_multiplier":0.5,"other":1}"#);
        config.absorb(BETTER_COMBAT_CONFIG, &w.data).unwrap();
        assert_eq!(config.upswing_multiplier(), Some(0.5));

        let mut w = Writer::new();
        w.bool(true);
        w.i32(2);
        utf(&mut w, "abc");
        utf(&mut w, "def");
        config.absorb(BETTER_COMBAT_REGISTRY, &w.data).unwrap();
        assert!(config.registry_compressed);
        assert_eq!(config.registry_chunks, vec!["abc", "def"]);

        let mut w = Writer::new();
        w.bool(false);
        w.i32(-1);
        assert!(config.absorb(BETTER_COMBAT_REGISTRY, &w.data).is_err());
    }
}
