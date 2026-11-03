//! The unified packet translation layer: every packet crossing the socket
//! in either direction passes here exactly once, before framing/compression
//! on egress and after decryption/deframing on ingress.
//!
//! EN: one gateway for both directions. A packet body is
//! `VarInt id + payload`. The router consults the active `Translation`
//! (set once the server's protocol is known) and rewrites the id, or drops
//! the packet when no counterpart exists. Nothing else in the codebase may
//! rewrite packet ids.
//! RU: единый шлюз в обе стороны. Тело пакета - `VarInt id + payload`;
//! маршрутизатор сверяется с активной трансляцией (ставится после пробы
//! версии сервера) и переписывает id либо дропает пакет, если аналога нет.
//! Никто другой в кодовой базе id пакетов не переписывает.
use std::sync::OnceLock;

/// A packet-id translation between the client's native protocol and the
/// wire protocol the server speaks. `None` for an id means the packet has
/// no counterpart: the router drops it (egress) or passes it through
/// untouched with a warning (ingress, so unknown packets stay visible).
/// / RU: трансляция id между родным протоколом клиента и проводным
/// протоколом сервера. None = аналога нет: egress дропает, ingress
/// пропускает как есть (неизвестное остаётся видимым).
pub struct Translation {
    /// Native (egress) id -> wire id, serverbound play packets.
    pub egress: fn(i32) -> Option<i32>,
    /// Wire (ingress) id -> native id, clientbound play packets.
    pub ingress: fn(i32) -> i32,
}

static ACTIVE: OnceLock<Translation> = OnceLock::new();

/// Activate a translation. Called once, after the server's protocol is
/// probed; calling it again panics (a second activation would mean two
/// owners fight over the wire format).
/// / RU: включить трансляцию. Вызывается один раз после пробы версии
/// сервера; повторный вызов - паника (два владельца формата - баг).
pub fn activate(translation: Translation) {
    ACTIVE.set(translation).ok().expect("the translation is already active");
}

/// Whether a translation is active.
pub fn is_active() -> bool {
    ACTIVE.get().is_some()
}

/// Rewrite an egress packet body (serverbound). Returns `None` to drop.
/// Idempotent: a packet whose id already looks wire-native passes through
/// unchanged, so a producer that already spoke the wire protocol (login
/// phase, configuration phase) is never double-translated.
/// / RU: переписать исходящее тело пакета. None = дроп. Идемпотентно:
/// пакет с уже проводным id проходит без изменений - фазы без трансляции
/// не задваиваются.
pub fn egress(body: &[u8]) -> Option<Vec<u8>> {
    let Some(tr) = ACTIVE.get() else {
        return Some(body.to_vec());
    };
    let (id, k) = read_varint(body)?;
    let wire = (tr.egress)(id)?;
    if wire == id && k == varint_len(wire) {
        return Some(body.to_vec());
    }
    let mut out = Vec::with_capacity(body.len() + 2);
    write_varint(&mut out, wire);
    out.extend_from_slice(&body[k..]);
    Some(out)
}

/// Rewrite an ingress packet (clientbound) id. Returns the id the native
/// decoder should use; unknown ids pass through so the caller's Unknown
/// handling keeps working.
/// / RU: переписать id входящего пакета. Неизвестные проходят насквозь -
/// обработка Unknown у вызывающего продолжает работать.
pub fn ingress(id: i32) -> i32 {
    let Some(tr) = ACTIVE.get() else {
        return id;
    };
    (tr.ingress)(id)
}

fn read_varint(body: &[u8]) -> Option<(i32, usize)> {
    let mut value = 0i32;
    let mut shift = 0;
    let mut k = 0;
    while let Some(&b) = body.get(k) {
        k += 1;
        value |= ((b & 0x7F) as i32) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            return Some((value, k));
        }
    }
    None
}

fn varint_len(value: i32) -> usize {
    let mut v = value as u32;
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

fn write_varint(out: &mut Vec<u8>, value: i32) {
    let mut v = value as u32;
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}



/// The 767 (1.21.x) <-> 777 (26.x) play-packet translation tables, built
/// from the vanilla 1.21.1 server jar (piston-data + Mojang mappings).
/// Wire rule: id = lambda index + 1 (the bundle delimiter occupies wire
/// id 0 in both directions).
/// / RU: таблицы трансляции play-пакетов 767 <-> 777, собранные из
/// vanilla 1.21.1 (piston-data + маппинги Mojang). Правило провода:
/// id = индекс лямбды + 1 (0 занимает делимитер бандла в обе стороны).
pub mod p767 {
    pub fn sb777_to_767(id: i32) -> Option<i32> {
        match id {
            0 => Some(1),
            2 => Some(2),
            4 => Some(3),
            6 => Some(4),
            7 => Some(5),
            8 => Some(6),
            9 => Some(7),
            10 => Some(8),
            11 => Some(9),
            12 => Some(10),
            14 => Some(11),
            15 => Some(12),
            16 => Some(13),
            17 => Some(14),
            18 => Some(15),
            19 => Some(16),
            20 => Some(17),
            22 => Some(18),
            24 => Some(20),
            25 => Some(21),
            26 => Some(22),
            27 => Some(23),
            28 => Some(24),
            29 => Some(25),
            30 => Some(26),
            31 => Some(27),
            32 => Some(28),
            33 => Some(29),
            34 => Some(30),
            35 => Some(31),
            39 => Some(33),
            40 => Some(34),
            41 => Some(35),
            42 => Some(36),
            43 => Some(37),
            45 => Some(38),
            47 => Some(39),
            48 => Some(40),
            49 => Some(41),
            50 => Some(42),
            51 => Some(43),
            52 => Some(44),
            53 => Some(45),
            54 => Some(46),
            55 => Some(47),
            56 => Some(48),
            57 => Some(49),
            59 => Some(50),
            60 => Some(51),
            62 => Some(52),
            64 => Some(54),
            66 => Some(55),
            67 => Some(56),
            _ => None,
        }
    }

    /// 767 wire id (bundle+delimiter occupy 0/1) -> 777 id, clientbound play.
    /// / RU: проводной id 767 -> id 777, clientbound play.
    /// 767 wire id (wire = index + 1: BUNDLE=0, delimiter=1) -> 777 id,
    /// clientbound play, built from vanilla 1.21.1 + Horizon 777.
    /// / RU: проводной id 767 -> id 777, clientbound play.
    pub fn cb(id: i32) -> i32 {
        match id {
            2 => 1,
            4 => 2,
            5 => 3,
            6 => 4,
            7 => 5,
            8 => 6,
            9 => 7,
            10 => 8,
            11 => 9,
            12 => 10,
            13 => 11,
            14 => 12,
            15 => 13,
            16 => 14,
            17 => 15,
            18 => 16,
            19 => 17,
            20 => 18,
            21 => 19,
            22 => 20,
            23 => 22,
            24 => 23,
            25 => 24,
            26 => 25,
            27 => 30,
            28 => 31,
            29 => 32,
            30 => 33,
            31 => 34,
            32 => 36,
            33 => 38,
            34 => 39,
            36 => 43,
            37 => 44,
            38 => 45,
            39 => 46,
            40 => 47,
            41 => 48,
            42 => 49,
            43 => 50,
            44 => 52,
            45 => 53,
            46 => 54,
            47 => 55,
            48 => 57,
            49 => 58,
            50 => 59,
            51 => 60,
            52 => 61,
            53 => 62,
            54 => 64,
            55 => 65,
            56 => 66,
            57 => 67,
            58 => 68,
            59 => 69,
            60 => 70,
            61 => 71,
            62 => 72,
            63 => 73,
            65 => 78,
            66 => 79,
            67 => 80,
            68 => 81,
            69 => 82,
            70 => 84,
            71 => 85,
            72 => 86,
            73 => 87,
            74 => 88,
            75 => 89,
            76 => 90,
            77 => 91,
            78 => 92,
            79 => 93,
            80 => 94,
            81 => 95,
            83 => 96,
            84 => 97,
            85 => 99,
            86 => 100,
            87 => 101,
            88 => 102,
            89 => 103,
            90 => 104,
            91 => 105,
            92 => 106,
            93 => 108,
            94 => 109,
            95 => 111,
            96 => 112,
            97 => 113,
            98 => 114,
            99 => 115,
            100 => 116,
            101 => 117,
            102 => 118,
            103 => 119,
            104 => 120,
            105 => 121,
            106 => 122,
            107 => 124,
            108 => 125,
            109 => 126,
            110 => 127,
            111 => 128,
            112 => 130,
            113 => 131,
            114 => 132,
            115 => 133,
            116 => 134,
            117 => 135,
            118 => 136,
            119 => 137,
            120 => 138,
            121 => 139,
            122 => 140,
            other => other,
        }
    }
    /// The egress/ingress pair for a 767-speaking server.
    /// / RU: пара трансляций для сервера на 767.
    pub fn translation() -> super::Translation {
        super::Translation {
            egress: sb777_to_767,
            ingress: cb,
        }
    }
}
