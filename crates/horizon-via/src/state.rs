//! Protocol connection states for `horizon-via`.
//!
//! Minecraft's state machine is almost stable across versions â€” the one real
//! wrinkle is the `Configuration` state, introduced in 1.20.2: on older
//! protocols the connection jumps straight from `Login` to `Play`. This
//! module centralizes that wrinkle so every step and the (future) pipeline
//! agree on one numbering.
//!
//! RU: ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ñ ÑÐ¾ÐµÐ´Ð¸Ð½ÐµÐ½Ð¸Ñ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð°. Ð•Ð´Ð¸Ð½ÑÑ‚Ð²ÐµÐ½Ð½Ð°Ñ Ð½Ð°ÑÑ‚Ð¾ÑÑ‰Ð°Ñ Ñ€Ð°Ð·Ð²Ð¸Ð»ÐºÐ° â€”
//! ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ðµ `Configuration` (Ð¿Ð¾ÑÐ²Ð¸Ð»Ð¾ÑÑŒ Ð² 1.20.2): Ð½Ð° ÑÑ‚Ð°Ñ€Ñ‹Ñ… Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð°Ñ… Ð¿Ð¾ÑÐ»Ðµ
//! `Login` ÑÑ€Ð°Ð·Ñƒ `Play`. ÐœÐ¾Ð´ÑƒÐ»ÑŒ Ð´ÐµÑ€Ð¶Ð¸Ñ‚ ÑÑ‚Ñƒ Ñ€Ð°Ð·Ð²Ð¸Ð»ÐºÑƒ Ð² Ð¾Ð´Ð½Ð¾Ð¼ Ð¼ÐµÑÑ‚Ðµ.

use crate::errors::ViaError;

/// Connection state, mirroring the server-side state machine.
///
/// Ids below are OUR internal canonical numbering (0 handshake, 1 status,
/// 2 login, 3 configuration, 4 play), shared with protocol_const::STATE_*.
/// The wire Next State byte only ever carries 1/2/3; see from_id_at for the
/// pre-1.20.2 mapping.
///
/// RU: ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ðµ ÑÐ¾ÐµÐ´Ð¸Ð½ÐµÐ½Ð¸Ñ. ÐÐ¾Ð¼ÐµÑ€Ð° â€” ÑÐ¾Ð²Ñ€ÐµÐ¼ÐµÐ½Ð½Ñ‹Ðµ (26.3): 0..4; Ð´Ð»Ñ ÑÑ‚Ð°Ñ€Ð¾Ð¹
/// Ð½ÑƒÐ¼ÐµÑ€Ð°Ñ†Ð¸Ð¸ ÐµÑÑ‚ÑŒ `from_id_at`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProtocolState {
    /// The very first packet: version + intent, then we switch.
    Handshake,
    /// Server list ping (legacy "multiplayer" screen).
    Status,
    /// Authentication and profile exchange.
    Login,
    /// Registry/plugin negotiation between login and play (1.20.2+).
    Configuration,
    /// The game proper.
    Play,
}

impl ProtocolState {
    /// Parse a state id using the modern (1.20.2+ / 26.3) numbering.
    ///
    /// Unknown ids become `UnsupportedPacket`-shaped errors rather than
    /// panics â€” a hostile server can send anything.
    ///
    /// RU: Ñ€Ð°Ð·Ð±Ð¾Ñ€ id ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ñ Ð¿Ð¾ ÑÐ¾Ð²Ñ€ÐµÐ¼ÐµÐ½Ð½Ð¾Ð¹ Ð½ÑƒÐ¼ÐµÑ€Ð°Ñ†Ð¸Ð¸; Ð½ÐµÐ¸Ð·Ð²ÐµÑÑ‚Ð½Ñ‹Ð¹ id â€”
    /// Ð¾ÑˆÐ¸Ð±ÐºÐ°, Ð½Ðµ Ð¿Ð°Ð½Ð¸ÐºÐ°.
    pub fn from_id(id: u8) -> Result<Self, ViaError> {
        Self::from_id_at(id, i32::MAX)
    }

    /// Parse a state id as it was numbered *at* the given protocol.
    ///
    /// Before 1.20.2 there is no `Configuration` state, and `3` means `Play`
    /// instead (the old numbering: 0 handshake, 1 status, 2 login, 3 play).
    /// At and after 1.20.2, `3` is `Configuration` and `4` is `Play`.
    ///
    /// RU: Ñ€Ð°Ð·Ð±Ð¾Ñ€ id Ð¿Ð¾ Ð½ÑƒÐ¼ÐµÑ€Ð°Ñ†Ð¸Ð¸ ÐºÐ¾Ð½ÐºÑ€ÐµÑ‚Ð½Ð¾Ð³Ð¾ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð°: Ð´Ð¾ 1.20.2 Ñ‚Ñ€Ð¾Ð¹ÐºÐ° â€”
    /// ÑÑ‚Ð¾ `Play`, Ð¿Ð¾ÑÐ»Ðµ â€” `Configuration`.
    pub fn from_id_at(id: u8, protocol: i32) -> Result<Self, ViaError> {
        if !Self::has_configuration(protocol) {
            match id {
                0 => Ok(ProtocolState::Handshake),
                1 => Ok(ProtocolState::Status),
                2 => Ok(ProtocolState::Login),
                3 => Ok(ProtocolState::Play),
                _ => Err(Self::bad_state_id(id, protocol)),
            }
        } else {
            match id {
                0 => Ok(ProtocolState::Handshake),
                1 => Ok(ProtocolState::Status),
                2 => Ok(ProtocolState::Login),
                3 => Ok(ProtocolState::Configuration),
                4 => Ok(ProtocolState::Play),
                _ => Err(Self::bad_state_id(id, protocol)),
            }
        }
    }

    /// Error constructor shared by both id parsers.
    fn bad_state_id(id: u8, protocol: i32) -> ViaError {
        ViaError::UnsupportedPacket {
            state: "handshake/switch",
            id: u32::from(id),
            version: protocol,
        }
    }

    /// Does this protocol have the `Configuration` state (1.20.2+)?
    ///
    /// Protocol 764 is 1.20.2, the release that introduced configuration.
    /// On older protocols, `Login` goes straight to `Play`.
    ///
    /// FIXME: verify exact boundary â€” snapshot protocols between 763 and 764
    /// (1.20.2 snapshots) may already carry the state; treat 764 as the cut
    /// until the version table in `versions` says otherwise.
    ///
    /// RU: ÐµÑÑ‚ÑŒ Ð»Ð¸ Ñƒ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ðµ `Configuration` (1.20.2+, 764+).
    /// FIXME: Ð¿Ñ€Ð¾Ð²ÐµÑ€Ð¸Ñ‚ÑŒ Ñ‚Ð¾Ñ‡Ð½ÑƒÑŽ Ð³Ñ€Ð°Ð½Ð¸Ñ†Ñƒ (ÑÐ½Ð°Ð¿ÑˆÐ¾Ñ‚Ñ‹ 1.20.2).
    pub fn has_configuration(protocol: i32) -> bool {
        // FIXME: verify exact boundary (1.20.2 snapshots vs release 764).
        protocol >= 764
    }

    /// The state the connection moves to after the current one, at the given
    /// protocol.
    ///
    /// Simplified per the crate contract: the handshake's fork into
    /// `Status` vs `Login` is decided by the *next packet's* intent field,
    /// not by the state itself, so this method assumes the login path.
    /// Terminal states map to themselves.
    ///
    /// RU: ÑÐ»ÐµÐ´ÑƒÑŽÑ‰ÐµÐµ ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ðµ (ÑƒÐ¿Ñ€Ð¾Ñ‰Ñ‘Ð½Ð½Ð¾: Ð¿Ð¾ÑÐ»Ðµ handshake â€” login; Ñ‚ÐµÑ€Ð¼Ð¸Ð½Ð°Ð»ÑŒÐ½Ñ‹Ðµ
    /// ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ñ Ð¿ÐµÑ€ÐµÑ…Ð¾Ð´ÑÑ‚ ÑÐ°Ð¼Ð¸ Ð² ÑÐµÐ±Ñ). Ð’ÐµÑ‚ÐºÑƒ handshakeâ†’status Ñ€ÐµÑˆÐ°ÐµÑ‚
    /// ÑÐ»ÐµÐ´ÑƒÑŽÑ‰Ð¸Ð¹ Ð¿Ð°ÐºÐµÑ‚, Ð½Ðµ ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ðµ.
    pub fn next(self, protocol: i32) -> Self {
        match self {
            ProtocolState::Handshake => ProtocolState::Login,
            ProtocolState::Login => {
                if Self::has_configuration(protocol) {
                    ProtocolState::Configuration
                } else {
                    ProtocolState::Play
                }
            }
            ProtocolState::Configuration => ProtocolState::Play,
            ProtocolState::Status | ProtocolState::Play => self,
        }
    }

    /// Lowercase state name as used in logs and error messages.
    ///
    /// RU: Ð¸Ð¼Ñ Ñ„Ð°Ð·Ñ‹ Ð´Ð»Ñ Ð»Ð¾Ð³Ð¾Ð² Ð¸ ÑÐ¾Ð¾Ð±Ñ‰ÐµÐ½Ð¸Ð¹ Ð¾Ð± Ð¾ÑˆÐ¸Ð±ÐºÐ°Ñ….
    pub fn phase_name(&self) -> &'static str {
        match self {
            ProtocolState::Handshake => "handshake",
            ProtocolState::Status => "status",
            ProtocolState::Login => "login",
            ProtocolState::Configuration => "configuration",
            ProtocolState::Play => "play",
        }
    }
}

impl std::fmt::Display for ProtocolState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.phase_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_id_all_modern_values() {
        assert_eq!(ProtocolState::from_id(0).unwrap(), ProtocolState::Handshake);
        assert_eq!(ProtocolState::from_id(1).unwrap(), ProtocolState::Status);
        assert_eq!(ProtocolState::from_id(2).unwrap(), ProtocolState::Login);
        assert_eq!(ProtocolState::from_id(3).unwrap(), ProtocolState::Configuration);
        assert_eq!(ProtocolState::from_id(4).unwrap(), ProtocolState::Play);
    }

    #[test]
    fn from_id_rejects_unknown() {
        for bad in [5u8, 100, 255] {
            let e = ProtocolState::from_id(bad).unwrap_err();
            match e {
                ViaError::UnsupportedPacket { state, id, .. } => {
                    assert_eq!(id, u32::from(bad));
                    assert_eq!(state, "handshake/switch");
                }
                other => panic!("expected UnsupportedPacket, got {}", other),
            }
        }
    }

    #[test]
    fn from_id_at_old_numbering_play_is_3() {
        // 754 = 1.16.5: no configuration state, id 3 is play, id 4 is invalid.
        assert_eq!(ProtocolState::from_id_at(3, 754).unwrap(), ProtocolState::Play);
        assert!(ProtocolState::from_id_at(4, 754).is_err());
    }

    #[test]
    fn from_id_at_new_numbering_configuration_is_3() {
        // 764 = 1.20.2+: id 3 is configuration, id 4 is play.
        assert_eq!(ProtocolState::from_id_at(3, 764).unwrap(), ProtocolState::Configuration);
        assert_eq!(ProtocolState::from_id_at(4, 764).unwrap(), ProtocolState::Play);
    }

    #[test]
    fn has_configuration_boundary() {
        assert!(!ProtocolState::has_configuration(763)); // 1.20.1
        assert!(ProtocolState::has_configuration(764)); // 1.20.2
        assert!(!ProtocolState::has_configuration(754)); // 1.16.5
        assert!(ProtocolState::has_configuration(999_999)); // placeholder 26.3
    }

    #[test]
    fn next_chain_old_protocol_754() {
        // No configuration: handshake -> login -> play -> play.
        let s = ProtocolState::Handshake;
        let s = s.next(754);
        assert_eq!(s, ProtocolState::Login);
        let s = s.next(754);
        assert_eq!(s, ProtocolState::Play);
        let s = s.next(754);
        assert_eq!(s, ProtocolState::Play, "play is terminal");
    }

    #[test]
    fn next_chain_placeholder_263() {
        // With configuration (protocol 999999 is a 26.3 placeholder):
        // handshake -> login -> configuration -> play -> play.
        let s = ProtocolState::Handshake;
        let s = s.next(999_999);
        assert_eq!(s, ProtocolState::Login);
        let s = s.next(999_999);
        assert_eq!(s, ProtocolState::Configuration);
        let s = s.next(999_999);
        assert_eq!(s, ProtocolState::Play);
        let s = s.next(999_999);
        assert_eq!(s, ProtocolState::Play, "play is terminal");
    }

    #[test]
    fn next_status_is_terminal() {
        assert_eq!(
            ProtocolState::Status.next(999_999),
            ProtocolState::Status
        );
        assert_eq!(ProtocolState::Status.next(754), ProtocolState::Status);
    }

    #[test]
    fn next_handshake_always_login_by_default() {
        // The status fork is decided by the next packet's intent, not here.
        assert_eq!(ProtocolState::Handshake.next(754), ProtocolState::Login);
        assert_eq!(
            ProtocolState::Handshake.next(999_999),
            ProtocolState::Login
        );
    }

    #[test]
    fn phase_names() {
        assert_eq!(ProtocolState::Handshake.phase_name(), "handshake");
        assert_eq!(ProtocolState::Status.phase_name(), "status");
        assert_eq!(ProtocolState::Login.phase_name(), "login");
        assert_eq!(ProtocolState::Configuration.phase_name(), "configuration");
        assert_eq!(ProtocolState::Play.phase_name(), "play");
    }

    #[test]
    fn display_uses_phase_name() {
        assert_eq!(ProtocolState::Play.to_string(), "play");
        assert_eq!(ProtocolState::Configuration.to_string(), "configuration");
    }
}
