//! Minecraft Java protocol version table used by every translation step.
//!
//! EN: this module owns the mapping `protocol number <-> version name` for
//! every release the translation chain can walk through, from 1.8 (protocol
//! 47) up to the newest version known to this crate. Step modules and the
//! future pipeline ask *this* table (and only this table) which protocols
//! exist and in which order — never their own private copies.
//!
//! RU: таблица «номер протокола <-> имя версии» для всей цепочки трансляции,
//! от 1.8 (протокол 47) до последней известной версии. Шаги и пайплайн
//! спрашивают порядок протоколов только здесь, частных копий быть не должно.
//!
//! ## Data honesty
//!
//! Numbers below 1.21.5 (protocol 770) are stable public knowledge and match
//! ViaVersion's version registry. Anything above 1.21.10, including the whole
//! 26.x line the Horizon client speaks, is **not** pinned here: the target
//! protocol is a placeholder constant with a FIXME for the orchestrator.
//!
//! RU: номера до 1.21.5 стабильны и совпадают с реестром ViaVersion.
//! Всё выше 1.21.10 и вся линейка 26.x здесь сознательно не закреплены —
//! цель трансляции это placeholder с FIXME для оркестратора.

use crate::protocol_const::VIA_TARGET;

/// Placeholder protocol number for the Horizon client's own version (26.3).
///
/// EN: the real 26.3 protocol number is not known to this crate yet. Until
/// the orchestrator pins the exact value from `horizon-protocol`, every
/// chain that needs "the top of the ladder" uses this sentinel. It is
/// intentionally absurd (`999_999`) so a mistake is loud, not silent.
///
/// FIXME(orchestrator): replace with the real 26.3 protocol number
/// taken from `horizon-protocol`; then delete this doc paragraph.
///
/// RU: заглушка номера протокола клиента Horizon (26.3). Реальный номер
/// закрепит оркестратор из horizon-protocol; значение специально абсурдное,
/// чтобы ошибка была громкой.
// TODO(orchestrator): pin exact numbers from horizon-protocol
pub const VIA_TARGET_PROTOCOL: i32 = VIA_TARGET;

/// One entry of the canonical protocol table.
///
/// EN: a protocol number plus the *display name* of the newest patch that
/// uses it (e.g. 754 -> "1.16.5"). Patch versions that share a protocol
/// number share one entry, because they share one wire format.
///
/// RU: номер протокола и имя последнего патча на нём (754 -> "1.16.5").
/// Патчи с общим номером протокола имеют одинаковый формат на проводе —
/// и одну запись здесь.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VersionEntry {
    /// Wire protocol number as sent in the handshake.
    /// RU: номер протокола, как в handshake.
    pub protocol: i32,
    /// Display name of the newest patch on this protocol.
    /// RU: отображаемое имя последнего патча этого протокола.
    pub name: &'static str,
}

/// Canonical protocol table, sorted ascending by protocol number.
///
/// EN: every entry is one *distinct wire format*. Walking this table upward
/// is exactly the ladder the translation chain climbs. The table is `pub`
/// so other modules can iterate it, but they must not mutate or duplicate it.
///
/// Sorted order is verified by a unit test; do not break it.
///
/// RU: каноническая таблица, по возрастанию номера. Каждая запись — отдельный
/// формат на проводе; подъём по таблице и есть цепочка трансляции. Порядок
/// проверяется тестом — не ломать.
pub const VERSIONS: &[VersionEntry] = &[
    VersionEntry { protocol: 47, name: "1.8.9" },    // 1.8 - 1.8.9
    VersionEntry { protocol: 107, name: "1.9" },
    VersionEntry { protocol: 108, name: "1.9.1" },
    VersionEntry { protocol: 109, name: "1.9.2" },
    VersionEntry { protocol: 110, name: "1.9.4" },   // 1.9.3, 1.9.4
    VersionEntry { protocol: 210, name: "1.10.2" },  // 1.10 - 1.10.2
    VersionEntry { protocol: 315, name: "1.11" },
    VersionEntry { protocol: 316, name: "1.11.2" },  // 1.11.1, 1.11.2
    VersionEntry { protocol: 335, name: "1.12" },
    VersionEntry { protocol: 338, name: "1.12.1" },
    VersionEntry { protocol: 340, name: "1.12.2" },
    VersionEntry { protocol: 393, name: "1.13" },
    VersionEntry { protocol: 401, name: "1.13.1" },
    VersionEntry { protocol: 404, name: "1.13.2" },
    VersionEntry { protocol: 477, name: "1.14" },
    VersionEntry { protocol: 480, name: "1.14.1" },
    VersionEntry { protocol: 485, name: "1.14.2" },
    VersionEntry { protocol: 490, name: "1.14.3" },
    VersionEntry { protocol: 498, name: "1.14.4" },
    VersionEntry { protocol: 573, name: "1.15" },
    VersionEntry { protocol: 575, name: "1.15.1" },
    VersionEntry { protocol: 578, name: "1.15.2" },
    VersionEntry { protocol: 735, name: "1.16" },
    VersionEntry { protocol: 736, name: "1.16.1" },
    VersionEntry { protocol: 751, name: "1.16.3" },  // 1.16.2, 1.16.3
    VersionEntry { protocol: 754, name: "1.16.5" },  // 1.16.4, 1.16.5
    VersionEntry { protocol: 755, name: "1.17" },
    VersionEntry { protocol: 756, name: "1.17.1" },
    VersionEntry { protocol: 757, name: "1.18.1" },  // 1.18, 1.18.1
    VersionEntry { protocol: 758, name: "1.18.2" },
    VersionEntry { protocol: 759, name: "1.19" },
    VersionEntry { protocol: 760, name: "1.19.2" },  // 1.19.1, 1.19.2
    VersionEntry { protocol: 761, name: "1.19.3" },
    VersionEntry { protocol: 762, name: "1.19.4" },
    VersionEntry { protocol: 763, name: "1.20.1" },  // 1.20, 1.20.1
    VersionEntry { protocol: 764, name: "1.20.2" },
    VersionEntry { protocol: 765, name: "1.20.4" },  // 1.20.3, 1.20.4
    VersionEntry { protocol: 766, name: "1.20.6" },  // 1.20.5, 1.20.6
    VersionEntry { protocol: 767, name: "1.21.1" },  // 1.21, 1.21.1
    VersionEntry { protocol: 768, name: "1.21.3" },  // 1.21.2, 1.21.3
    VersionEntry { protocol: 769, name: "1.21.4" },
    VersionEntry { protocol: 770, name: "1.21.5" },
    VersionEntry { protocol: 771, name: "1.21.6" },
    VersionEntry { protocol: 772, name: "1.21.8" },  // 1.21.7, 1.21.8
    VersionEntry { protocol: 773, name: "1.21.9" },
    VersionEntry { protocol: 774, name: "1.21.10" },
    // 1.21.11 and the whole 26.x line are deliberately absent: numbers not
    // verified from horizon-protocol. See VIA_TARGET_PROTOCOL.
    // RU: 1.21.11 и 26.x сознательно отсутствуют — номера не подтверждены.
];

/// Alternative version names accepted by [`protocol_of`], in addition to the
/// canonical names in [`VERSIONS`].
///
/// EN: covers every patch that shares a protocol number (a server list may
/// call itself "1.16.2" while the canonical entry says "1.16.3") plus the
/// common shorthand major names ("1.8", "1.10"). All of them resolve to the
/// protocol number of their group.
///
/// RU: альтернативные имена — патчи с общим номером протокола и короткие
/// имена мажорок ("1.8"). Все ведут к номеру своей группы.
pub const NAME_ALIASES: &[(&str, i32)] = &[
    ("1.8", 47),
    ("1.8.8", 47),
    ("1.9.3", 110),
    ("1.10", 210),
    ("1.10.1", 210),
    ("1.11.1", 316),
    ("1.16.2", 751),
    ("1.16.4", 754),
    ("1.18", 757),
    ("1.19.1", 760),
    ("1.20", 763),
    ("1.20.3", 765),
    ("1.20.5", 766),
    ("1.21", 767),
    ("1.21.2", 768),
    ("1.21.7", 772),
];

/// Look up the display name of a protocol number.
///
/// EN: returns the canonical name of the newest patch that uses this
/// protocol (e.g. 754 -> "1.16.5"). `None` for anything the table does not
/// know — including pre-1.8 numbers and the 26.x placeholder.
///
/// RU: имя последнего патча с этим номером протокола (754 -> "1.16.5"),
/// или None для неизвестного номера.
pub fn version_of(protocol: i32) -> Option<&'static str> {
    VERSIONS
        .iter()
        .find(|e| e.protocol == protocol)
        .map(|e| e.name)
}

/// Look up the protocol number of a version name.
///
/// EN: exact match against canonical names first, then against
/// [`NAME_ALIASES`]. Matching is case-sensitive and whitespace-sensitive —
/// trim the input before calling if it comes from user chat. `None` for
/// unknown names ("1.7.10", "26.3", "1.21.11", ...).
///
/// RU: точный поиск: сначала канонические имена, потом алиасы. Регистр и
/// пробелы важны. None для неизвестных имён.
pub fn protocol_of(name: &str) -> Option<i32> {
    if let Some(e) = VERSIONS.iter().find(|e| e.name == name) {
        return Some(e.protocol);
    }
    NAME_ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|(_, p)| *p)
}

/// List the protocol numbers a translation chain must pass through.
///
/// EN: returns every distinct wire format between `from` and `to`, ascending,
/// both boundaries included. The boundaries themselves are always part of the
/// result even when they are not in the table (the caller may pass a
/// not-yet-pinned target like [`VIA_TARGET_PROTOCOL`]); interior steps are
/// only taken from [`VERSIONS`]. For `from > to` the answer is empty, because
/// a chain never walks downward through this helper.
///
/// Example: `ordered_steps_between(110, 335)` yields
/// `[110, 210, 315, 316, 335]`.
///
/// RU: все форматы на проводе между from и to по возрастанию, обе границы
/// включительно. Границы попадают в ответ даже если их нет в таблице;
/// промежуточные ступени — только из таблицы. При from > to — пустой вектор.
pub fn ordered_steps_between(from: i32, to: i32) -> Vec<i32> {
    if from > to {
        return Vec::new();
    }
    if from == to {
        return vec![from];
    }
    let mut steps = Vec::with_capacity(8);
    steps.push(from);
    for entry in VERSIONS {
        if entry.protocol > from && entry.protocol < to {
            steps.push(entry.protocol);
        }
    }
    steps.push(to);
    steps
}

/// Can the translator attach to a server speaking this protocol?
///
/// EN: true for every protocol number present in [`VERSIONS`] — i.e. every
/// wire format a step chain can start from. False for pre-1.8 numbers,
/// unknown future numbers, and the placeholder [`VIA_TARGET_PROTOCOL`]
/// (the client's own version needs no translation, and the placeholder is
/// not a real number anyway).
///
/// RU: true для любого номера из таблицы VERSIONS. False для всего
/// остального, включая заглушку VIA_TARGET_PROTOCOL.
pub fn is_supported(from: i32) -> bool {
    VERSIONS.iter().any(|e| e.protocol == from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_ascending() {
        for pair in VERSIONS.windows(2) {
            assert!(pair[0].protocol < pair[1].protocol,
                "table out of order around {} / {}", pair[0].protocol, pair[1].protocol);
        }
    }

    #[test]
    fn table_has_no_placeholder() {
        // 26.3 (=777) is the pinned target; it sits above 1.21.x (774) in the chain
    assert!(!VERSIONS.iter().any(|e| e.protocol == VIA_TARGET_PROTOCOL));
    }

    #[test]
    fn version_of_known() {
        assert_eq!(version_of(47), Some("1.8.9"));
        assert_eq!(version_of(754), Some("1.16.5"));
        assert_eq!(version_of(763), Some("1.20.1"));
        assert_eq!(version_of(767), Some("1.21.1"));
        assert_eq!(version_of(110), Some("1.9.4"));
        assert_eq!(version_of(316), Some("1.11.2"));
    }

    #[test]
    fn version_of_unknown() {
        assert_eq!(version_of(0), None);
        assert_eq!(version_of(46), None); // pre-1.8
        assert_eq!(version_of(999_998), None);
        assert_eq!(version_of(VIA_TARGET_PROTOCOL), None);
        assert_eq!(version_of(-1), None);
    }

    #[test]
    fn protocol_of_canonical_names() {
        assert_eq!(protocol_of("1.8.9"), Some(47));
        assert_eq!(protocol_of("1.16.5"), Some(754));
        assert_eq!(protocol_of("1.12.2"), Some(340));
        assert_eq!(protocol_of("1.20.4"), Some(765));
        assert_eq!(protocol_of("1.21.4"), Some(769));
    }

    #[test]
    fn protocol_of_aliases() {
        assert_eq!(protocol_of("1.8"), Some(47));
        assert_eq!(protocol_of("1.8.8"), Some(47));
        assert_eq!(protocol_of("1.9.3"), Some(110));
        assert_eq!(protocol_of("1.10"), Some(210));
        assert_eq!(protocol_of("1.11.1"), Some(316));
        assert_eq!(protocol_of("1.16.2"), Some(751));
        assert_eq!(protocol_of("1.16.4"), Some(754));
        assert_eq!(protocol_of("1.18"), Some(757));
        assert_eq!(protocol_of("1.19.1"), Some(760));
        assert_eq!(protocol_of("1.20"), Some(763));
        assert_eq!(protocol_of("1.21"), Some(767));
        assert_eq!(protocol_of("1.21.7"), Some(772));
    }

    #[test]
    fn protocol_of_unknown() {
        assert_eq!(protocol_of("1.7.10"), None);
        assert_eq!(protocol_of("26.3"), None); // not pinned yet
        assert_eq!(protocol_of("1.21.11"), None); // not pinned yet
        assert_eq!(protocol_of(""), None);
        assert_eq!(protocol_of(" 1.16.5"), None); // no trimming, documented
        assert_eq!(protocol_of("1.16.5 "), None);
    }

    #[test]
    fn version_of_and_protocol_of_roundtrip() {
        for entry in VERSIONS {
            assert_eq!(protocol_of(entry.name), Some(entry.protocol),
                "canonical name {} broke the roundtrip", entry.name);
        }
    }

    #[test]
    fn aliases_point_into_table() {
        for (alias, protocol) in NAME_ALIASES {
            assert!(is_supported(*protocol),
                "alias {} points at unknown protocol {}", alias, protocol);
            let _ = alias; // also compiled into the loop above
        }
    }

    #[test]
    fn steps_single_hop() {
        assert_eq!(ordered_steps_between(47, 107), vec![47, 107]);
        assert_eq!(ordered_steps_between(754, 755), vec![754, 755]);
    }

    #[test]
    fn steps_small_range() {
        assert_eq!(ordered_steps_between(110, 335), vec![110, 210, 315, 316, 335]);
    }

    #[test]
    fn steps_are_strictly_ascending_and_closed() {
        let steps = ordered_steps_between(47, 763);
        assert_eq!(steps.first(), Some(&47));
        assert_eq!(steps.last(), Some(&763));
        for pair in steps.windows(2) {
            assert!(pair[0] < pair[1]);
        }
        // every interior value must be a real table protocol
        for &p in &steps[1..steps.len() - 1] {
            assert!(is_supported(p), "interior step {} is not in the table", p);
        }
    }

    #[test]
    fn steps_equal_bounds() {
        assert_eq!(ordered_steps_between(754, 754), vec![754]);
        // equal unknown bounds still echo back
        assert_eq!(ordered_steps_between(12345, 12345), vec![12345]);
    }

    #[test]
    fn steps_reversed_is_empty() {
        assert!(ordered_steps_between(755, 754).is_empty());
        assert!(ordered_steps_between(763, 47).is_empty());
    }

    #[test]
    fn steps_unknown_boundaries_are_kept() {
        // from below the table floor
        assert_eq!(ordered_steps_between(0, 47), vec![0, 47]);
        // target placeholder as the upper bound
        let steps = ordered_steps_between(774, VIA_TARGET_PROTOCOL);
        assert_eq!(steps, vec![774, VIA_TARGET_PROTOCOL]);
        // full ladder ends at the placeholder
        let full = ordered_steps_between(47, VIA_TARGET_PROTOCOL);
        assert_eq!(full.first(), Some(&47));
        assert_eq!(full.last(), Some(&VIA_TARGET_PROTOCOL));
        assert!(full.len() > VERSIONS.len()); // 47..774 plus both bounds
    }

    #[test]
    fn is_supported_matches_table() {
        for entry in VERSIONS {
            assert!(is_supported(entry.protocol), "{} missing from is_supported", entry.protocol);
        }
        assert!(!is_supported(46));
        assert!(!is_supported(0));
        assert!(!is_supported(VIA_TARGET_PROTOCOL));
        assert!(!is_supported(999_998));
    }
}
