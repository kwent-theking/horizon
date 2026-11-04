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
/// A packet-id translation between the client's native protocol and the
/// wire protocol the server speaks. `None` for an id means the packet has
/// no counterpart: the router drops it (egress) or passes it through
/// untouched with a warning (ingress, so unknown packets stay visible).
/// / RU: трансляция id между родным протоколом клиента и проводным
/// протоколом сервера. None = аналога нет: egress дропает, ingress
/// пропускает как есть (неизвестное остаётся видимым).
#[derive(Clone)]
pub struct Translation {
    /// Native (egress) id -> wire id, serverbound play packets.
    pub egress: fn(i32) -> Option<i32>,
    /// Wire (ingress) id -> native id, clientbound play packets.
    pub ingress: fn(i32) -> i32,
    /// Rewrite an ingress packet body (767 wire shape -> native shape)
    /// after the id was mapped. `None` passes the body through unchanged.
    /// / RU: переписать тело входящего пакета (формат 767 -> родной)
    /// после маппинга id. None = тело не меняется.
    pub ingress_body: fn(i32, &[u8]) -> Option<Vec<u8>>,
}

static ACTIVE: std::sync::RwLock<Option<Translation>> = std::sync::RwLock::new(None);

/// Activate a translation. Called after the server's protocol is probed
/// and again after every configuration phase ends (the translation covers
/// the play phase only).
/// / RU: включить трансляцию: после пробы версии сервера и после каждой
/// завершённой фазы конфигурации (трансляция описывает только play-фазу).
pub fn activate(translation: Translation) {
    *ACTIVE.write().unwrap() = Some(translation);
}

/// Deactivate: a reconfiguration started, the wire speaks the
/// configuration protocol (identical in 767 and 777 - no translation).
/// / RU: выключить: началась реконфигурация, провод говорит на языке
/// конфигурации (у 767 и 777 он совпадает - трансляция не нужна).
pub fn deactivate() {
    *ACTIVE.write().unwrap() = None;
}

/// Whether a translation is active.
pub fn is_active() -> bool {
    ACTIVE.read().unwrap().is_some()
}

/// Rewrite an egress packet body (serverbound). Returns `None` to drop.
/// Idempotent: a packet whose id already looks wire-native passes through
/// unchanged, so a producer that already spoke the wire protocol (login
/// phase, configuration phase) is never double-translated.
/// / RU: переписать исходящее тело пакета. None = дроп. Идемпотентно:
/// пакет с уже проводным id проходит без изменений - фазы без трансляции
/// не задваиваются.
pub fn egress(body: &[u8]) -> Option<Vec<u8>> {
    let Some(tr) = ACTIVE.read().unwrap().clone() else {
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
    let Some(tr) = ACTIVE.read().unwrap().clone() else {
        return id;
    };
    (tr.ingress)(id)
}

/// Rewrite an ingress packet body from the wire shape into the native
/// shape. `None` = unchanged.
/// / RU: переписать тело входящего пакета из провода в родной формат.
/// None = без изменений.
pub fn ingress_body(id: i32, body: &[u8]) -> Option<Vec<u8>> {
    let Some(tr) = ACTIVE.read().unwrap().clone() else {
        return None;
    };
    (tr.ingress_body)(id, body)
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
            0 => Some(0),
            2 => Some(1),
            4 => Some(2),
            6 => Some(3),
            7 => Some(4),
            8 => Some(5),
            9 => Some(6),
            10 => Some(7),
            11 => Some(8),
            12 => Some(9),
            14 => Some(10),
            15 => Some(11),
            16 => Some(12),
            17 => Some(13),
            18 => Some(14),
            19 => Some(15),
            20 => Some(16),
            22 => Some(17),
            24 => Some(19),
            25 => Some(20),
            26 => Some(21),
            27 => Some(22),
            28 => Some(24),
            29 => Some(24),
            30 => Some(25),
            31 => Some(26),
            32 => Some(27),
            33 => Some(28),
            34 => Some(29),
            35 => Some(30),
            39 => Some(32),
            40 => Some(33),
            41 => Some(34),
            42 => Some(35),
            43 => Some(36),
            45 => Some(37),
            47 => Some(38),
            48 => Some(39),
            49 => Some(40),
            50 => Some(41),
            51 => Some(42),
            52 => Some(43),
            53 => Some(44),
            54 => Some(45),
            55 => Some(46),
            56 => Some(47),
            57 => Some(48),
            59 => Some(49),
            60 => Some(50),
            62 => Some(51),
            64 => Some(53),
            66 => Some(54),
            67 => Some(55),
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
            12 => 11,
            13 => 12,
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
            83 => 97,
            84 => 98,
            85 => 100,
            86 => 101,
            87 => 102,
            88 => 103,
            89 => 104,
            90 => 54,
            91 => 106,
            92 => 107,
            93 => 109,
            94 => 110,
            95 => 112,
            96 => 113,
            97 => 114,
            98 => 115,
            99 => 116,
            100 => 117,
            101 => 118,
            102 => 119,
            103 => 120,
            104 => 35,
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
            ingress_body: body767_to_777,
        }
    }

    /// Rewrite 767 wire packet bodies into the native (777) shape. Only the
    /// packets whose wire formats differ get a transform; everything else
    /// passes through.
    /// / RU: переписывает тела пакетов 767 в родной (777) формат. Трансформ
    /// получают только пакеты с различиями; остальное проходит насквозь.
    pub fn body767_to_777(id: i32, body: &[u8]) -> Option<Vec<u8>> {
        match id {
            // LEVEL_CHUNK_WITH_LIGHT: the 767 heightmaps are one unnamed
            /// root NBT compound of long arrays; 777 sends a counted list of
            /// (varint kind, long array) pairs.
            /// / RU: у 767 heightmaps - один NBT-compound из long-массивов;
            /// 777 шлёт список пар (varint-тип, long-массив).
            crate::ids::play::clientbound::LEVEL_CHUNK_WITH_LIGHT => Some(heightmaps_nbt_to_list(body)),
            // ADD_ENTITY: the 767 velocity is three packed shorts at the
            /// tail; 777 carries a low-precision packed Vec3 right after
            /// the position.
            /// / RU: у 767 скорость - три short'а в хвосте; у 777 -
            /// упакованный LpVec3 сразу после позиции.
            crate::ids::play::clientbound::ADD_ENTITY => Some(add_entity_velocity(body)),
            // LOGIN: 767 ends with one bool (enforcesSecure); 777 has two
            /// (online + secure) and a sea_level in SpawnInfo. Insert the
            /// online-mode bool before the trailing one and the sea level
            /// after the SpawnInfo's portal cooldown.
            /// / RU: 767 заканчивается одним bool; у 777 их два и в SpawnInfo
            /// есть sea_level. Вставляем недостающие поля.
            crate::ids::play::clientbound::LOGIN => Some(login_767_to_777(body)),
            // COMMANDS: node layout identical, argument type ids remapped
            /// (resource_selector inserted at 47, uuid 53 -> 60).
            /// / RU: структура узлов та же, id типов аргументов сдвигаются.
            crate::ids::play::clientbound::COMMANDS => Some(commands_767_to_777(body)),
            // UPDATE_ADVANCEMENTS: the icon stack's count/item varints
            /// swap, x/y move to the advancement tail, trailing bool added.
            /// / RU: у icon меняются местами count/item, x/y переезжают
            /// в хвост, добавляется bool в конец.
            crate::ids::play::clientbound::UPDATE_ADVANCEMENTS => Some(update_advancements_767_to_777(body)),
            // UPDATE_ATTRIBUTES: holder (id+1) -> plain registry id.
            /// / RU: holder (id+1) -> плоский id реестра.
            crate::ids::play::clientbound::UPDATE_ATTRIBUTES => Some(update_attributes_767_to_777(body)),
            _ => None,
        }
    }

    /// AddEntity 767 -> 777: move the velocity from three trailing shorts
    /// into the LpVec3 slot after the position (head yaw and data follow).
    /// 767: [varint id][uuid][varint kind][dbl x y z][b pitch][b yaw][b
    /// headYaw][varint data][short vx vy vz]
    /// 777: [varint id][uuid][varint kind][dbl x y z][LpVec3][b pitch][b
    /// yaw][b headYaw][varint data]
    /// / RU: переносим скорость из трёх short'ов в хвосте в слот LpVec3
    /// после позиции.
    fn add_entity_velocity(body: &[u8]) -> Vec<u8> {
        // 767: [varint id][uuid][varint kind][dbl x y z][b pitch][b yaw][b
        /// headYaw][varint data][short vx][short vy][short vz]
        /// 777: [varint id][uuid][varint kind][dbl x y z][LpVec3][b
        /// pitch][b yaw][b headYaw][varint data]
        /// / RU: скорость из трёх short'ов в хвосте переносится в слот
        /// LpVec3 сразу после позиции.
        use crate::buf::Reader;
        let mut r = Reader::new(body);
        let _id = r.varint();
        let _uuid = r.uuid();
        let _kind = r.varint();
        let mut xyz = [0f64; 3];
        let mut ok = true;
        for v in xyz.iter_mut() {
            match r.f64() {
                Ok(x) => *v = x,
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok || _id.is_err() || _uuid.is_err() || _kind.is_err() {
            return body.to_vec();
        }
        let after_xyz = r.position();
        // Three shorts at the tail: the 767 velocity.
        if body.len() < 6 {
            return body.to_vec();
        }
        let vx = i16::from_be_bytes([body[body.len() - 6], body[body.len() - 5]]);
        let vy = i16::from_be_bytes([body[body.len() - 4], body[body.len() - 3]]);
        let vz = i16::from_be_bytes([body[body.len() - 2], body[body.len() - 1]]);
        let [x, y, z] = [vx as f64 / 8000.0, vy as f64 / 8000.0, vz as f64 / 8000.0];
        let mut out = Vec::with_capacity(body.len());
        // head + position
        out.extend_from_slice(&body[..after_xyz]);
        // LpVec3 right after the position (before pitch/yaw/headYaw)
        write_lp_vec3(&mut out, [x, y, z]);
        // the rest up to the six velocity bytes
        out.extend_from_slice(&body[after_xyz..body.len() - 6]);
        out
    }

    fn write_lp_vec3(out: &mut Vec<u8>, v: [f64; 3]) {
        let unpack = |b: u64| ((b & 32767) as f64).min(32766.0) * 2.0 / 32766.0 - 1.0;
        let pack = |v: f64| ((v * 0.5 + 0.5) * 32766.0).round() as u64;
        let [x, y, z] = v.map(|c| if c.is_nan() { 0.0 } else { c.clamp(-1.717_986_918_3e10, 1.717_986_918_3e10) });
        let length = x.abs().max(y.abs()).max(z.abs());
        if length < 3.051_944_088_384_301e-5 {
            out.push(0);
            return;
        }
        let scale = length.ceil() as u64;
        let partial = scale & 3 != scale;
        let markers = if partial { scale & 3 | 4 } else { scale };
        let s = scale as f64;
        let buffer = markers | pack(x / s) << 3 | pack(y / s) << 18 | pack(z / s) << 33;
        out.push(buffer as u8);
        out.push((buffer >> 8) as u8);
        out.extend_from_slice(&((buffer >> 16) as u32).to_be_bytes());
        if partial {
            let mut v = (scale >> 2) as u32;
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
        let _ = unpack(0u64); // silence unused
    }

    /// ResourceLocation.STREAM_CODEC == STRING_UTF8: copy verbatim.
    /// / RU: ResourceLocation = строка UTF-8: копия как есть.
    fn copy_ident(r: &mut crate::buf::Reader, w: &mut crate::buf::Writer) -> crate::Result<()> {
        let s = r.string(32767)?;
        w.string(&s);
        Ok(())
    }
    fn copy_nbt(r: &mut crate::buf::Reader, w: &mut crate::buf::Writer) -> crate::Result<()> {
        let tag = r.nbt()?;
        w.nbt(&tag);
        Ok(())
    }

    /// UpdateAdvancements 767 -> 777: the icon stack's count/item varints
    /// swap (767: [count][holder id+1][patch]; 777: [id][count][patch]),
    /// the x/y floats move from the display to the advancement tail and
    /// the 777 packet ends with one more bool.
    /// / RU: у icon меняются местами count/item, x/y переезжают из display
    /// в хвост advancement, в конце добавляется bool.
    fn update_advancements_767_to_777(body: &[u8]) -> Vec<u8> {
        let mut r = crate::buf::Reader::new(body);
        let mut w = crate::buf::Writer::new();
        let reset = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
        w.bool(reset);
        let n = match r.count(r.remaining()) { Ok(v) => v, Err(_) => return body.to_vec() };
        w.varint(n as i32);
        for _ in 0..n {
            if copy_ident(&mut r, &mut w).is_err() { return body.to_vec(); }
            let has = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
            w.bool(has);
            if has && copy_ident(&mut r, &mut w).is_err() { return body.to_vec(); }
            let has_display = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
            w.bool(has_display);
            let (x, y) = if has_display {
                if copy_nbt(&mut r, &mut w).is_err() { return body.to_vec(); }
                if copy_nbt(&mut r, &mut w).is_err() { return body.to_vec(); }
                // ItemStack 767: [varint count][varint holder = id+1][patch]
                let count = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
                let holder = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
                if count <= 0 || holder <= 0 { return body.to_vec(); }
                let start = r.position();
                let positive = match r.count(65536) { Ok(v) => v, Err(_) => return body.to_vec() };
                for _ in 0..positive {
                    let kind = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
                    if crate::item::Component::read(kind, &mut r).is_err() { return body.to_vec(); }
                }
                let negative = match r.count(65536) { Ok(v) => v, Err(_) => return body.to_vec() };
                for _ in 0..negative {
                    if r.varint().is_err() { return body.to_vec(); }
                }
                // 777: [varint item][varint count][patch verbatim]
                w.varint(holder - 1);
                w.varint(count);
                w.bytes(r.since(start));
                let frame = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
                w.varint(frame);
                let flags = match r.i32() { Ok(v) => v, Err(_) => return body.to_vec() };
                w.i32(flags);
                if flags & 1 != 0 && copy_ident(&mut r, &mut w).is_err() { return body.to_vec(); }
                let x = match r.f32() { Ok(v) => v, Err(_) => return body.to_vec() };
                let y = match r.f32() { Ok(v) => v, Err(_) => return body.to_vec() };
                (x, y)
            } else { (0.0, 0.0) };
            // requirements: [varint groups][per: [varint k][k strings]]
            let groups = match r.count(r.remaining()) { Ok(v) => v, Err(_) => return body.to_vec() };
            w.varint(groups as i32);
            for _ in 0..groups {
                let k = match r.count(r.remaining()) { Ok(v) => v, Err(_) => return body.to_vec() };
                w.varint(k as i32);
                for _ in 0..k {
                    if copy_ident(&mut r, &mut w).is_err() { return body.to_vec(); }
                }
            }
            let telemetry = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
            w.bool(telemetry);
            w.f32(x);
            w.f32(y);
        }
        // removed
        let n = match r.count(r.remaining()) { Ok(v) => v, Err(_) => return body.to_vec() };
        w.varint(n as i32);
        for _ in 0..n {
            if copy_ident(&mut r, &mut w).is_err() { return body.to_vec(); }
        }
        // progress
        let n = match r.count(r.remaining()) { Ok(v) => v, Err(_) => return body.to_vec() };
        w.varint(n as i32);
        for _ in 0..n {
            if copy_ident(&mut r, &mut w).is_err() { return body.to_vec(); }
            let k = match r.count(r.remaining()) { Ok(v) => v, Err(_) => return body.to_vec() };
            w.varint(k as i32);
            for _ in 0..k {
                if copy_ident(&mut r, &mut w).is_err() { return body.to_vec(); }
                let done = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
                w.bool(done);
                if done {
                    let at = match r.i64() { Ok(v) => v, Err(_) => return body.to_vec() };
                    w.i64(at);
                }
            }
        }
        w.bool(true); // the 777-only trailing show
        w.data
    }

    /// Commands 767 -> 777: node layout identical, argument type ids
    /// remapped (resource_selector inserted at 47, uuid 53 -> 60).
    /// / RU: структура узлов та же, id типов аргументов сдвигаются.
    fn commands_767_to_777(body: &[u8]) -> Vec<u8> {
        let map = |id: i32| match id {
            0..=46 => id,
            47..=52 => id + 1,
            53 => 60,
            other => other,
        };
        let mut r = crate::buf::Reader::new(body);
        let mut w = crate::buf::Writer::new();
        let n = match r.count(r.remaining()) { Ok(v) => v, Err(_) => return body.to_vec() };
        w.varint(n as i32);
        for _ in 0..n {
            let flags = match r.u8() { Ok(v) => v, Err(_) => return body.to_vec() };
            w.u8(flags);
            let k = match r.count(r.remaining()) { Ok(v) => v, Err(_) => return body.to_vec() };
            w.varint(k as i32);
            for _ in 0..k {
                let c = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
                w.varint(c);
            }
            if flags & 8 != 0 {
                let redirect = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
                w.varint(redirect);
            }
            let kind = flags & 3;
            if kind >= 1 && copy_ident(&mut r, &mut w).is_err() { return body.to_vec(); }
            if kind == 2 {
                let id = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
                let props: Vec<u8> = if (0..=46).contains(&id) {
                    let start = r.position();
                    if crate::packets::play::ArgumentProperties::read(&mut r, id).is_err() {
                        return body.to_vec();
                    }
                    r.since(start).to_vec()
                } else if (47..=53).contains(&id) {
                    Vec::new()
                } else {
                    return body.to_vec(); // modded argument type: unmappable
                };
                w.varint(map(id));
                w.bytes(&props);
                if flags & 16 != 0 && copy_ident(&mut r, &mut w).is_err() { return body.to_vec(); }
            }
        }
        let root = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
        w.varint(root);
        w.data
    }

    /// UpdateAttributes 767 -> 777: the 767 attribute is a registry holder
    /// (varint id+1, 0 = direct); 777 uses the plain registry id.
    /// / RU: holder (id+1) -> плоский id; остальное байт-в-байт.
    fn update_attributes_767_to_777(body: &[u8]) -> Vec<u8> {
        let mut r = crate::buf::Reader::new(body);
        let mut w = crate::buf::Writer::new();
        let id = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
        w.varint(id);
        let n = match r.count(r.remaining()) { Ok(v) => v, Err(_) => return body.to_vec() };
        w.varint(n as i32);
        for _ in 0..n {
            let holder = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
            if holder <= 0 { return body.to_vec(); }
            w.varint(holder - 1);
            let base = match r.f64() { Ok(v) => v, Err(_) => return body.to_vec() };
            w.f64(base);
            let k = match r.count(r.remaining()) { Ok(v) => v, Err(_) => return body.to_vec() };
            w.varint(k as i32);
            for _ in 0..k {
                if copy_ident(&mut r, &mut w).is_err() { return body.to_vec(); }
                let amount = match r.f64() { Ok(v) => v, Err(_) => return body.to_vec() };
                w.f64(amount);
                let op = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
                w.varint(op);
            }
        }
        w.data
    }

    /// Heightmap kind ids for the 777 list form, from the 777 heightmap
    /// types table (same order as the vanilla Heightmap.Type enum:
    /// WORLD_FLOOR=0, WORLD_SURFACE=1, OCEAN_FLOOR=2, OCEAN_FLOOR_WG=3,
    /// MOTION_BLOCKING=4, MOTION_BLOCKING_NO_LEAVES=5).
    /// / RU: id типов heightmap для списка 777 в порядке enum Heightmap.Type.
    /// Login 767 -> 777: 767 is
    /// [eid][hardcore][dims][max][chunkR][simD][reduced][showDeath][limited]
    /// [SpawnInfo][secure]
    /// 777 is the same with [SpawnInfo][online][secure] and one extra
    /// SpawnInfo field (sea_level) at the end of the SpawnInfo.
    /// Strategy: parse both fully with 767 readers and re-emit in the 777
    /// shape. On any parse failure the original body passes through.
    /// / RU: разбираем пакет полностью по 767-полям и перезаписываем в
    /// формате 777; при ошибке - тело проходит как есть.
    fn login_767_to_777(body: &[u8]) -> Vec<u8> {
        use crate::buf::Reader;
        let mut r = Reader::new(body);
        let player_id = match r.i32() { Ok(v) => v, Err(_) => return body.to_vec() };
        let hardcore = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
        let dims = match read_ident_set(&mut r) { Ok(v) => v, Err(_) => return body.to_vec() };
        let max_players = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
        let chunk_radius = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
        let sim_distance = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
        let reduced = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
        let show_death = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
        let limited = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
        // CommonPlayerSpawnInfo (767, without sea_level)
        let dim_type = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
        let dimension = match r.string(32767) { Ok(v) => v, Err(_) => return body.to_vec() };
        let seed = match r.i64() { Ok(v) => v, Err(_) => return body.to_vec() };
        // 767 writes the game types as bytes, not varints.
        /// / RU: у 767 типы игры - байты, не varint.
        let game_type = match r.i8() { Ok(v) => v as i32, Err(_) => return body.to_vec() };
        let previous = match r.i8() { Ok(v) => v as i32, Err(_) => return body.to_vec() };
        let debug = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
        let flat = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
        let last_death = match read_opt_global_pos(&mut r) { Ok(v) => v, Err(_) => return body.to_vec() };
        let portal_cooldown = match r.varint() { Ok(v) => v, Err(_) => return body.to_vec() };
        let secure = match r.bool() { Ok(v) => v, Err(_) => return body.to_vec() };
        // Re-emit in the 777 shape.
        let mut w = crate::buf::Writer::new();
        w.i32(player_id);
        w.bool(hardcore);
        w.varint(dims.len() as i32);
        for d in &dims {
            write_string(&mut w, d);
        }
        w.varint(max_players);
        w.varint(chunk_radius);
        w.varint(sim_distance);
        w.bool(reduced);
        w.bool(show_death);
        w.bool(limited);
        // SpawnInfo with the 777 sea_level tail
        w.varint(dim_type);
        write_string(&mut w, &dimension);
        w.i64(seed);
        w.varint(game_type);
        w.bool(true); // 767 always carries a previous game type
        w.varint(previous);
        w.bool(debug);
        w.bool(flat);
        match last_death {
            Some((ref dim, pos)) => {
                w.bool(true);
                write_string(&mut w, dim);
                w.i32(pos[0]);
                w.i32(pos[1]);
                w.i32(pos[2]);
            }
            None => w.bool(false),
        }
        w.varint(portal_cooldown);
        w.varint(63); // sea level
        w.bool(true); // online mode
        w.bool(secure);
        w.data
    }

    /// [varint n][n idents]
    fn read_ident_set(r: &mut crate::buf::Reader) -> crate::Result<Vec<String>> {
        let n = r.count(r.remaining())?;
        (0..n).map(|_| r.string(32767)).collect()
    }

    /// Opt<VarInt>: bool present + varint.
    fn read_opt_varint(r: &mut crate::buf::Reader) -> crate::Result<Option<i32>> {
        if r.bool()? {
            Ok(Some(r.varint()?))
        } else {
            Ok(None)
        }
    }

    /// Opt<GlobalPos>: bool + ident + a packed 767 BlockPos long
    /// (x << 38 | z << 12 | y, 26/26/12 bits).
    /// / RU: Opt<GlobalPos>: bool + ident + упакованная BlockPos-long.
    fn read_opt_global_pos(r: &mut crate::buf::Reader) -> crate::Result<Option<(String, [i32; 3])>> {
        if r.bool()? {
            let dim = r.string(32767)?;
            let packed = r.i64()? as i64;
            let x = (packed >> 38) as i32;
            let z = ((packed << 26) >> 38) as i32;
            let y = ((packed << 52) >> 52) as i32;
            Ok(Some((dim, [x, y, z])))
        } else {
            Ok(None)
        }
    }

    fn write_string(w: &mut crate::buf::Writer, s: &str) {
        w.string(s);
    }

    fn heightmap_kind(name: &str) -> i32 {
        match name {
            "WORLD_FLOOR" => 0,
            "WORLD_SURFACE" => 1,
            "OCEAN_FLOOR" => 2,
            "OCEAN_FLOOR_WG" => 3,
            "MOTION_BLOCKING" => 4,
            "MOTION_BLOCKING_NO_LEAVES" => 5,
            _ => 6,
        }
    }

    /// Transform the level-chunk packet: [x][z][NBT heightmaps][sections]
    /// -> [x][z][list heightmaps][777 sections]. The 767 section is
    /// [i16 nonEmpty][blocks][biomes]; 777 adds an i16 fluids after the
    /// non-empty count. Block entities and light data follow
    /// byte-identical.
    /// / RU: трансформация чанка: NBT-heightmaps -> список, а в каждую
    /// секцию после nonEmpty вставляется i16 fluids (его нет у 767).
    /// Блок-энтити и свет идут байт-в-байт.
    fn heightmaps_nbt_to_list(body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(body.len() + 96);
        // [int x][int z]
        if body.len() < 8 {
            return body.to_vec();
        }
        out.extend_from_slice(&body[..8]);
        let nbt = &body[8..];
        // Parse the unnamed network NBT root; on any failure keep the
        /// original bytes (the native decoder will surface the error).
        let (tag, consumed) = match horizon_world::nbt::read_network(nbt, horizon_world::nbt::Limits::default()) {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let mut entries: Vec<(i32, Vec<i64>)> = Vec::new();
        if let horizon_world::nbt::Tag::Compound(map) = &tag {
            for (name, value) in map.iter() {
                if let horizon_world::nbt::Tag::LongArray(data) = value {
                    entries.push((heightmap_kind(&name.to_string_lossy()), data.clone()));
                }
            }
        }
        // 777 Heightmaps list: [varint n][per: varint kind][varint len][longs]
        let mut w = crate::buf::Writer::new();
        w.varint(entries.len() as i32);
        for (kind, data) in &entries {
            w.varint(*kind);
            w.long_array(data);
        }
        out.extend_from_slice(&w.data);
        // The section buffer: a varint byte count, then the 767 sections.
        let rest = &nbt[consumed..];
        let mut r = crate::buf::Reader::new(rest);
        let buf_len = match r.varint() {
            Ok(v) => v,
            Err(_) => {
                out.extend_from_slice(rest);
                return out;
            }
        };
        let buf_len = match usize::try_from(buf_len) {
            Ok(v) => v,
            Err(_) => {
                out.extend_from_slice(rest);
                return out;
            }
        };
        let sections_start = r.position();
        let sections_end = sections_start + buf_len;
        if sections_end > rest.len() {
            out.extend_from_slice(rest);
            return out;
        }
        let sections = &rest[sections_start..sections_end];
        let after = &rest[sections_end..];
        match sections_767_to_777(sections) {
            Some(mut transformed) => {
                w = crate::buf::Writer::new();
                w.varint(transformed.len() as i32);
                out.extend_from_slice(&w.data);
                out.append(&mut transformed);
            }
            None => {
                // Not parseable as sections: pass through unchanged and
                // say so once - a silent passthrough never decodes
                // natively on 777.
                // / RU: секции не разобрались - проход насквозь с одним
                // логом: молчаливый проход никогда не декодится в 777.
                
                w = crate::buf::Writer::new();
                w.varint(buf_len as i32);
                out.extend_from_slice(&w.data);
                out.extend_from_slice(sections);
            }
        }
        out.extend_from_slice(after);
        out
    }

    /// Rewrite the 767 section run into the 777 shape: per section the 767
    /// is [i16 nonEmpty][Paletted blocks][Paletted biomes] and the 777 adds
    /// an i16 fluids after the nonEmpty count. The whole run must parse;
    /// on any failure None (the caller copies the run unchanged).
    /// / RU: перезапись секций 767 в формат 777: после nonEmpty вставляется
    /// i16 fluids. Весь блок должен разбираться; иначе None.
    fn sections_767_to_777(sections: &[u8]) -> Option<Vec<u8>> {
        let mut r = crate::buf::Reader::new(sections);
        let mut out = Vec::with_capacity(sections.len() + 48);
        while r.remaining() > 0 {
            let sect_start = r.position();
            let non_empty = r.i16().ok()?;
            // Paletted: [u8 bits][palette?][longs] for both blocks (4096
            /// entries) and biomes (64 entries), 767 shape.
            let blocks = copy_paletted_767(&mut r, 4096)?;
            let biomes = copy_paletted_767(&mut r, 64)?;
            let sect_end = r.position();
            out.extend_from_slice(&sections[sect_start..sect_start + 2]);
            out.extend_from_slice(&0i16.to_be_bytes()); // fluids: none
            out.extend_from_slice(&sections[sect_start + 2..sect_end]);
            let _ = (blocks, biomes);
        }
        Some(out)
    }

    /// Skip one 767 paletted container and return the byte range it
    /// occupies; None when the run cannot parse.
    /// / RU: пропустить один контейнер 767 и вернуть его байтовый диапазон.
    fn copy_paletted_767(r: &mut crate::buf::Reader, entries: usize) -> Option<std::ops::Range<usize>> {
        let start = r.position();
        let bits = r.u8().ok()?;
        // getConfigurationForBitCount: 0 = single value; 1..=4 (blocks) /
        /// 1..=3 (biomes) use a palette; beyond that the global palette.
        let has_palette = match (entries, bits) {
            (_, 0) => true,
            (4096, 1..=4) => true,
            (64, 1..=3) => true,
            _ => false,
        };
        if bits > 32 {
            return None;
        }
        if has_palette {
            if bits == 0 {
                r.varint().ok()?;
            } else {
                let n = r.count(1 << bits).ok()?;
                for _ in 0..n {
                    r.varint().ok()?;
                }
            }
        }
        let per = if bits == 0 { 0 } else { 64 / bits as usize };
        let longs = if bits == 0 { 0 } else { entries.div_ceil(per) };
        r.fixed_longs(longs).ok()?;
        Some(start..r.position())
    }
}
