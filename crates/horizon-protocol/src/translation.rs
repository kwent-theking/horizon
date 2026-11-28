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

/// The registries whose server names the unknown-id log lines quote, and the
/// sound events CBC's blast waves name by registry id.
const NAMED_REGISTRIES: [&str; 7] = [
    "minecraft:item",
    "minecraft:entity_type",
    "minecraft:command_argument_type",
    "minecraft:data_component_type",
    "minecraft:particle_type",
    "minecraft:sound_event",
    "neoforge:entity_data_serializers",
];

/// (registry, id) -> the server's name.
type ServerNames = std::sync::RwLock<std::collections::HashMap<(String, i32), String>>;

fn server_names() -> &'static ServerNames {
    static NAMES: std::sync::OnceLock<ServerNames> = std::sync::OnceLock::new();
    NAMES.get_or_init(Default::default)
}

/// Remember the server's names for the ids the 767 translation may meet
/// without a codec (mods' item components, particles and entity data
/// serializers), from its `*.registries.tsv` (`registry \t id \t name`).
pub fn set_server_names(tsv: &str) {
    let names = parse_server_names(tsv);
    if let Ok(mut table) = server_names().write() {
        *table = names;
    }
}

fn parse_server_names(tsv: &str) -> std::collections::HashMap<(String, i32), String> {
    tsv.lines()
        .filter_map(|line| {
            let mut cols = line.splitn(3, '\t');
            let (registry, id, name) = (cols.next()?, cols.next()?, cols.next()?);
            if !NAMED_REGISTRIES.contains(&registry) {
                return None;
            }
            let key = (registry.to_owned(), id.parse::<i32>().ok()?);
            Some((key, name.to_owned()))
        })
        .collect()
}

/// The server's name for a reserved particle id (`MODDED_PARTICLE_BASE` plus its
/// `minecraft:particle_type` id) a translated LevelParticles packet carries.
pub fn modded_particle_name(id: i32) -> Option<String> {
    modded_particle_name_in(|server| server_name("minecraft:particle_type", server), id)
}

fn modded_particle_name_in(names: impl Fn(i32) -> Option<String>, id: i32) -> Option<String> {
    names(crate::entity_data::v767::reserved_particle_server_id(id)?)
}

/// Log `message` once per process (a bounded number of distinct ones).
pub(crate) fn warn_once(message: String) {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let Ok(mut seen) = SEEN.get_or_init(Default::default).lock() else {
        return;
    };
    if seen.len() < 1024 && seen.insert(message.clone()) {
        log::warn!("{message}");
    }
}

/// The server's name for `id` in `registry`, when its table is loaded.
fn server_name(registry: &str, id: i32) -> Option<String> {
    server_names()
        .read()
        .ok()
        .and_then(|table| table.get(&(registry.to_owned(), id)).cloned())
}

/// The server's id for `name` in `registry`, when its table is loaded.
fn server_id(registry: &str, name: &str) -> Option<i32> {
    server_names().read().ok().and_then(|table| {
        table
            .iter()
            .find(|((r, _), n)| r == registry && n.as_str() == name)
            .map(|((_, id), _)| *id)
    })
}

/// The native id of an ingress packet after its body was rewritten.
pub fn native_id(id: i32) -> i32 {
    if id == p767::EXPERIENCE_ORB {
        crate::ids::play::clientbound::ADD_ENTITY
    } else {
        id
    }
}

/// Log, once, a 767 id of `registry` the translation has no codec for, with
/// the server's name for it when known.
pub(crate) fn unknown_767(registry: &str, id: i32) {
    let name = server_names()
        .read()
        .ok()
        .and_then(|table| table.get(&(registry.to_owned(), id)).cloned())
        .unwrap_or_else(|| "no server name".to_owned());
    warn_once(format!(
        "Horizon: no 1.21.1 codec for {registry} {id} ({name}); what holds it is dropped"
    ));
}

/// Log, once, a mod's 767 id of `registry` whose value the server's codec
/// tree (crate::codec_tree) read past and left out.
pub(crate) fn left_out_767(registry: &str, id: i32) {
    let name = server_name(registry, id).unwrap_or_else(|| "no server name".to_owned());
    warn_once(format!(
        "Horizon: {registry} {id} ({name}) read by the server's codec tree and left out"
    ));
}

/// Rewrite an egress packet body (serverbound). Returns `None` to drop.
/// Idempotent: a packet whose id already looks wire-native passes through
/// unchanged, so a producer that already spoke the wire protocol (login
/// phase, configuration phase) is never double-translated.
/// / RU: переписать исходящее тело пакета. None = дроп. Идемпотентно:
/// пакет с уже проводным id проходит без изменений - фазы без трансляции
/// не задваиваются.

/// Length in bytes of the varint at the start of `b`.
/// / RU: длина varint в начале среза `b`.
fn varint_len_at(b: &[u8]) -> usize {
    for (i, &x) in b.iter().enumerate().take(5) {
        if x & 0x80 == 0 {
            return i + 1;
        }
    }
    5
}

/// What the egress rewrite must remember about the local player between
/// packets: the entity id (from the login packet) and whether the last
/// player_input had shift held (767 learns sneaking from player_command, not
/// from player_input).
/// / RU: что egress помнит о локальном игроке между пакетами: id сущности
/// (из login) и зажатый shift из последнего player_input.
pub struct EgressState {
    entity_id: std::sync::atomic::AtomicI32,
    shift: std::sync::atomic::AtomicBool,
    /// Signed player_chat packets received and not yet acknowledged.
    pending_chat: std::sync::atomic::AtomicI32,
}

/// Unacknowledged signed chat lines after which a chat_ack goes out
/// (vanilla acknowledges in batches of 64).
const CHAT_ACK_BATCH: i32 = 64;

impl EgressState {
    pub const fn new() -> Self {
        Self {
            entity_id: std::sync::atomic::AtomicI32::new(-1),
            shift: std::sync::atomic::AtomicBool::new(false),
            pending_chat: std::sync::atomic::AtomicI32::new(0),
        }
    }

    pub fn set_entity_id(&self, id: i32) {
        use std::sync::atomic::Ordering::Relaxed;
        self.entity_id.store(id, Relaxed);
        self.shift.store(false, Relaxed);
        // A new connection's chat listener has acknowledged nothing yet.
        self.pending_chat.store(0, Relaxed);
    }
}

impl Default for EgressState {
    fn default() -> Self {
        Self::new()
    }
}

static EGRESS_STATE: EgressState = EgressState::new();

/// Record the local player's entity id (called when the login packet is seen).
/// / RU: запомнить id сущности локального игрока (из login).
pub fn set_local_entity_id(id: i32) {
    EGRESS_STATE.set_entity_id(id);
}

/// A respawn or dimension change gives the server a fresh, non-sneaking
/// player: forget that shift was pressed, so the next input sends it again.
pub fn reset_local_sneak() {
    EGRESS_STATE
        .shift
        .store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Rewrite an egress packet body (serverbound) into the bodies to put on the
/// wire: usually one, none when the packet has no counterpart, two when the
/// wire protocol needs an extra packet (sneak press/release).
/// / RU: переписать исходящее тело в список тел для провода: обычно одно,
/// ни одного - если аналога нет, два - если нужен дополнительный пакет.
pub fn egress_all(body: &[u8]) -> Vec<Vec<u8>> {
    let Some(tr) = ACTIVE.read().unwrap().clone() else {
        return vec![body.to_vec()];
    };
    egress_with(&tr, &EGRESS_STATE, body)
}

fn with_id(wire: i32, tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tail.len() + 2);
    write_varint(&mut out, wire);
    out.extend_from_slice(tail);
    out
}

fn without_last(rest: &[u8]) -> &[u8] {
    &rest[..rest.len().saturating_sub(1)]
}

/// ContainerClick 777 -> 767: the hashed stacks cannot be turned back into
/// stacks, so every stack is written empty and the server resyncs the slots.
/// 777: [varint cid][varint state][i16 slot][i8 btn][varint mode][varint n]
/// [(i16 slot, HashedSlot) x n][HashedSlot carried]
/// 767: [u8 cid][varint state][i16 slot][i8 btn][varint mode][varint n]
/// [(i16 slot, 00) x n][00]
fn container_click_767(rest: &[u8]) -> Option<Vec<u8>> {
    let mut r = crate::buf::Reader::new(rest);
    let container = r.varint().ok()?;
    let state = r.varint().ok()?;
    let slot = r.i16().ok()?;
    let button = r.i8().ok()?;
    let mode = r.varint().ok()?;
    let n = r.count(128).ok()?;
    let mut changed = Vec::with_capacity(n);
    for _ in 0..n {
        changed.push(r.i16().ok()?);
        skip_hashed_slot(&mut r)?;
    }
    let mut out = vec![container as u8];
    write_varint(&mut out, state);
    out.extend_from_slice(&slot.to_be_bytes());
    out.push(button as u8);
    write_varint(&mut out, mode);
    write_varint(&mut out, n as i32);
    for s in changed {
        out.extend_from_slice(&s.to_be_bytes());
        out.push(0);
    }
    out.push(0);
    Some(out)
}

fn skip_hashed_slot(r: &mut crate::buf::Reader) -> Option<()> {
    if !r.bool().ok()? {
        return Some(());
    }
    r.varint().ok()?;
    r.varint().ok()?;
    for _ in 0..r.count(256).ok()? {
        r.varint().ok()?;
        r.i32().ok()?;
    }
    for _ in 0..r.count(256).ok()? {
        r.varint().ok()?;
    }
    Some(())
}

/// SetCreativeModeSlot 777 -> 767 for a stack with no component patch:
/// [i16 slot][varint count][varint item][00][00] (empty stack: [i16][00]).
/// A stack with a patch (enchantments, a name, damage...) cannot be
/// re-encoded for 767 here, and the server installs what it is sent as given,
/// so that packet is dropped (None) and the server's copy stays intact.
fn creative_slot_767(rest: &[u8]) -> Option<Vec<u8>> {
    let mut r = crate::buf::Reader::new(rest);
    let slot = r.i16().ok()?;
    let count = r.varint().ok()?;
    let mut out = slot.to_be_bytes().to_vec();
    if count <= 0 {
        out.push(0);
        return Some(out);
    }
    let item = r.varint().ok()?;
    let added = r.varint().ok()?;
    let removed = r.varint().ok()?;
    if added != 0 || removed != 0 {
        return None;
    }
    write_varint(&mut out, count);
    write_varint(&mut out, item);
    out.extend_from_slice(&[0, 0]);
    Some(out)
}

/// The 767 INTERACT_AT that precedes every entity interaction in 1.21.1
/// (armor stands and other `interactAt` targets react to it):
/// [varint id][varint 2][f32 x y z][varint hand][u8 sneaking], the location
/// being the decoded 777 LpVec3 (relative to the entity in both versions).
fn interact_at_767(wire: i32, rest: &[u8], sneaking: bool) -> Option<Vec<u8>> {
    use crate::buf::{Codec, LpVec3};
    let mut r = crate::buf::Reader::new(rest);
    let entity = r.varint().ok()?;
    let hand = r.varint().ok()?;
    let at = <LpVec3 as Codec<[f64; 3]>>::read(&mut r).ok()?;
    let mut out = Vec::with_capacity(24);
    write_varint(&mut out, wire);
    write_varint(&mut out, entity);
    write_varint(&mut out, 2);
    for c in at {
        out.extend_from_slice(&(c as f32).to_be_bytes());
    }
    write_varint(&mut out, hand);
    out.push(u8::from(sneaking));
    Some(out)
}

/// 767 learns about sneaking from PRESS_SHIFT/RELEASE_SHIFT player_commands.
fn sneak_command(tr: &Translation, entity_id: i32, shift: bool) -> Option<Vec<u8>> {
    let wire = (tr.egress)(crate::ids::play::serverbound::PLAYER_COMMAND)?;
    let mut out = Vec::with_capacity(8);
    write_varint(&mut out, wire);
    write_varint(&mut out, entity_id);
    out.push(u8::from(!shift));
    out.push(0);
    Some(out)
}

/// The packets for the wire, led by a chat_ack once enough signed chat lines
/// are unacknowledged (767 disconnects past its pending limit).
fn egress_with(tr: &Translation, st: &EgressState, body: &[u8]) -> Vec<Vec<u8>> {
    use std::sync::atomic::Ordering::Relaxed;
    let mut out = egress_packet(tr, st, body);
    if out.is_empty() || st.pending_chat.load(Relaxed) < CHAT_ACK_BATCH {
        return out;
    }
    let pending = st.pending_chat.swap(0, Relaxed);
    if let Some(wire) = (tr.egress)(crate::ids::play::serverbound::CHAT_ACK) {
        let mut tail = Vec::with_capacity(3);
        write_varint(&mut tail, pending);
        out.insert(0, with_id(wire, &tail));
    }
    out
}

fn egress_packet(tr: &Translation, st: &EgressState, body: &[u8]) -> Vec<Vec<u8>> {
    use crate::ids::play::serverbound as sb;
    use std::sync::atomic::Ordering::Relaxed;
    let Some((id, k)) = read_varint(body) else {
        return Vec::new();
    };
    let Some(wire) = (tr.egress)(id) else {
        return Vec::new();
    };
    let rest = &body[k..];
    let single = |tail: &[u8]| vec![with_id(wire, tail)];
    match id {
        // The 767 confirm is the varint teleport id alone.
        sb::ACCEPT_TELEPORTATION => single(&rest[..varint_len_at(rest).min(rest.len())]),
        // 777 [u8 flags: fwd 1, back 2, left 4, right 8, jump 16, shift 32];
        // 767 [f32 left-right][f32 fwd-back][u8 jump | shift << 1].
        sb::PLAYER_INPUT => {
            let flags = rest.first().copied().unwrap_or(0);
            let axis =
                |plus: u8, minus: u8| f32::from(flags & plus != 0) - f32::from(flags & minus != 0);
            let shift = flags & 0x20 != 0;
            let mut tail = Vec::with_capacity(9);
            tail.extend_from_slice(&axis(0x04, 0x08).to_be_bytes());
            tail.extend_from_slice(&axis(0x01, 0x02).to_be_bytes());
            tail.push(u8::from(flags & 0x10 != 0) | u8::from(shift) << 1);
            let mut out = Vec::with_capacity(2);
            let entity_id = st.entity_id.load(Relaxed);
            if st.shift.swap(shift, Relaxed) != shift && entity_id >= 0 {
                out.extend(sneak_command(tr, entity_id, shift));
            }
            out.push(with_id(wire, &tail));
            out
        }
        // 767 attack = interact [id][1][sneaking]; the server applies the
        // flag with setShiftKeyDown, so it must be the real shift state.
        sb::ATTACK => {
            let Some((entity, _)) = read_varint(rest) else {
                return Vec::new();
            };
            let mut tail = Vec::with_capacity(8);
            write_varint(&mut tail, entity);
            write_varint(&mut tail, 1);
            tail.push(u8::from(st.shift.load(Relaxed)));
            single(&tail)
        }
        // 777 [id][hand][LpVec3][secondary] -> 767 plain interact [id][0][hand][sneaking].
        sb::INTERACT => {
            let Some((entity, n)) = read_varint(rest) else {
                return Vec::new();
            };
            let Some((hand, _)) = read_varint(&rest[n..]) else {
                return Vec::new();
            };
            let sneaking = rest.last().is_some_and(|&b| b != 0);
            let mut tail = Vec::with_capacity(10);
            write_varint(&mut tail, entity);
            write_varint(&mut tail, 0);
            write_varint(&mut tail, hand);
            tail.push(u8::from(sneaking));
            let mut out = Vec::with_capacity(2);
            out.extend(interact_at_767(wire, rest, sneaking));
            out.push(with_id(wire, &tail));
            out
        }
        // 767 has no game-mode packet: the vanilla command does the same.
        sb::CHANGE_GAME_MODE => {
            let mode = match read_varint(rest) {
                Some((0, _)) => "survival",
                Some((1, _)) => "creative",
                Some((2, _)) => "adventure",
                Some((3, _)) => "spectator",
                _ => return Vec::new(),
            };
            let mut w = crate::buf::Writer::new();
            w.string(&format!("gamemode {mode}"));
            single(&w.data)
        }
        // 777 punch (empty) -> 767 swing [hand 0].
        sb::PUNCH => single(&[0]),
        // The particle status varint at the end does not exist in 767.
        sb::CLIENT_INFORMATION if !rest.is_empty() => single(without_last(rest)),
        // [varint hand][i64 pos][varint face][f32 x3][bool inside] then the
        // 777 world-border bool, then the varint sequence.
        sb::USE_ITEM_ON => {
            let face_at = varint_len_at(rest) + 8;
            let drop_at = rest
                .get(face_at..)
                .map(|f| face_at + varint_len_at(f) + 12 + 1)
                .filter(|&at| at < rest.len());
            match drop_at {
                Some(at) => single(&[&rest[..at], &rest[at + 1..]].concat()),
                None => single(rest),
            }
        }
        // 777 Action adds CHANGE_DESTROY_DIRECTION (1) and STAB (8).
        sb::PLAYER_ACTION => {
            let Some((action, n)) = read_varint(rest) else {
                return Vec::new();
            };
            let mapped = match action {
                0 => 0,
                2..=7 => action - 1,
                _ => return Vec::new(),
            };
            let mut tail = Vec::with_capacity(rest.len());
            write_varint(&mut tail, mapped);
            tail.extend_from_slice(&rest[n..]);
            single(&tail)
        }
        // The 777 last-seen update ends with a checksum byte.
        sb::CHAT => single(without_last(rest)),
        // 777 appends an on-ground bool to the vehicle move.
        sb::MOVE_VEHICLE => single(without_last(rest)),
        sb::CONTAINER_CLICK => container_click_767(rest).map_or_else(Vec::new, |t| single(&t)),
        // 767 has PRESS_SHIFT/RELEASE_SHIFT first: every other action is +2.
        sb::PLAYER_COMMAND => {
            let Some((entity, n)) = read_varint(rest) else {
                return Vec::new();
            };
            let Some((action, m)) = read_varint(&rest[n..]) else {
                return Vec::new();
            };
            let mut tail = Vec::with_capacity(rest.len());
            write_varint(&mut tail, entity);
            write_varint(&mut tail, action + 2);
            tail.extend_from_slice(&rest[n + m..]);
            single(&tail)
        }
        // 777 [pos][4 lines][bool front]; 767 [pos][bool front][4 lines].
        sb::SIGN_UPDATE if rest.len() > 8 => {
            let mut tail = rest[..8].to_vec();
            tail.push(rest[rest.len() - 1]);
            tail.extend_from_slice(&rest[8..rest.len() - 1]);
            single(&tail)
        }
        sb::SET_CREATIVE_MODE_SLOT => creative_slot_767(rest).map_or_else(Vec::new, |t| single(&t)),
        // 767 knows only RESPAWN (0) and REQUEST_STATS (1).
        sb::CLIENT_COMMAND => match read_varint(rest) {
            Some((action, _)) if action < 2 => single(rest),
            _ => Vec::new(),
        },
        // Recipe display ids have no 767 equivalent (it sends a resource location).
        sb::PLACE_RECIPE | sb::RECIPE_BOOK_SEEN_RECIPE => Vec::new(),
        // 777 flag bit 1 is horizontal collision; 767 reads the byte as on-ground.
        sb::MOVE_PLAYER_POS
        | sb::MOVE_PLAYER_POS_ROT
        | sb::MOVE_PLAYER_ROT
        | sb::MOVE_PLAYER_STATUS_ONLY => {
            let mut tail = rest.to_vec();
            if let Some(flags) = tail.last_mut() {
                *flags &= 1;
            }
            single(&tail)
        }
        _ => single(rest),
    }
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
    // Internal packet id for the slot-based action removed before protocol 777.
    const PICK_INVENTORY_SLOT: i32 = 0x1_0000;

    pub fn pick_inventory_slot(slot: i32) -> crate::packets::play::Serverbound {
        let mut w = crate::buf::Writer::new();
        w.varint(slot);
        crate::packets::play::Serverbound::Unknown(PICK_INVENTORY_SLOT, w.data)
    }

    pub fn sb777_to_767(id: i32) -> Option<i32> {
        match id {
            // Generated from vanilla 1.21.1's packets report (--reports) and
            // ids.rs by packet name; 26.3 packets 767 lacks are dropped.
            0 => Some(0),   // accept_teleportation
            2 => Some(1),   // block_entity_tag_query
            4 => Some(2),   // change_difficulty
            5 => Some(4),   // change_game_mode: 767's chat_command (egress body)
            6 => Some(3),   // chat_ack
            7 => Some(4),   // chat_command
            8 => Some(5),   // chat_command_signed
            9 => Some(6),   // chat
            10 => Some(7),  // chat_session_update
            11 => Some(8),  // chunk_batch_received
            12 => Some(9),  // client_command
            14 => Some(10), // client_information
            15 => Some(11), // command_suggestion
            16 => Some(12), // configuration_acknowledged
            17 => Some(13), // container_button_click
            18 => Some(14), // container_click
            19 => Some(15), // container_close
            20 => Some(16), // container_slot_state_changed
            21 => Some(17), // cookie_response
            22 => Some(18), // custom_payload
            24 => Some(20), // edit_book
            25 => Some(21), // entity_tag_query
            1 => Some(22),  // attack: 767's interact, type attack (egress body)
            26 => Some(22), // interact
            46 => Some(54), // punch: 767's swing (egress body)
            27 => Some(23), // jigsaw_generate
            28 => Some(24), // keep_alive
            29 => Some(25), // lock_difficulty
            30 => Some(26), // move_player_pos
            31 => Some(27), // move_player_pos_rot
            32 => Some(28), // move_player_rot
            33 => Some(29), // move_player_status_only
            34 => Some(30), // move_vehicle
            35 => Some(31), // paddle_boat
            38 => Some(33), // ping_request
            39 => Some(34), // place_recipe
            40 => Some(35), // player_abilities
            41 => Some(36), // player_action
            42 => Some(37), // player_command
            43 => Some(38), // player_input
            45 => Some(39), // pong
            47 => Some(40), // recipe_book_change_settings
            48 => Some(41), // recipe_book_seen_recipe
            49 => Some(42), // rename_item
            50 => Some(43), // resource_pack
            51 => Some(44), // seen_advancements
            52 => Some(45), // select_trade
            53 => Some(46), // set_beacon
            54 => Some(47), // set_carried_item
            55 => Some(48), // set_command_block
            56 => Some(49), // set_command_minecart
            57 => Some(50), // set_creative_mode_slot
            59 => Some(51), // set_jigsaw_block
            60 => Some(52), // set_structure_block
            62 => Some(53), // sign_update
            64 => Some(55), // teleport_to_entity
            66 => Some(56), // use_item_on
            67 => Some(57), // use_item

            PICK_INVENTORY_SLOT => Some(32), // pick_item
            _ => None,
        }
    }

    /// Stand-in id for 767's add_experience_orb until its body is rewritten
    /// into an add_entity one (`super::native_id`); 9999 is "skipped".
    pub const EXPERIENCE_ORB: i32 = 9998;

    /// 767 wire id (bundle+delimiter occupy 0/1) -> 777 id, clientbound play.
    /// / RU: проводной id 767 -> id 777, clientbound play.
    /// 767 wire id (wire = index + 1: BUNDLE=0, delimiter=1) -> 777 id,
    /// clientbound play, built from vanilla 1.21.1 + Horizon 777.
    /// / RU: проводной id 767 -> id 777, clientbound play.
    pub fn cb(id: i32) -> i32 {
        match id {
            // Generated from vanilla 1.21.1's packets report (--reports) and
            // ids.rs by packet name (horse_screen_open is mount_screen_open,
            // set_carried_item set_held_slot); 9999: no 26.3 packet, skipped.
            0 => 0,              // bundle_delimiter
            1 => 1,              // add_entity
            2 => EXPERIENCE_ORB, // add_experience_orb: rewritten to add_entity
            3 => 2,              // animate
            4 => 3,              // award_stats
            5 => 4,              // block_changed_ack
            6 => 5,              // block_destruction
            7 => 6,              // block_entity_data
            8 => 7,              // block_event
            9 => 8,              // block_update
            10 => 9,             // boss_event
            11 => 10,            // change_difficulty
            12 => 11,            // chunk_batch_finished
            13 => 12,            // chunk_batch_start
            14 => 13,            // chunks_biomes
            15 => 14,            // clear_titles
            16 => 15,            // command_suggestions
            17 => 16,            // commands
            18 => 17,            // container_close
            19 => 18,            // container_set_content
            20 => 19,            // container_set_data
            21 => 20,            // container_set_slot
            22 => 21,            // cookie_request
            23 => 22,            // cooldown
            24 => 23,            // custom_chat_completions
            25 => 24,            // custom_payload
            26 => 25,            // damage_event
            27 => 30,            // debug_sample
            28 => 31,            // delete_chat
            29 => 32,            // disconnect
            30 => 33,            // disguised_chat
            31 => 34,            // entity_event
            32 => 36,            // explode
            33 => 38,            // forget_level_chunk
            34 => 39,            // game_event
            35 => 42,            // horse_screen_open
            36 => 43,            // hurt_animation
            37 => 44,            // initialize_border
            38 => 45,            // keep_alive
            39 => 46,            // level_chunk_with_light
            40 => 47,            // level_event
            41 => 48,            // level_particles
            42 => 49,            // light_update
            43 => 50,            // login
            44 => 52,            // map_item_data
            45 => 53,            // merchant_offers
            46 => 54,            // move_entity_pos
            47 => 55,            // move_entity_pos_rot
            48 => 57,            // move_entity_rot
            49 => 58,            // move_vehicle
            50 => 59,            // open_book
            51 => 60,            // open_screen
            52 => 61,            // open_sign_editor
            53 => 62,            // ping
            54 => 63,            // pong_response
            55 => 9999, // place_ghost_recipe: 767 sends a resource location, 26.3 a display
            56 => 65,   // player_abilities
            57 => 66,   // player_chat
            58 => 67,   // player_combat_end
            59 => 68,   // player_combat_enter
            60 => 69,   // player_combat_kill
            61 => 70,   // player_info_remove
            62 => 71,   // player_info_update
            63 => 72,   // player_look_at
            64 => 73,   // player_position
            65 => 9999, // recipe
            66 => 78,   // remove_entities
            67 => 79,   // remove_mob_effect
            68 => 80,   // reset_score
            69 => 81,   // resource_pack_pop
            70 => 82,   // resource_pack_push
            71 => 84,   // respawn
            72 => 85,   // rotate_head
            73 => 86,   // section_blocks_update
            74 => 87,   // select_advancements_tab
            75 => 88,   // server_data
            76 => 89,   // set_action_bar_text
            77 => 90,   // set_border_center
            78 => 91,   // set_border_lerp_size
            79 => 92,   // set_border_size
            80 => 93,   // set_border_warning_delay
            81 => 94,   // set_border_warning_distance
            82 => 95,   // set_camera
            83 => 107,  // set_carried_item
            84 => 96,   // set_chunk_cache_center
            85 => 97,   // set_chunk_cache_radius
            86 => 99,   // set_default_spawn_position
            87 => 100,  // set_display_objective
            88 => 101,  // set_entity_data
            89 => 102,  // set_entity_link
            90 => 103,  // set_entity_motion
            91 => 104,  // set_equipment
            92 => 105,  // set_experience
            93 => 106,  // set_health
            94 => 108,  // set_objective
            95 => 109,  // set_passengers
            96 => 111,  // set_player_team
            97 => 112,  // set_score
            98 => 113,  // set_simulation_distance
            99 => 114,  // set_subtitle_text
            100 => 115, // set_time
            101 => 116, // set_title_text
            102 => 117, // set_titles_animation
            103 => 118, // sound_entity
            104 => 119, // sound
            105 => 120, // start_configuration
            106 => 121, // stop_sound
            107 => 122, // store_cookie
            108 => 124, // system_chat
            109 => 125, // tab_list
            110 => 126, // tag_query
            111 => 127, // take_item_entity
            112 => 128, // teleport_entity
            113 => 130, // ticking_state
            114 => 131, // ticking_step
            115 => 132, // transfer
            116 => 133, // update_advancements
            117 => 134, // update_attributes
            118 => 135, // update_mob_effect
            119 => 9999, // update_recipes: 767 full recipes, 777 has only item sets
            120 => 137, // update_tags
            121 => 138, // projectile_power
            122 => 139, // custom_report_details
            123 => 140, // server_links
            _ => 9999,
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
            // PLAYER_POSITION: the 767 body is [x,y,z][yaw,pitch]
            /// [u8 flags][varint id]; the 777 shape is [varint id][pos]
            /// [delta][yaw,pitch][i32 flags]. Reorder into the native
            /// shape with a zero delta and the wire byte flags widened
            /// to i32.
            /// / RU: PLAYER_POSITION: тело 767 - [x,y,z][yaw,pitch]
            /// [u8 флаги][varint id]; формат 777 - [varint id][позиция]
            /// [дельта][yaw,pitch][i32 флаги]. Перестановка в нативный
            /// формат с нулевой дельтой и флагами, расширенными до i32.
            crate::ids::play::clientbound::PLAYER_POSITION => player_position_767_to_777(body),
            crate::ids::play::clientbound::MOVE_ENTITY_POS => {
                move_entity_767_to_777(body, true, false)
            }
            crate::ids::play::clientbound::MOVE_ENTITY_POS_ROT => {
                move_entity_767_to_777(body, true, true)
            }
            crate::ids::play::clientbound::MOVE_ENTITY_ROT => {
                move_entity_767_to_777(body, false, true)
            }
            crate::ids::play::clientbound::SET_ENTITY_MOTION => set_entity_motion_767_to_777(body),
            crate::ids::play::clientbound::TELEPORT_ENTITY => teleport_entity_767_to_777(body),
            crate::ids::play::clientbound::RESPAWN => {
                super::reset_local_sneak();
                respawn_767_to_777(body)
            }
            crate::ids::play::clientbound::CONTAINER_SET_SLOT => {
                container_set_slot_767_to_777(body)
            }
            crate::ids::play::clientbound::SET_DEFAULT_SPAWN_POSITION => {
                spawn_position_767_to_777(body)
            }
            // PLAYER_CHAT: 777 starts with a global index varint 767 lacks.
            crate::ids::play::clientbound::PLAYER_CHAT => Some(player_chat_767_to_777(body)),
            crate::ids::play::clientbound::LIGHT_UPDATE => Some(light_update_767_to_777(body)),
            crate::ids::play::clientbound::COOLDOWN => Some(cooldown_767_to_777(body)),
            crate::ids::play::clientbound::EXPLODE => Some(explode_767_to_777(body)),
            crate::ids::play::clientbound::SET_TIME => Some(set_time_767_to_777(body)),
            crate::ids::play::clientbound::SET_PLAYER_TEAM => {
                Some(set_player_team_767_to_777(body))
            }
            crate::ids::play::clientbound::LEVEL_CHUNK_WITH_LIGHT => {
                // Debug capture: full 767 chunk bodies for offline analysis.
                /// / RU: отладочный захват полных тел чанков 767.
                if std::env::var_os("HORIZON_DUMP_CHUNKS").is_some()
                    && let Ok(dir) = std::env::var("TEMP")
                {
                    use std::sync::atomic::{AtomicU32, Ordering};
                    static N: AtomicU32 = AtomicU32::new(0);
                    let n = N.fetch_add(1, Ordering::Relaxed);
                    if n < 3 {
                        let _ = std::fs::write(format!("{}\\horizon-chunk-767-{n}.bin", dir), body);
                    }
                }
                Some(heightmaps_nbt_to_list(body))
            }
            // ADD_ENTITY: the 767 velocity is three packed shorts at the
            /// tail; 777 carries a low-precision packed Vec3 right after
            /// the position.
            /// / RU: у 767 скорость - три short'а в хвосте; у 777 -
            /// упакованный LpVec3 сразу после позиции.
            crate::ids::play::clientbound::ADD_ENTITY => Some(add_entity_velocity(body)),
            // 767 add_experience_orb [varint id][dbl x y z][short count]
            // becomes an add_entity of the server's experience_orb type.
            EXPERIENCE_ORB => experience_orb_767_to_777(body),
            // LOGIN: 767 ends with one bool (enforcesSecure); 777 has two
            /// (online + secure) and a sea_level in SpawnInfo. Insert the
            /// online-mode bool before the trailing one and the sea level
            /// after the SpawnInfo's portal cooldown.
            /// / RU: 767 заканчивается одним bool; у 777 их два и в SpawnInfo
            /// есть sea_level. Вставляем недостающие поля.
            crate::ids::play::clientbound::LOGIN => Some(login_767_to_777(body)),
            // COMMANDS: node layout identical, argument type ids remapped
            /// (hex_color and resource_selector inserted; uuid 53 -> 61).
            /// / RU: структура узлов та же, id типов аргументов сдвигаются.
            crate::ids::play::clientbound::COMMANDS => Some(commands_767_to_777(body)),
            // UPDATE_ADVANCEMENTS: the icon stack's count/item varints
            /// swap, x/y move to the advancement tail, trailing bool added.
            /// / RU: у icon меняются местами count/item, x/y переезжают
            /// в хвост, добавляется bool в конец.
            crate::ids::play::clientbound::UPDATE_ADVANCEMENTS => {
                Some(update_advancements_767_to_777(body))
            }
            // UPDATE_ATTRIBUTES: holder (id+1) -> plain registry id.
            /// / RU: holder (id+1) -> плоский id реестра.
            crate::ids::play::clientbound::UPDATE_ATTRIBUTES => {
                Some(update_attributes_767_to_777(body))
            }
            // Item stacks, entity data and particles: 1.21.1 component,
            // serializer and particle ids and codecs into 26.3's
            // (item::v767, entity_data::v767).
            crate::ids::play::clientbound::CONTAINER_SET_CONTENT => {
                Some(container_set_content_767_to_777(body))
            }
            crate::ids::play::clientbound::SET_EQUIPMENT => Some(set_equipment_767_to_777(body)),
            crate::ids::play::clientbound::MERCHANT_OFFERS => {
                Some(merchant_offers_767_to_777(body))
            }
            crate::ids::play::clientbound::SET_ENTITY_DATA => {
                Some(set_entity_data_767_to_777(body))
            }
            crate::ids::play::clientbound::LEVEL_PARTICLES => {
                Some(level_particles_767_to_777(body))
            }
            _ => None,
        }
    }

    /// PlayerPosition 767 -> 777, parsed by field (the teleport id varint
    /// grows past 127 with every correction).
    /// 767: [f64 x y z][f32 yaw pitch][u8 flags][varint teleport id]
    /// 777: [varint id][f64 x y z][f64 dx dy dz][f32 yaw pitch][i32 flags]
    /// The relative X/Y/Z bits also set the DELTA bits (5-7) so velocity
    /// stays on relative axes, as 1.21.1 did.
    fn player_position_767_to_777(body: &[u8]) -> Option<Vec<u8>> {
        let flags = i32::from(*body.get(32)?);
        let (_, id_len) = super::read_varint(body.get(33..)?)?;
        let mut out = Vec::with_capacity(body.len() + 15);
        out.extend_from_slice(&body[33..33 + id_len]);
        out.extend_from_slice(&body[..24]);
        out.extend_from_slice(&[0u8; 24]);
        out.extend_from_slice(&body[24..32]);
        out.extend_from_slice(&(flags | (flags & 7) << 5).to_be_bytes());
        Some(out)
    }

    /// MoveEntity 767 -> 777: move the trailing onGround before the delta/rotation.
    /// Pos/PosRot encode it as bit 0 of the properties VarInt, with zero steps
    /// (a linear VecDelta). The three 1/4096-block shorts stay unchanged.
    /// Rot has no properties VarInt: it starts with [id][bool][yaw][pitch].
    fn move_entity_767_to_777(body: &[u8], pos: bool, rot: bool) -> Option<Vec<u8>> {
        let mut r = crate::buf::Reader::new(body);
        let id = r.varint().ok()?;
        let delta = r.bytes(if pos { 6 } else { 0 }).ok()?;
        let rotation = r.bytes(if rot { 2 } else { 0 }).ok()?;
        let on_ground = r.bool().ok()?;
        let mut w = crate::buf::Writer::new();
        w.varint(id);
        if pos {
            w.varint(i32::from(on_ground)); // bit 0: onGround; bits 1+: zero steps
        } else {
            w.bool(on_ground);
        }
        w.bytes(delta);
        w.bytes(rotation);
        Some(w.data)
    }

    /// SetEntityMotion 767 -> 777: three shorts (1/8000 block per tick) become an LpVec3.
    fn set_entity_motion_767_to_777(body: &[u8]) -> Option<Vec<u8>> {
        let mut r = crate::buf::Reader::new(body);
        let id = r.varint().ok()?;
        let mut velocity = [0f64; 3];
        for v in velocity.iter_mut() {
            *v = f64::from(r.i16().ok()?) / 8000.0;
        }
        let mut out = Vec::with_capacity(12);
        super::write_varint(&mut out, id);
        write_lp_vec3(&mut out, velocity);
        Some(out)
    }

    /// TeleportEntity 767 -> 777. 767: [varint id][f64 x y z][i8 yaw][i8 pitch]
    /// [bool onGround]; bytes a server mod appends after it are ignored.
    /// 777: [varint id][f64 x y z][f64 dx dy dz][f32 yaw pitch][i32 relatives][bool].
    /// Relatives 0xE0 (the DELTA bits) keep the client's velocity.
    fn teleport_entity_767_to_777(body: &[u8]) -> Option<Vec<u8>> {
        let mut r = crate::buf::Reader::new(body);
        let id = r.varint().ok()?;
        let mut out = Vec::with_capacity(64);
        super::write_varint(&mut out, id);
        for _ in 0..3 {
            out.extend_from_slice(&r.f64().ok()?.to_be_bytes());
        }
        let yaw = r.i8().ok()?;
        let pitch = r.i8().ok()?;
        let on_ground = r.bool().ok()?;
        out.extend_from_slice(&[0u8; 24]);
        for angle in [yaw, pitch] {
            out.extend_from_slice(&(f32::from(angle) * 360.0 / 256.0).to_be_bytes());
        }
        out.extend_from_slice(&0xE0i32.to_be_bytes());
        out.push(u8::from(on_ground));
        Some(out)
    }

    /// Respawn 767 -> 777: the CommonPlayerSpawnInfo rewrite of the login
    /// packet (game types are bytes in 767, previous is OptVarInt in 777,
    /// 777 adds sea_level), followed by the data-to-keep byte.
    fn respawn_767_to_777(body: &[u8]) -> Option<Vec<u8>> {
        let mut r = crate::buf::Reader::new(body);
        let mut w = crate::buf::Writer::new();
        w.varint(r.varint().ok()?);
        write_string(&mut w, &r.string(32767).ok()?);
        w.i64(r.i64().ok()?);
        w.varint(i32::from(r.i8().ok()?));
        w.varint(i32::from(r.i8().ok()?) + 1);
        w.bool(r.bool().ok()?);
        w.bool(r.bool().ok()?);
        match read_opt_global_pos(&mut r).ok()? {
            Some((dim, pos)) => {
                w.bool(true);
                write_string(&mut w, &dim);
                w.i64(crate::buf::pack_block_pos(pos));
            }
            None => w.bool(false),
        }
        w.varint(r.varint().ok()?);
        w.varint(63); // sea level
        w.data.push(r.i8().ok()? as u8);
        Some(w.data)
    }

    /// ContainerSetSlot 767 -> 777: the container id is a signed byte
    /// (-1 cursor, -2 player inventory) where 777 has a varint, and the
    /// stack goes through the item rewrite. `None` when nothing changed.
    fn container_set_slot_767_to_777(body: &[u8]) -> Option<Vec<u8>> {
        fn set_slot(r: &mut crate::buf::Reader, w: &mut crate::buf::Writer) -> crate::Result<()> {
            w.varint(i32::from(r.i8()?));
            w.varint(r.varint()?);
            w.i16(r.i16()?);
            crate::item::v767::slot(r, w)
        }
        let mut r = crate::buf::Reader::new(body);
        let mut w = crate::buf::Writer::new();
        match set_slot(&mut r, &mut w) {
            Ok(()) => (w.data != body).then_some(w.data),
            Err(e) => Some(dropped("container_set_slot", &e)),
        }
    }

    /// A 767 body that couldn't be rewritten: neither shape may reach the
    /// 26.3 decoders, so an empty body (undecodable: the packet is skipped
    /// and counted) goes on, after one log line per reason.
    fn dropped(packet: &str, e: &crate::Error) -> Vec<u8> {
        super::warn_once(format!("Horizon: 1.21.1 {packet} dropped: {e}"));
        Vec::new()
    }

    /// ContainerSetContent 767 -> 777: the container id is an unsigned byte
    /// (a varint in 777) and every stack goes through the item rewrite. A
    /// stack that can't be read ends the list there: the slots before it
    /// are set, the rest keep what they held, the carried stack is emptied.
    fn container_set_content_767_to_777(body: &[u8]) -> Vec<u8> {
        fn head(r: &mut crate::buf::Reader, w: &mut crate::buf::Writer) -> crate::Result<usize> {
            w.varint(i32::from(r.u8()?));
            w.varint(r.varint()?);
            r.count(r.remaining())
        }
        let mut r = crate::buf::Reader::new(body);
        let mut w = crate::buf::Writer::new();
        let n = match head(&mut r, &mut w) {
            Ok(n) => n,
            Err(e) => return dropped("container_set_content", &e),
        };
        let mut items = crate::buf::Writer::new();
        let mut done = 0;
        let mut cut = None;
        for _ in 0..n {
            let mut stack = crate::buf::Writer::new();
            if let Err(e) = crate::item::v767::slot(&mut r, &mut stack) {
                cut = Some(e);
                break;
            }
            items.bytes(&stack.data);
            done += 1;
        }
        w.varint(done);
        w.bytes(&items.data);
        let mut carried = crate::buf::Writer::new();
        if cut.is_none()
            && let Err(e) = crate::item::v767::slot(&mut r, &mut carried)
        {
            cut = Some(e);
        }
        match cut {
            None => w.bytes(&carried.data),
            Some(e) => {
                super::warn_once(format!(
                    "Horizon: 1.21.1 container_set_content cut short (later slots keep their \
                     stacks): {e}"
                ));
                w.varint(0);
            }
        }
        w.data
    }

    /// SetEquipment 767 -> 777: the same (slot | more-bit, stack) list, each
    /// stack through the item rewrite. A stack that can't be read ends the
    /// list at the entry before it.
    fn set_equipment_767_to_777(body: &[u8]) -> Vec<u8> {
        let mut r = crate::buf::Reader::new(body);
        let id = match r.varint() {
            Ok(id) => id,
            Err(e) => return dropped("set_equipment", &e),
        };
        let mut entries = Vec::new();
        let cut = loop {
            let slot = match r.u8() {
                Ok(slot) => slot,
                Err(e) => break Some(e),
            };
            let mut stack = crate::buf::Writer::new();
            if let Err(e) = crate::item::v767::slot(&mut r, &mut stack) {
                break Some(e);
            }
            entries.push((slot & 127, stack.data));
            if slot & 128 == 0 {
                break None;
            }
        };
        if let Some(e) = cut {
            if entries.is_empty() {
                return dropped("set_equipment", &e);
            }
            super::warn_once(format!("Horizon: 1.21.1 set_equipment cut short: {e}"));
        }
        let mut w = crate::buf::Writer::new();
        w.varint(id);
        let last = entries.len() - 1;
        for (i, (slot, stack)) in entries.iter().enumerate() {
            w.u8(if i < last { slot | 128 } else { *slot });
            w.bytes(stack);
        }
        w.data
    }

    /// MerchantOffers 767 -> 777: the same layout, each offer's costs
    /// (ItemCost: item, count, exact components) and result stack through
    /// the item rewrite. An offer that can't be read ends the list there;
    /// the villager level and xp after the list are then unknown (0, no
    /// progress bar).
    fn merchant_offers_767_to_777(body: &[u8]) -> Vec<u8> {
        use crate::buf::{Reader, Writer};
        fn cost(r: &mut Reader, w: &mut Writer) -> crate::Result<()> {
            w.varint(r.varint()?);
            w.varint(r.varint()?);
            crate::item::v767::exact_components(r, w)
        }
        fn offer(r: &mut Reader, w: &mut Writer) -> crate::Result<()> {
            cost(r, w)?;
            crate::item::v767::slot(r, w)?;
            let cost_b = r.bool()?;
            w.bool(cost_b);
            if cost_b {
                cost(r, w)?;
            }
            // outOfStock, uses, maxUses, xp, specialPrice, priceMultiplier, demand.
            let start = r.position();
            r.bool()?;
            for _ in 0..4 {
                r.i32()?;
            }
            r.f32()?;
            r.i32()?;
            w.bytes(r.since(start));
            Ok(())
        }
        fn head(r: &mut Reader) -> crate::Result<(i32, usize)> {
            Ok((r.varint()?, r.count(r.remaining())?))
        }
        // villagerLevel, villagerXp, showProgress, canRestock.
        fn tail(r: &mut Reader) -> crate::Result<()> {
            r.varint()?;
            r.varint()?;
            r.bool()?;
            r.bool()?;
            Ok(())
        }
        let mut r = Reader::new(body);
        let (container, n) = match head(&mut r) {
            Ok(head) => head,
            Err(e) => return dropped("merchant_offers", &e),
        };
        let mut offers = Writer::new();
        let mut done = 0;
        let mut cut = None;
        for _ in 0..n {
            let mut one = Writer::new();
            if let Err(e) = offer(&mut r, &mut one) {
                cut = Some(e);
                break;
            }
            offers.bytes(&one.data);
            done += 1;
        }
        let start = r.position();
        let rest = match cut {
            Some(e) => Err(e),
            None => tail(&mut r),
        };
        let mut w = Writer::new();
        w.varint(container);
        w.varint(done);
        w.bytes(&offers.data);
        match rest {
            Ok(()) => w.bytes(r.since(start)),
            Err(e) => {
                super::warn_once(format!("Horizon: 1.21.1 merchant_offers cut short: {e}"));
                w.varint(0);
                w.varint(0);
                w.bool(false);
                w.bool(false);
            }
        }
        w.data
    }

    /// SetEntityData 767 -> 777: the entity id, then the values with
    /// 26.3's serializer ids and codecs (entity_data::v767).
    fn set_entity_data_767_to_777(body: &[u8]) -> Vec<u8> {
        let mut r = crate::buf::Reader::new(body);
        let mut w = crate::buf::Writer::new();
        match r.varint() {
            Ok(id) => w.varint(id),
            Err(e) => return dropped("set_entity_data", &e),
        }
        crate::entity_data::v767::values(&mut r, &mut w);
        w.data
    }

    /// LevelParticles 767 -> 777. 767: [bool force][f64 x y z][f32 dx dy dz]
    /// [f32 maxSpeed][i32 count][particle]; 777: [particle][bool
    /// overrideLimiter][bool alwaysShow][f64 x y z][f32 dx dy dz][f32 speed
    /// x y z][varint count][varint randomization]. 1.21.1's force did what
    /// both 777 flags do; its one speed goes on every axis, as
    /// ServerLevel.sendParticles does.
    pub(super) fn level_particles_with_names(
        body: &[u8],
        name: impl Fn(&str, i32) -> Option<String>,
    ) -> crate::Result<Vec<u8>> {
        use crate::entity_data::CBC_BREECH_SMOKE;
        use crate::entity_data::{
            BlastWave, CBC_AUTOCANNON_PLUME, CBC_BIG_CANNON_PLUME, CBC_CANNON_BLAST_WAVE,
            CBC_DROP_MORTAR_PLUME, CBC_SHELL_BLAST_WAVE, Particle, ParticleData,
        };
        // A plume carrying its scale alone (`AutocannonPlumeParticleData`,
        // `DropMortarPlumeParticleData`).
        let scaled = |id, data: &mut crate::buf::Reader| -> crate::Result<Particle> {
            Ok(Particle {
                id,
                data: Some(ParticleData::CannonPlume {
                    size: data.f32()?,
                    power: 0.0,
                    lifetime: 0,
                }),
            })
        };
        // `AbstractBlastWaveEffectParticleData.createStreamCodec`: the sound is
        // a `Holder<SoundEvent>` (registry id + 1, or 0 and a direct event).
        let blast = |id, data: &mut crate::buf::Reader| -> crate::Result<Particle> {
            let blast_radius = data.f64()?;
            let (sound, fixed_range) = match data.varint()? {
                0 => {
                    let sound = data.string(32767)?;
                    let range = if data.bool()? {
                        Some(data.f32()?)
                    } else {
                        None
                    };
                    (sound, range)
                }
                holder => (
                    name("minecraft:sound_event", holder - 1)
                        .ok_or_else(|| crate::Error::new("unknown blast wave sound"))?,
                    None,
                ),
            };
            Ok(Particle {
                id,
                data: Some(ParticleData::BlastWave(BlastWave {
                    blast_radius,
                    sound,
                    fixed_range,
                    source: data.varint()?,
                    volume: data.f32()?,
                    pitch: data.f32()?,
                    air_absorption: data.f32()?,
                    power: data.f32()?,
                })),
            })
        };
        let mut r = crate::buf::Reader::new(body);
        let force = r.bool()?;
        let pos = [r.f64()?, r.f64()?, r.f64()?];
        let dist = [r.f32()?, r.f32()?, r.f32()?];
        let speed = r.f32()?;
        let count = r.i32()?;
        let mut w = crate::buf::Writer::new();
        let rest = r.rest();
        let mut data = crate::buf::Reader::new(rest);
        let id = data.varint()?;
        let cbc = match name("minecraft:particle_type", id).as_deref() {
            Some("createbigcannons:big_cannon_plume") => Some(Particle {
                id: CBC_BIG_CANNON_PLUME,
                data: Some(ParticleData::CannonPlume {
                    size: data.f32()?,
                    power: data.f32()?,
                    lifetime: data.varint()?,
                }),
            }),
            Some("createbigcannons:autocannon_plume") => {
                Some(scaled(CBC_AUTOCANNON_PLUME, &mut data)?)
            }
            Some("createbigcannons:drop_mortar_plume") => {
                Some(scaled(CBC_DROP_MORTAR_PLUME, &mut data)?)
            }
            Some("createbigcannons:quick_firing_breech_smoke") => Some(Particle {
                id: CBC_BREECH_SMOKE,
                data: None,
            }),
            Some("createbigcannons:cannon_blast_wave") => {
                Some(blast(CBC_CANNON_BLAST_WAVE, &mut data)?)
            }
            Some("createbigcannons:shell_blast_wave") => {
                Some(blast(CBC_SHELL_BLAST_WAVE, &mut data)?)
            }
            _ => None,
        };
        match cbc {
            Some(particle) => {
                if data.remaining() != 0 {
                    return Err(crate::Error::new("unexpected bytes after CBC particle"));
                }
                particle.write(&mut w);
            }
            None => crate::entity_data::v767::trailing_particle(rest, &mut w)?,
        }
        w.bool(force);
        w.bool(force);
        pos.iter().for_each(|v| w.f64(*v));
        dist.iter().for_each(|v| w.f32(*v));
        (0..3).for_each(|_| w.f32(speed));
        w.varint(count);
        w.varint(0);
        Ok(w.data)
    }

    fn level_particles_767_to_777(body: &[u8]) -> Vec<u8> {
        level_particles_with_names(body, super::server_name)
            .unwrap_or_else(|e| dropped("level_particles", &e))
    }

    /// SetDefaultSpawnPosition 767 -> 777: [i64 pos][f32 angle] gains the
    /// overworld dimension in front and a zero pitch at the end.
    fn spawn_position_767_to_777(body: &[u8]) -> Option<Vec<u8>> {
        let tail = body.get(..12)?;
        let mut w = crate::buf::Writer::new();
        write_string(&mut w, "minecraft:overworld");
        let mut out = w.data;
        out.extend_from_slice(tail);
        out.extend_from_slice(&0f32.to_be_bytes());
        Some(out)
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

    fn experience_orb_767_to_777(body: &[u8]) -> Option<Vec<u8>> {
        experience_orb_with_kind(
            body,
            super::server_id("minecraft:entity_type", "minecraft:experience_orb")?,
        )
    }

    pub(super) fn experience_orb_with_kind(body: &[u8], kind: i32) -> Option<Vec<u8>> {
        use crate::buf::{Reader, Writer};
        let mut r = Reader::new(body);
        let id = r.varint().ok()?;
        let xyz = [r.f64().ok()?, r.f64().ok()?, r.f64().ok()?];
        let mut w = Writer::new();
        w.varint(id);
        // Orbs have no uuid on the wire: derive a stable one from the id.
        w.uuid((0x6F72_6200u128 << 64) | u128::from(id as u32));
        w.varint(kind);
        xyz.iter().for_each(|&c| w.f64(c));
        write_lp_vec3(&mut w.data, [0.0; 3]);
        w.bytes(&[0, 0, 0]); // pitch, yaw, head yaw
        w.varint(0);
        Some(w.data)
    }

    fn write_lp_vec3(out: &mut Vec<u8>, v: [f64; 3]) {
        let unpack = |b: u64| ((b & 32767) as f64).min(32766.0) * 2.0 / 32766.0 - 1.0;
        let pack = |v: f64| ((v * 0.5 + 0.5) * 32766.0).round() as u64;
        let [x, y, z] = v.map(|c| {
            if c.is_nan() {
                0.0
            } else {
                c.clamp(-1.717_986_918_3e10, 1.717_986_918_3e10)
            }
        });
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

    /// PlayerChat 767 -> 777: 777 leads with a global index 767 lacks. A
    /// signed line also counts toward the next chat_ack.
    fn player_chat_767_to_777(body: &[u8]) -> Vec<u8> {
        // [uuid][varint index][bool has signature]...
        let signed = body
            .get(16..)
            .is_some_and(|b| b.get(super::varint_len_at(b)).is_some_and(|&has| has != 0));
        if signed {
            super::EGRESS_STATE
                .pending_chat
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        [&[0u8][..], body].concat()
    }

    /// LightUpdate 767 -> 777: [varint x][varint z] then the light data,
    /// whose four bit sets are long arrays in 767 and byte arrays in 777.
    fn light_update_767_to_777(body: &[u8]) -> Vec<u8> {
        let mut r = crate::buf::Reader::new(body);
        let (Ok(x), Ok(z)) = (r.varint(), r.varint()) else {
            return Vec::new();
        };
        match repack_light_data_767(&body[r.position()..]) {
            Some(light) => {
                let mut w = crate::buf::Writer::new();
                w.varint(x);
                w.varint(z);
                w.bytes(&light);
                w.data
            }
            None => dropped("light_update", &crate::Error::new("unreadable light data")),
        }
    }

    /// Cooldown 767 -> 777: the item (plain registry id) becomes its
    /// identifier, the 777 cooldown group; the ticks stay.
    fn cooldown_767_to_777(body: &[u8]) -> Vec<u8> {
        let mut r = crate::buf::Reader::new(body);
        let (Ok(item), Ok(ticks)) = (r.varint(), r.varint()) else {
            return Vec::new();
        };
        let Some(name) = super::server_name("minecraft:item", item) else {
            super::unknown_767("minecraft:item", item);
            return Vec::new();
        };
        let mut w = crate::buf::Writer::new();
        w.string(&name);
        w.varint(ticks);
        w.data
    }

    /// Explode 767 -> 777. 767: [f64 x y z][f32 power][varint n][n x 3
    /// i8][f32 kx ky kz][varint blockInteraction][particle small][particle
    /// large][sound holder]; 777: [f64 x y z][f32 radius][i32 n][opt f64
    /// vec3][particle][sound holder][varint 0 block particles][bool sound].
    /// 1.21.1's client chose the large particle for power >= 2 when blocks
    /// are affected; the knockback floats widen to the optional vec3.
    fn explode_767_to_777(body: &[u8]) -> Vec<u8> {
        fn convert(r: &mut crate::buf::Reader) -> crate::Result<Vec<u8>> {
            let center = [r.f64()?, r.f64()?, r.f64()?];
            let power = r.f32()?;
            let blocks = r.count(r.remaining())?;
            r.bytes(blocks * 3)?;
            let knockback = [r.f32()?, r.f32()?, r.f32()?];
            let interaction = r.varint()?;
            let mut small = crate::buf::Writer::new();
            crate::entity_data::v767::particle(r, &mut small)?;
            let mut large = crate::buf::Writer::new();
            crate::entity_data::v767::particle(r, &mut large)?;
            let mut w = crate::buf::Writer::new();
            center.iter().for_each(|v| w.f64(*v));
            w.f32(power);
            w.i32(blocks as i32);
            w.bool(true);
            knockback.iter().for_each(|v| w.f64(f64::from(*v)));
            let large_blast = power >= 2.0 && interaction != 0;
            w.bytes(if large_blast {
                &large.data
            } else {
                &small.data
            });
            // The sound holder: [varint id + 1] or [0][string][opt f32].
            let sound = r.position();
            if r.varint()? == 0 {
                r.string(32767)?;
                if r.bool()? {
                    r.f32()?;
                }
            }
            w.bytes(r.since(sound));
            w.varint(0);
            w.bool(true);
            Ok(w.data)
        }
        convert(&mut crate::buf::Reader::new(body)).unwrap_or_else(|e| dropped("explode", &e))
    }

    /// The client's overworld world_clock registry id: 767 has no clocks, and
    /// the client falls back to the first clock it is sent.
    const OVERWORLD_CLOCK: i32 = 0;

    /// SetTime 767 -> 777: [i64 game][i64 day time] (negative day time = the
    /// clock is stopped) becomes the game time and one overworld clock.
    fn set_time_767_to_777(body: &[u8]) -> Vec<u8> {
        let mut r = crate::buf::Reader::new(body);
        let (Ok(game), Ok(day)) = (r.i64(), r.i64()) else {
            return Vec::new();
        };
        let mut w = crate::buf::Writer::new();
        w.i64(game);
        w.varint(1);
        w.varint(OVERWORLD_CLOCK);
        w.varlong(day.wrapping_abs());
        w.f32(0.0);
        w.f32(if day < 0 { 0.0 } else { 1.0 });
        w.data
    }

    /// SetPlayerTeam 767 -> 777 for methods 0 and 2. 767: [string name][i8
    /// method][nbt display][i8 options][string visibility][string
    /// collision][varint colour][nbt prefix][nbt suffix]; 777: name, method,
    /// display, prefix, suffix, varint visibility, varint collision,
    /// optional colour, options. The players list (methods 0, 3, 4) follows
    /// unchanged; the other methods are the same in both.
    fn set_player_team_767_to_777(body: &[u8]) -> Vec<u8> {
        fn convert(body: &[u8]) -> crate::Result<Vec<u8>> {
            let mut r = crate::buf::Reader::new(body);
            let mut w = crate::buf::Writer::new();
            let name = r.string(32767)?;
            let method = r.i8()?;
            w.string(&name);
            w.i8(method);
            if method != 0 && method != 2 {
                w.bytes(r.rest());
                return Ok(w.data);
            }
            let display = r.nbt()?;
            let options = r.i8()?;
            let visibility = r.string(40)?;
            let collision = r.string(40)?;
            let colour = r.varint()?;
            let prefix = r.nbt()?;
            let suffix = r.nbt()?;
            w.nbt(&display);
            w.nbt(&prefix);
            w.nbt(&suffix);
            w.varint(match visibility.as_str() {
                "never" => 1,
                "hideForOtherTeams" => 2,
                "hideForOwnTeam" => 3,
                _ => 0,
            });
            w.varint(match collision.as_str() {
                "never" => 1,
                "pushOtherTeams" => 2,
                "pushOwnTeam" => 3,
                _ => 0,
            });
            w.bool((0..=15).contains(&colour));
            if (0..=15).contains(&colour) {
                w.varint(colour);
            }
            w.i8(options);
            w.bytes(r.rest());
            Ok(w.data)
        }
        convert(body).unwrap_or_else(|e| dropped("set_player_team", &e))
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
        let reset = match r.bool() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        w.bool(reset);
        let n = match r.count(r.remaining()) {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        w.varint(n as i32);
        for _ in 0..n {
            if copy_ident(&mut r, &mut w).is_err() {
                return body.to_vec();
            }
            let has = match r.bool() {
                Ok(v) => v,
                Err(_) => return body.to_vec(),
            };
            w.bool(has);
            if has && copy_ident(&mut r, &mut w).is_err() {
                return body.to_vec();
            }
            let has_display = match r.bool() {
                Ok(v) => v,
                Err(_) => return body.to_vec(),
            };
            w.bool(has_display);
            let (x, y) = if has_display {
                if copy_nbt(&mut r, &mut w).is_err() {
                    return body.to_vec();
                }
                if copy_nbt(&mut r, &mut w).is_err() {
                    return body.to_vec();
                }
                // The icon: 767's ItemStack.STREAM_CODEC [count][item, a plain
                // registry id][patch] becomes 777's ItemStackTemplate
                // [item][count][patch] (item::v767). An icon that can't be
                // read leaves the rest of the packet unreadable.
                if let Err(e) = crate::item::v767::template(&mut r, &mut w) {
                    return dropped("update_advancements", &e);
                }
                let frame = match r.varint() {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
                w.varint(frame);
                let flags = match r.i32() {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
                w.i32(flags);
                if flags & 1 != 0 && copy_ident(&mut r, &mut w).is_err() {
                    return body.to_vec();
                }
                let x = match r.f32() {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
                let y = match r.f32() {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
                (x, y)
            } else {
                (0.0, 0.0)
            };
            // requirements: [varint groups][per: [varint k][k strings]]
            let groups = match r.count(r.remaining()) {
                Ok(v) => v,
                Err(_) => return body.to_vec(),
            };
            w.varint(groups as i32);
            for _ in 0..groups {
                let k = match r.count(r.remaining()) {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
                w.varint(k as i32);
                for _ in 0..k {
                    if copy_ident(&mut r, &mut w).is_err() {
                        return body.to_vec();
                    }
                }
            }
            let telemetry = match r.bool() {
                Ok(v) => v,
                Err(_) => return body.to_vec(),
            };
            w.bool(telemetry);
            w.f32(x);
            w.f32(y);
        }
        // removed
        let n = match r.count(r.remaining()) {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        w.varint(n as i32);
        for _ in 0..n {
            if copy_ident(&mut r, &mut w).is_err() {
                return body.to_vec();
            }
        }
        // progress
        let n = match r.count(r.remaining()) {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        w.varint(n as i32);
        for _ in 0..n {
            if copy_ident(&mut r, &mut w).is_err() {
                return body.to_vec();
            }
            let k = match r.count(r.remaining()) {
                Ok(v) => v,
                Err(_) => return body.to_vec(),
            };
            w.varint(k as i32);
            for _ in 0..k {
                if copy_ident(&mut r, &mut w).is_err() {
                    return body.to_vec();
                }
                let done = match r.bool() {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
                w.bool(done);
                if done {
                    let at = match r.i64() {
                        Ok(v) => v,
                        Err(_) => return body.to_vec(),
                    };
                    w.i64(at);
                }
            }
        }
        w.bool(true); // the 777-only trailing show
        w.data
    }

    /// `brigadier:string` and its GREEDY_PHRASE property.
    const STRING_ARGUMENT: i32 = 5;
    const GREEDY_PHRASE: i32 = 2;

    fn commands_767_to_777(body: &[u8]) -> Vec<u8> {
        commands_with_names(body, |id| {
            super::server_name("minecraft:command_argument_type", id)
        })
    }

    /// Consume a 767 ArgumentTypeInfo template before replacing its parser.
    /// None means the layout is unknown: templates have no length prefix, so
    /// guessing zero properties would corrupt suggestions and all later nodes.
    /// The inner None is a known mod template that becomes a greedy string.
    fn command_properties_767(
        r: &mut crate::buf::Reader<'_>,
        id: i32,
        names: &impl Fn(i32) -> Option<String>,
        depth: usize,
    ) -> crate::Result<Option<Option<i32>>> {
        if depth >= 64 {
            return Ok(None);
        }
        let name = names(id);
        let native = if let Some(name) = name.as_deref() {
            let name = if name == "minecraft:color" {
                "minecraft:team_color"
            } else {
                name
            };
            crate::packets::play::ARGUMENT_TYPES
                .iter()
                .position(|n| *n == name)
                .map(|i| i as i32)
        } else {
            match id {
                0..=16 => Some(id),
                17..=46 => Some(id + 1),
                47..=52 => Some(id + 2),
                53 => Some(61),
                _ => None,
            }
        };
        if let Some(native) = native {
            crate::packets::play::ArgumentProperties::read(r, native)?;
            return Ok(Some(Some(native)));
        }
        match name.as_deref() {
            Some("sable:sub_level") => {
                r.u8()?;
            }
            Some("neoforge:enum") => {
                r.string(32767)?;
            }
            Some("computercraft:repeat") => {
                r.bool()?;
                let child = r.varint()?;
                if command_properties_767(r, child, names, depth + 1)?.is_none() {
                    return Ok(None);
                }
                // TRUSTED_CONTEXT_FREE_STREAM_CODEC: a network NBT component.
                r.nbt()?;
            }
            // SingletonArgumentInfo (including the optional vanilla game-test
            // parsers) and Accessories' RecordArgumentTypeInfo.of unit template.
            Some(
                "minecraft:test_argument"
                | "minecraft:test_class"
                | "sable:vec3_absolute"
                | "neoforge:modid"
                | "sophisticatedbackpacks:backpack_uuid"
                | "sophisticatedbackpacks:player_name"
                | "sophisticatedbackpacks:template_name"
                | "sophisticatedbackpacks:backpack_item"
                | "sophisticatedbackpacks:backpack_upgrade_item"
                | "relics:relic_ability"
                | "relics:relic_ability_stat"
                | "curios:slot_type"
                | "accessories:slot_type"
                | "accessories:resource"
                | "computercraft:tracking_field"
                | "computercraft:computer"
                | "sereneseasons:season"
                | "guideme:guide_id"
                | "guideme:page_anchor"
                | "ponder:config_path"
                | "irons_spellbooks:spell",
            ) => {}
            _ => return Ok(None),
        }
        Ok(Some(None))
    }

    fn greedy_commands_767() -> Vec<u8> {
        // An unframed, unknown template cannot be skipped safely. Accept any
        // command for the server to parse rather than exposing a partial tree.
        let mut w = crate::buf::Writer::new();
        w.varint(2);
        w.u8(0); // root -> argument
        w.varint(1);
        w.varint(1);
        w.u8(6); // executable argument, no children
        w.varint(0);
        w.string("command");
        w.varint(STRING_ARGUMENT);
        w.varint(GREEDY_PHRASE);
        w.varint(0); // root index
        w.data
    }

    /// `ArgumentTypeInfos.bootstrap` (1.21.1 -> 26.3): color becomes
    /// team_color, hex_color is inserted at 17, resource_selector at 48,
    /// and the new parsers after loot_modifier move uuid from 53 to 61.
    /// The matched serializers have identical network properties: numeric
    /// flags and bounds, string VarInt, entity/score-holder byte, time i32,
    /// resource registry identifier; SingletonArgumentInfo has no properties.
    pub(super) fn commands_with_names(
        body: &[u8],
        names: impl Fn(i32) -> Option<String>,
    ) -> Vec<u8> {
        let mut r = crate::buf::Reader::new(body);
        let mut w = crate::buf::Writer::new();
        let n = match r.count(r.remaining()) {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        w.varint(n as i32);
        for _ in 0..n {
            let flags = match r.u8() {
                Ok(v) => v,
                Err(_) => return body.to_vec(),
            };
            w.u8(flags);
            let k = match r.count(r.remaining()) {
                Ok(v) => v,
                Err(_) => return body.to_vec(),
            };
            w.varint(k as i32);
            for _ in 0..k {
                let c = match r.varint() {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
                w.varint(c);
            }
            if flags & 8 != 0 {
                let redirect = match r.varint() {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
                w.varint(redirect);
            }
            let kind = flags & 3;
            if kind >= 1 && copy_ident(&mut r, &mut w).is_err() {
                return body.to_vec();
            }
            if kind == 2 {
                let id = match r.varint() {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
                let start = r.position();
                let native = match command_properties_767(&mut r, id, &names, 0) {
                    Ok(Some(native)) => native,
                    Ok(None) => {
                        super::unknown_767("minecraft:command_argument_type", id);
                        return greedy_commands_767();
                    }
                    Err(_) => return body.to_vec(),
                };
                if let Some(native) = native {
                    w.varint(native);
                    w.bytes(r.since(start));
                } else {
                    w.varint(STRING_ARGUMENT);
                    w.varint(GREEDY_PHRASE);
                }
                if flags & 16 != 0 && copy_ident(&mut r, &mut w).is_err() {
                    return body.to_vec();
                }
            }
        }
        let root = match r.varint() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        w.varint(root);
        w.data
    }

    /// UpdateAttributes 767 -> 777: both versions write the attribute as a
    /// plain registry id, so ids pass through unchanged.
    /// / RU: в обеих версиях атрибут - плоский id реестра, id не меняется.
    fn update_attributes_767_to_777(body: &[u8]) -> Vec<u8> {
        let mut r = crate::buf::Reader::new(body);
        let mut w = crate::buf::Writer::new();
        let id = match r.varint() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        w.varint(id);
        let n = match r.count(r.remaining()) {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        w.varint(n as i32);
        for _ in 0..n {
            let attribute = match r.varint() {
                Ok(v) => v,
                Err(_) => return body.to_vec(),
            };
            w.varint(attribute);
            let base = match r.f64() {
                Ok(v) => v,
                Err(_) => return body.to_vec(),
            };
            w.f64(base);
            let k = match r.count(r.remaining()) {
                Ok(v) => v,
                Err(_) => return body.to_vec(),
            };
            w.varint(k as i32);
            for _ in 0..k {
                if copy_ident(&mut r, &mut w).is_err() {
                    return body.to_vec();
                }
                let amount = match r.f64() {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
                w.f64(amount);
                let op = match r.varint() {
                    Ok(v) => v,
                    Err(_) => return body.to_vec(),
                };
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
        let player_id = match r.i32() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        super::set_local_entity_id(player_id);
        let hardcore = match r.bool() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let dims = match read_ident_set(&mut r) {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let max_players = match r.varint() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let chunk_radius = match r.varint() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let sim_distance = match r.varint() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let reduced = match r.bool() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let show_death = match r.bool() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let limited = match r.bool() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        // CommonPlayerSpawnInfo (767, without sea_level)
        let dim_type = match r.varint() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let dimension = match r.string(32767) {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let seed = match r.i64() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        // 767 writes the game types as bytes, not varints.
        /// / RU: у 767 типы игры - байты, не varint.
        let game_type = match r.i8() {
            Ok(v) => v as i32,
            Err(_) => return body.to_vec(),
        };
        let previous = match r.i8() {
            Ok(v) => v as i32,
            Err(_) => return body.to_vec(),
        };
        let debug = match r.bool() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let flat = match r.bool() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let last_death = match read_opt_global_pos(&mut r) {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let portal_cooldown = match r.varint() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
        let secure = match r.bool() {
            Ok(v) => v,
            Err(_) => return body.to_vec(),
        };
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
        // 777's previous game type is OptVarInt (0 = none, else id + 1); 767's
        // byte is -1 for none. A bool plus the raw value shifted every field
        // after it, and "none" (5 bytes of -1) broke the login outright.
        w.varint(previous + 1);
        w.bool(debug);
        w.bool(flat);
        match last_death {
            Some((ref dim, pos)) => {
                // 777's GlobalPos is a packed BlockPos long, as 767's: three
                // ints (12 bytes) shifted the rest and broke every login after
                // the player's first death.
                w.bool(true);
                write_string(&mut w, dim);
                w.i64(crate::buf::pack_block_pos(pos));
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
    fn read_opt_global_pos(
        r: &mut crate::buf::Reader,
    ) -> crate::Result<Option<(String, [i32; 3])>> {
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
        let (tag, consumed) =
            match horizon_world::nbt::read_network(nbt, horizon_world::nbt::Limits::default()) {
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
        // The trailing block entities copy through; the 767 light data
        /// does not: its bit sets are [varint long count][longs]
        /// (FriendlyByteBuf.writeBitSet) while the native 777 shape uses
        /// byte arrays ([varint byte count][bytes]). Repack the light
        /// section after the block entity list.
        /// / RU: хвост block entities проходит как есть; свет 767 - нет:
        /// его битовые маски это [varint число longs][longs]
        /// (writeBitSet), а нативный формат 777 - байтовые массивы.
        /// Перепаковываем секцию света после списка block entities.
        let after = repack_light_767(after);
        out.extend_from_slice(&after);
        out
    }

    /// Repack the 767 light tail into the native 777 byte-array shape.
    /// Structure: [BE list (passed through)][4 bit sets][2 layer lists].
    /// The block entity list is parsed to find where the light starts.
    /// / RU: перепаковка хвоста света 767 в байтовый формат 777. Структура:
    /// [список BE (проходит насквозь)][4 битовые маски][2 списка слоёв].
    /// Encode a u32 as a bare varint (Writer is not always at hand).
    /// / RU: кодирование u32 как varint без Writer.
    fn varint_bytes(mut v: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(5);
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return out;
            }
            out.push(b | 0x80);
        }
    }

    fn repack_light_767(tail: &[u8]) -> Vec<u8> {
        let limits = horizon_world::nbt::Limits::default();
        let mut r = crate::buf::Reader::new(tail);
        // Block entity list: [varint count][per: i8, i16, varint, NBT].
        /// The NBT root may be a single End byte (empty compound payload).
        /// / RU: список block entities: [varint count][i8, i16, varint, NBT].
        /// Корень NBT может быть одним байтом End.
        let n = match r.count(tail.len()) {
            Ok(v) => v,
            Err(_) => return tail.to_vec(),
        };
        for _ in 0..n {
            if r.i8().is_err() || r.i16().is_err() || r.varint().is_err() {
                return tail.to_vec();
            }
            let before = r.position();
            match horizon_world::nbt::read_network(&tail[before..], limits) {
                Ok((_, used)) => {
                    if r.bytes(used).is_err() {
                        return tail.to_vec();
                    }
                }
                Err(_) => return tail.to_vec(),
            }
        }
        let light_start = r.position();
        let Some(light) = repack_light_data_767(&tail[light_start..]) else {
            return tail.to_vec();
        };
        let mut out = Vec::with_capacity(tail.len());
        out.extend_from_slice(&tail[..light_start]);
        out.extend_from_slice(&light);
        out
    }

    /// The light data of a 767 chunk or light packet in the 777 shape: four
    /// bit sets and two layer lists. None when it cannot be read.
    fn repack_light_data_767(light: &[u8]) -> Option<Vec<u8>> {
        // 4 bit sets: 767 [varint long count][longs] -> 777
        /// [varint byte count][bytes]. BitSet.toByteArray is
        /// little-endian longs with trailing zero bytes dropped.
        /// / RU: 4 битовые маски: 767 [varint число longs][longs] ->
        /// 777 [varint число байтов][байты]. toByteArray - little-endian
        /// longs без хвостовых нулевых байтов.
        let mut lr = crate::buf::Reader::new(light);
        let mut masks: Vec<Vec<u8>> = Vec::with_capacity(4);
        for _ in 0..4 {
            let longs = match lr.varint() {
                Ok(v) => v,
                Err(_) => return None,
            };
            let Ok(longs) = usize::try_from(longs) else {
                return None;
            };
            let data = match lr.fixed_longs(longs) {
                Ok(v) => v,
                Err(_) => return None,
            };
            let mut bytes = Vec::with_capacity(longs * 8);
            for l in &data {
                bytes.extend_from_slice(&l.to_le_bytes());
            }
            let len = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
            bytes.truncate(len);
            masks.push(bytes);
        }
        // 2 layer lists ([varint n][byte_array(2048)]) share their shape
        /// in both revisions; copy the rest verbatim.
        /// / RU: 2 списка слоёв ([varint n][byte_array(2048)]) совпадают
        /// по форме; остаток копируется дословно.
        let layers_start = lr.position();
        let mut out = Vec::with_capacity(light.len());
        for m in &masks {
            out.extend_from_slice(&varint_bytes(m.len() as u32));
            out.extend_from_slice(m);
        }
        out.extend_from_slice(&light[layers_start..]);
        Some(out)
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
    fn copy_paletted_767(
        r: &mut crate::buf::Reader,
        entries: usize,
    ) -> Option<std::ops::Range<usize>> {
        // Canonical 767 (vanilla 1.21.1) container, byte-exact-verified
        /// against captured chunk bodies: [u8 bits][palette?][varint
        /// long-count][longs]. A palette list is present for bits 0,
        /// blocks 1..=8 and biomes 1..=3 (the 767 host packs "global"
        /// containers with a palette up to width 8); the longs are
        /// prefixed by their count as a varint (FriendlyByteBuf
        /// writeLongArray) and packed at the wire width.
        /// / RU: канонический контейнер 767 (vanilla 1.21.1), сверенный
        /// байт-в-байт по захваченным телам: [u8 bits][палитра?][varint
        /// число longs][longs]. Список палитры есть при bits 0, блоках
        /// 1..=8 и биомах 1..=3 (хост 767 пакует "глобальные" контейнеры
        /// с палитрой до ширины 8); longs идут с varint-счётчиком
        /// (writeLongArray) и упакованы шириной провода.
        let start = r.position();
        let bits = r.u8().ok()?;
        if bits > 32 {
            return None;
        }
        let has_palette = match (entries, bits) {
            (_, 0) => true,
            (4096, 1..=8) => true,
            (64, 1..=3) => true,
            _ => false,
        };
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
        let per = if bits == 0 {
            usize::MAX
        } else {
            64 / bits as usize
        };
        let expected = if bits == 0 { 0 } else { entries.div_ceil(per) };
        // FriendlyByteBuf.writeLongArray: varint count then the longs.
        let count = r.varint().ok()?;
        let count = usize::try_from(count).ok()?;
        if count != expected {
            return None;
        }
        r.fixed_longs(count).ok()?;
        Some(start..r.position())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::play::clientbound as cb;
    use crate::packets::play::Clientbound;

    fn egress_bodies(state: &EgressState, body: &[u8]) -> Vec<Vec<u8>> {
        egress_with(&p767::translation(), state, body)
    }

    fn egress_one(body: &[u8]) -> Vec<u8> {
        let mut out = egress_bodies(&EgressState::new(), body);
        assert_eq!(out.len(), 1, "expected exactly one body");
        out.remove(0)
    }

    fn ingress_one(id: i32, body: &[u8]) -> Vec<u8> {
        (p767::translation().ingress_body)(id, body).expect("transformed")
    }

    #[test]
    fn sable_tcp_payloads_survive_767_bundle_translation() {
        let translation = p767::translation();
        let decode = |wire_id, body: &[u8]| {
            let id = (translation.ingress)(wire_id);
            let translated = (translation.ingress_body)(id, body);
            Clientbound::decode(id, translated.as_deref().unwrap_or(body)).unwrap()
        };
        let custom = |channel: &str, data: &[u8]| {
            let mut payload = crate::buf::Writer::new();
            payload.string(channel);
            payload.bytes(data);
            let Clientbound::CustomPayload(decoded) = decode(25, &payload.data) else {
                panic!("767 custom payload did not survive translation");
            };
            assert_eq!(decoded.channel, channel);
            assert_eq!(decoded.data, data);
        };
        let delimiter = || {
            assert!(matches!(decode(0, &[]), Clientbound::BundleDelimiter(_)));
        };
        let plot = 0x0000_0002_0000_0001i64.to_be_bytes();
        let mut start = plot.to_vec();
        // UUID, previous/current poses, bounds, optional name, and tick.
        start.extend_from_slice(&[0; 16 + 2 * 64 + 24 + 1 + 4]);
        for pose in [8 + 16, 8 + 16 + 64] {
            start[pose + 36..pose + 40].copy_from_slice(&1.0f32.to_be_bytes());
        }
        let mut column = crate::buf::Writer::new();
        column.i32(1_280_128);
        column.i32(1_280_256);
        column.nbt(&horizon_world::nbt::Tag::Compound(Default::default()));
        // One singleton-palette section, no block entities, and empty light masks/lists.
        column.byte_array(&[16, 0, 0, 1, 0, 0, 0, 0]);
        column.bytes(&[0; 7]);

        delimiter();
        custom("sable:start_tracking_sub_level", &start);
        let Clientbound::LevelChunkWithLight(column) = decode(39, &column.data) else {
            panic!("767 plot column did not survive translation");
        };
        assert_eq!((column.x, column.z), (1_280_128, 1_280_256));
        assert_eq!(column.chunk.buffer, [16, 0, 0, 0, 0, 1, 0, 0, 0, 0]);
        custom("sable:finalize_sub_level", &plot);
        delimiter();
        custom(
            "sable:snapshot_info_packet",
            &[0, 0, 0, 50, 0, 0, 0, 125, 0],
        );
        custom("sable:snapshot_packet", &[0, 0, 0, 125, 0]);
        custom("sable:stop_tracking_sub_level", &plot);
    }

    const POS: [u8; 8] = [0, 0, 0, 0, 0, 0x40, 0x10, 0x05];

    #[test]
    fn player_input_forward_is_positive_zza_and_nine_bytes() {
        let out = egress_one(&[43, 0x01]);
        assert_eq!(out, [38, 0, 0, 0, 0, 0x3F, 0x80, 0, 0, 0]);
    }

    #[test]
    fn player_input_left_jump_is_positive_xxa_with_jump_bit() {
        let out = egress_one(&[43, 0x04 | 0x10]);
        assert_eq!(out, [38, 0x3F, 0x80, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn player_input_right_back_is_negative_axes() {
        let out = egress_one(&[43, 0x08 | 0x02]);
        assert_eq!(out, [38, 0xBF, 0x80, 0, 0, 0xBF, 0x80, 0, 0, 0]);
    }

    #[test]
    fn player_input_shift_change_emits_sneak_press_then_release() {
        let state = EgressState::new();
        state.set_entity_id(5);
        let press = egress_bodies(&state, &[43, 0x20]);
        assert_eq!(
            press,
            vec![vec![37, 5, 0, 0], vec![38, 0, 0, 0, 0, 0, 0, 0, 0, 2]]
        );
        let held = egress_bodies(&state, &[43, 0x20]);
        assert_eq!(held, vec![vec![38, 0, 0, 0, 0, 0, 0, 0, 0, 2]]);
        let release = egress_bodies(&state, &[43, 0]);
        assert_eq!(
            release,
            vec![vec![37, 5, 1, 0], vec![38, 0, 0, 0, 0, 0, 0, 0, 0, 0]]
        );
    }

    #[test]
    fn attack_carries_the_current_shift_state() {
        let state = EgressState::new();
        state.set_entity_id(5);
        egress_bodies(&state, &[43, 0x20]);
        assert_eq!(egress_bodies(&state, &[1, 7]), vec![vec![22, 7, 1, 1]]);
    }

    #[test]
    fn use_item_on_drops_the_world_border_byte() {
        let mut input = vec![66, 0];
        input.extend_from_slice(&POS);
        input.push(1);
        input.extend_from_slice(&[0x3F, 0, 0, 0, 0x3F, 0, 0, 0, 0x3F, 0, 0, 0]);
        let mut expected = input.clone();
        expected[0] = 56;
        input.extend_from_slice(&[1, 0, 5]);
        expected.extend_from_slice(&[1, 5]);
        assert_eq!(egress_one(&input), expected);
    }

    #[test]
    fn plot_interactions_keep_the_packed_position_and_local_face() {
        use crate::buf::pack_block_pos;
        let pos = pack_block_pos([20_482_064, 64, 20_484_128]).to_be_bytes();
        for (action, wire_action) in [(0, 0), (2, 1), (3, 2)] {
            let mut packet = vec![41, action];
            packet.extend_from_slice(&pos);
            packet.extend_from_slice(&[5, 37]);
            let mut expected = packet.clone();
            expected[..2].copy_from_slice(&[36, wire_action]);
            assert_eq!(egress_one(&packet), expected);
        }
        let mut packet = vec![66, 1];
        packet.extend_from_slice(&pos);
        packet.push(5);
        for offset in [1.0_f32, 0.25, 0.75] {
            packet.extend_from_slice(&offset.to_be_bytes());
        }
        packet.extend_from_slice(&[0, 0, 38]);
        let mut expected = packet.clone();
        expected[0] = 56;
        expected.remove(expected.len() - 2);
        assert_eq!(egress_one(&packet), expected);
        assert_eq!(
            egress_one(&p767::pick_inventory_slot(25).encode()),
            [32, 25]
        );
    }

    #[test]
    fn player_action_is_remapped_and_stab_or_direction_dropped() {
        let action = |a: u8| {
            let mut b = vec![41, a];
            b.extend_from_slice(&POS);
            b.extend_from_slice(&[1, 7]);
            b
        };
        for (from, to) in [(0u8, 0u8), (2, 1), (3, 2), (4, 3), (5, 4), (6, 5), (7, 6)] {
            let out = egress_one(&action(from));
            assert_eq!(out[..2], [36, to], "action {from}");
            assert_eq!(out.len(), action(from).len());
        }
        for dropped in [1u8, 8] {
            assert!(egress_bodies(&EgressState::new(), &action(dropped)).is_empty());
        }
    }

    #[test]
    fn chat_drops_the_trailing_checksum() {
        assert_eq!(egress_one(&[9, 0xAA, 0xBB, 0xCC]), [6, 0xAA, 0xBB]);
    }

    #[test]
    fn move_vehicle_drops_the_on_ground_byte() {
        let mut input = vec![34];
        input.extend_from_slice(&[7; 33]);
        let mut expected = vec![30];
        expected.extend_from_slice(&[7; 32]);
        assert_eq!(egress_one(&input), expected);
    }

    #[test]
    fn container_click_writes_every_stack_empty() {
        // cid 0, state 5, slot 36, button 0, mode 0, one changed slot (1) holding
        // item 42 x3 with one hashed component and no removals, carried item 42 x1.
        let input = [
            18, 0, 5, 0, 36, 0, 0, 1, 0, 1, 1, 42, 3, 1, 5, 0, 0, 0, 7, 0, 1, 42, 1, 0, 0,
        ];
        assert_eq!(egress_one(&input), [14, 0, 5, 0, 36, 0, 0, 1, 0, 1, 0, 0]);
    }

    #[test]
    fn player_command_shifts_actions_by_two() {
        assert_eq!(egress_one(&[42, 5, 1, 0]), [37, 5, 3, 0]);
        assert_eq!(egress_one(&[42, 5, 6, 0]), [37, 5, 8, 0]);
        assert_eq!(egress_one(&[42, 5, 0, 0]), [37, 5, 2, 0]);
    }

    #[test]
    fn sign_update_moves_the_side_flag_after_the_position() {
        let mut input = vec![62];
        input.extend_from_slice(&POS);
        input.extend_from_slice(&[1, b'a', 1, b'b', 0, 0]);
        input.push(1);
        let mut expected = vec![53];
        expected.extend_from_slice(&POS);
        expected.push(1);
        expected.extend_from_slice(&[1, b'a', 1, b'b', 0, 0]);
        assert_eq!(egress_one(&input), expected);
    }

    #[test]
    fn creative_slot_keeps_plain_stacks_and_drops_ones_with_a_component_patch() {
        assert_eq!(
            egress_one(&[57, 0, 36, 1, 42, 0, 0]),
            [50, 0, 36, 1, 42, 0, 0]
        );
        assert_eq!(egress_one(&[57, 0, 36, 0]), [50, 0, 36, 0]);
        // One added component (or one removed) cannot be re-encoded: no packet,
        // so the server's stack is not replaced by a stripped one.
        for patch in [[1, 0, 5, 1, 9], [0, 1, 5, 0, 0]] {
            let mut input = vec![57, 0, 36, 1, 42];
            input.extend_from_slice(&patch);
            assert!(egress_bodies(&EgressState::new(), &input).is_empty());
        }
    }

    #[test]
    fn experience_orb_becomes_an_add_entity_of_the_given_type() {
        // 767: [varint id 300][dbl 1.0, 2.0, 3.0][short 5]
        let mut body = vec![0xAC, 0x02];
        for c in [1.0f64, 2.0, 3.0] {
            body.extend_from_slice(&c.to_be_bytes());
        }
        body.extend_from_slice(&[0, 5]);
        let out = p767::experience_orb_with_kind(&body, 42).expect("rewritten");
        let Clientbound::AddEntity(p) =
            Clientbound::decode(crate::ids::play::clientbound::ADD_ENTITY, &out).unwrap()
        else {
            panic!("wrong variant");
        };
        assert_eq!((p.id, p.kind), (300, 42));
        assert_eq!(p.uuid & 0xFFFF_FFFF, 300);
        assert_eq!(
            native_id(p767::EXPERIENCE_ORB),
            crate::ids::play::clientbound::ADD_ENTITY
        );
        assert_eq!(native_id(5), 5);
        assert_eq!(p767::cb(2), p767::EXPERIENCE_ORB);
        assert!(p767::experience_orb_with_kind(&body[..10], 42).is_none());
    }

    #[test]
    fn recipe_packets_are_skipped() {
        assert_eq!(p767::cb(65), 9999);
        assert_eq!(p767::cb(119), 9999);
    }

    #[test]
    fn client_command_game_rules_and_place_recipe_are_dropped() {
        let state = EgressState::new();
        assert_eq!(egress_bodies(&state, &[12, 0]), vec![vec![9, 0]]);
        assert_eq!(egress_bodies(&state, &[12, 1]), vec![vec![9, 1]]);
        assert!(egress_bodies(&state, &[12, 2]).is_empty());
        assert!(egress_bodies(&state, &[39, 0, 0, 0]).is_empty());
    }

    #[test]
    fn move_player_keeps_only_the_on_ground_bit() {
        assert_eq!(egress_one(&[30, 1, 2, 3, 0x03]), [26, 1, 2, 3, 0x01]);
        assert_eq!(egress_one(&[33, 0x02]), [29, 0x00]);
    }

    #[test]
    fn move_entity_pos_bytes_become_a_linear_vec_delta() {
        use crate::packets::play::VecDelta;

        assert_eq!(p767::cb(46), cb::MOVE_ENTITY_POS);
        // id 300; positive, negative and zero deltas (still in 1/4096 blocks).
        for on_ground in [0, 1] {
            let body = [0xAC, 0x02, 0x10, 0, 0xF0, 0, 0, 0, on_ground];
            let out = ingress_one(p767::cb(46), &body);
            assert_eq!(out, [0xAC, 0x02, on_ground, 0x10, 0, 0xF0, 0, 0, 0]);
            let Clientbound::MoveEntityPos(p) =
                Clientbound::decode(cb::MOVE_ENTITY_POS, &out).unwrap()
            else {
                panic!("wrong variant");
            };
            assert_eq!(p.0.id, 300);
            assert_eq!(p.0.delta, Some(VecDelta::Linear([4096, -4096, 0])));
            assert_eq!(p.0.rotation, None);
            assert_eq!(p.0.on_ground, on_ground != 0);
        }
    }

    #[test]
    fn move_entity_pos_rot_bytes_keep_short_limits_and_signed_angles() {
        use crate::packets::play::VecDelta;

        assert_eq!(p767::cb(47), cb::MOVE_ENTITY_POS_ROT);
        for on_ground in [0, 1] {
            // id 1; i16::MIN, i16::MAX, -1; yaw -128, pitch 127.
            let body = [1, 0x80, 0, 0x7F, 0xFF, 0xFF, 0xFF, 0x80, 0x7F, on_ground];
            let out = ingress_one(p767::cb(47), &body);
            assert_eq!(
                out,
                [1, on_ground, 0x80, 0, 0x7F, 0xFF, 0xFF, 0xFF, 0x80, 0x7F]
            );
            let Clientbound::MoveEntityPosRot(p) =
                Clientbound::decode(cb::MOVE_ENTITY_POS_ROT, &out).unwrap()
            else {
                panic!("wrong variant");
            };
            assert_eq!(p.0.id, 1);
            assert_eq!(p.0.delta, Some(VecDelta::Linear([i16::MIN, i16::MAX, -1])));
            assert_eq!(p.0.rotation, Some((-128, 127)));
            assert_eq!(p.0.on_ground, on_ground != 0);
        }
    }

    #[test]
    fn move_entity_rot_bytes_put_on_ground_before_angles() {
        assert_eq!(p767::cb(48), cb::MOVE_ENTITY_ROT);
        for on_ground in [0, 1] {
            let body = [0xAC, 0x02, 0xC0, 0x20, on_ground];
            let out = ingress_one(p767::cb(48), &body);
            assert_eq!(out, [0xAC, 0x02, on_ground, 0xC0, 0x20]);
            let Clientbound::MoveEntityRot(p) =
                Clientbound::decode(cb::MOVE_ENTITY_ROT, &out).unwrap()
            else {
                panic!("wrong variant");
            };
            assert_eq!(p.0.id, 300);
            assert_eq!(p.0.delta, None);
            assert_eq!(p.0.rotation, Some((-64, 32)));
            assert_eq!(p.0.on_ground, on_ground != 0);
        }
    }

    #[test]
    fn move_entity_rewrites_require_all_fields() {
        let fixtures: [(i32, &[u8]); 3] = [
            (46, &[0xAC, 0x02, 0x10, 0, 0xF0, 0, 0, 0, 1]),
            (47, &[1, 0x80, 0, 0x7F, 0xFF, 0xFF, 0xFF, 0x80, 0x7F, 0]),
            (48, &[0xAC, 0x02, 0xC0, 0x20, 1]),
        ];
        for (wire_id, body) in fixtures {
            for len in 0..body.len() {
                assert!(
                    (p767::translation().ingress_body)(p767::cb(wire_id), &body[..len]).is_none()
                );
            }
        }
    }

    #[test]
    fn clientbound_vehicle_and_head_movement_bytes_need_no_rewrite() {
        // MoveVehicle: doubles 1, -2, 0; floats 90, -45.
        let vehicle = [
            0x3F, 0xF0, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x42,
            0xB4, 0, 0, 0xC2, 0x34, 0, 0,
        ];
        assert_eq!(p767::cb(49), cb::MOVE_VEHICLE);
        assert!((p767::translation().ingress_body)(p767::cb(49), &vehicle).is_none());
        let Clientbound::MoveVehicle(p) = Clientbound::decode(p767::cb(49), &vehicle).unwrap()
        else {
            panic!("wrong variant");
        };
        assert_eq!(p.position, [1.0, -2.0, 0.0]);
        assert_eq!((p.yaw, p.pitch), (90.0, -45.0));

        let head = [0xAC, 0x02, 0xC0]; // id 300, yaw -64
        assert_eq!(p767::cb(72), cb::ROTATE_HEAD);
        assert!((p767::translation().ingress_body)(p767::cb(72), &head).is_none());
        let Clientbound::RotateHead(p) = Clientbound::decode(p767::cb(72), &head).unwrap() else {
            panic!("wrong variant");
        };
        assert_eq!((p.id, p.head_yaw), (300, -64));
    }

    #[test]
    fn set_entity_motion_dump_bytes_become_an_lp_vec3() {
        // dump: id 1163, shorts -708, -242, -73
        let out = ingress_one(
            cb::SET_ENTITY_MOTION,
            &[0x8B, 0x09, 0xFD, 0x3C, 0xFF, 0x0E, 0xFF, 0xB7],
        );
        let Clientbound::SetEntityMotion(p) =
            Clientbound::decode(cb::SET_ENTITY_MOTION, &out).unwrap()
        else {
            panic!("wrong variant");
        };
        assert_eq!(p.id, 1163);
        for (got, want) in p.velocity.iter().zip([-708.0, -242.0, -73.0]) {
            assert!((got - want / 8000.0).abs() < 1e-3, "{got} vs {want}");
        }
    }

    #[test]
    fn teleport_entity_tolerates_the_server_mod_trailing_byte() {
        let dump = [
            0x88, 0x09, 0xC0, 0x30, 0x70, 0xAB, 0xCC, 0xE3, 0x6C, 0x10, 0x40, 0x54, 0xC0, 0, 0, 0,
            0, 0, 0xC0, 0x1A, 0xFF, 0x65, 0xDF, 0x75, 0x27, 0x71, 0x76, 0, 1, 0,
        ];
        let out = ingress_one(cb::TELEPORT_ENTITY, &dump);
        let Clientbound::TeleportEntity(p) =
            Clientbound::decode(cb::TELEPORT_ENTITY, &out).unwrap()
        else {
            panic!("wrong variant");
        };
        assert_eq!(p.id, 1160);
        assert_eq!(
            p.position[0],
            f64::from_be_bytes([0xC0, 0x30, 0x70, 0xAB, 0xCC, 0xE3, 0x6C, 0x10])
        );
        assert_eq!(p.relatives, 0xE0);
        assert!((p.yaw - 0x76 as f32 * 360.0 / 256.0).abs() < 1e-3);
        assert!(p.on_ground);
    }

    #[test]
    fn player_position_dump_body_is_reordered() {
        // dump: x 2.5, y 76, z 2.5, yaw/pitch 0, flags 0, teleport id 1
        let mut dump = Vec::new();
        for v in [2.5f64, 76.0, 2.5] {
            dump.extend_from_slice(&v.to_be_bytes());
        }
        dump.extend_from_slice(&[0; 8]);
        dump.extend_from_slice(&[0, 1]);
        let out = ingress_one(cb::PLAYER_POSITION, &dump);
        let mut expected = vec![1];
        expected.extend_from_slice(&dump[..24]);
        expected.extend_from_slice(&[0; 24]);
        expected.extend_from_slice(&dump[24..32]);
        expected.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(out, expected);
    }

    #[test]
    fn player_position_teleport_id_past_127_is_not_dropped() {
        let mut body = vec![0u8; 32];
        body.push(0b0000_0001); // x relative
        body.extend_from_slice(&[0x80, 0x01]); // teleport id 128
        let out = ingress_one(cb::PLAYER_POSITION, &body);
        assert_eq!(out[..2], [0x80, 0x01]);
        assert_eq!(out.len(), 2 + 24 + 24 + 8 + 4);
        // X relative also sets DELTA_X (bit 5).
        assert_eq!(out[out.len() - 4..], [0, 0, 0, 0x21]);
    }

    #[test]
    fn respawn_gets_varint_game_types_and_sea_level() {
        let dim = b"minecraft:overworld";
        let mut input = vec![0, dim.len() as u8];
        input.extend_from_slice(dim);
        input.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 9]); // seed
        input.extend_from_slice(&[1, 0xFF, 0, 0, 0, 0, 3]); // gm, prev -1, debug, flat, no death, portal, keep
        let mut expected = vec![0, dim.len() as u8];
        expected.extend_from_slice(dim);
        expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 9]);
        expected.extend_from_slice(&[1, 0, 0, 0, 0, 0, 63, 3]);
        assert_eq!(ingress_one(cb::RESPAWN, &input), expected);
    }

    #[test]
    fn update_attributes_keeps_the_plain_registry_id() {
        let mut body = vec![5, 1, 18];
        body.extend_from_slice(&20.0f64.to_be_bytes());
        body.push(0);
        assert_eq!(ingress_one(cb::UPDATE_ATTRIBUTES, &body), body);
        // id 0 used to bail out untouched; it is an ordinary attribute now.
        let mut zero = vec![5, 1, 0];
        zero.extend_from_slice(&20.0f64.to_be_bytes());
        zero.push(0);
        assert_eq!(ingress_one(cb::UPDATE_ATTRIBUTES, &zero), zero);
    }

    #[test]
    fn container_set_slot_negative_ids_become_varints() {
        // [i8 container][varint state][i16 slot][stack]: the whole body is
        // read now (the stack goes through the item rewrite), so these are
        // complete packets: the cursor at slot -1 and inventory slot 36,
        // both emptied.
        let out = ingress_one(cb::CONTAINER_SET_SLOT, &[0xFF, 0, 0xFF, 0xFF, 0]);
        assert_eq!(out, [0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0, 0xFF, 0xFF, 0]);
        let out = ingress_one(cb::CONTAINER_SET_SLOT, &[0xFE, 0, 0, 36, 0]);
        assert_eq!(out, [0xFE, 0xFF, 0xFF, 0xFF, 0x0F, 0, 0, 36, 0]);
        assert!(
            (p767::translation().ingress_body)(cb::CONTAINER_SET_SLOT, &[0, 2, 0, 36, 0]).is_none()
        );
    }

    #[test]
    fn default_spawn_position_dump_bytes_gain_dimension_and_pitch() {
        let dump = [0, 0, 0, 0, 0, 0, 0, 0x4E, 0, 0, 0, 0];
        let out = ingress_one(cb::SET_DEFAULT_SPAWN_POSITION, &dump);
        let mut expected = vec![19];
        expected.extend_from_slice(b"minecraft:overworld");
        expected.extend_from_slice(&dump);
        expected.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(out, expected);
    }

    #[test]
    fn player_chat_gets_a_zero_global_index() {
        assert_eq!(ingress_one(cb::PLAYER_CHAT, &[9, 9]), [0, 9, 9]);
    }

    /// A network NBT string tag (TAG_String, no root name).
    fn nbt_string(s: &str) -> Vec<u8> {
        let mut out = vec![8, 0, s.len() as u8];
        out.extend_from_slice(s.as_bytes());
        out
    }

    // A 1.21.1 stack: 1 of item 100 with potion_contents (31) of potion 5,
    // and its 26.3 form (potion_contents is 53 and gains an absent name).
    const POTION_767: [u8; 9] = [1, 100, 1, 0, 31, 1, 5, 0, 0];
    const POTION_777: [u8; 10] = [1, 100, 1, 0, 53, 1, 5, 0, 0, 0];
    // A stack with a mod's component (57): unreadable past its id.
    const MODDED_767: [u8; 6] = [1, 101, 1, 0, 57, 1];

    #[test]
    fn container_set_slot_stacks_are_rewritten_or_dropped() {
        let mut body = vec![0, 3, 0, 36];
        body.extend_from_slice(&POTION_767);
        let out = ingress_one(cb::CONTAINER_SET_SLOT, &body);
        let mut expected = vec![0, 3, 0, 36];
        expected.extend_from_slice(&POTION_777);
        assert_eq!(out, expected);
        assert!(Clientbound::decode(cb::CONTAINER_SET_SLOT, &out).is_ok());
        let mut modded = vec![0, 3, 0, 36];
        modded.extend_from_slice(&MODDED_767);
        assert!(ingress_one(cb::CONTAINER_SET_SLOT, &modded).is_empty());
    }

    #[test]
    fn container_set_content_keeps_the_slots_before_an_unreadable_stack() {
        // Container 200 (an unsigned byte; a 2-byte VarInt in 777), state 7,
        // the potion and an empty slot, carried empty.
        let mut body = vec![200, 7, 2];
        body.extend_from_slice(&POTION_767);
        body.extend_from_slice(&[0, 0]);
        let out = ingress_one(cb::CONTAINER_SET_CONTENT, &body);
        let mut expected = vec![0xC8, 0x01, 7, 2];
        expected.extend_from_slice(&POTION_777);
        expected.extend_from_slice(&[0, 0]);
        assert_eq!(out, expected);
        let Clientbound::ContainerSetContent(p) =
            Clientbound::decode(cb::CONTAINER_SET_CONTENT, &out).unwrap()
        else {
            panic!("container_set_content");
        };
        assert_eq!((p.container, p.items.len()), (200, 2));
        // Slot 2 holds a mod's stack: slots 0 and 1 are kept, the carried
        // stack is emptied.
        let mut body = vec![1, 7, 4, 1, 10, 0, 0, 0];
        body.extend_from_slice(&MODDED_767);
        body.extend_from_slice(&[0, 0]);
        let out = ingress_one(cb::CONTAINER_SET_CONTENT, &body);
        assert_eq!(out, [1, 7, 2, 1, 10, 0, 0, 0, 0]);
        let Clientbound::ContainerSetContent(p) =
            Clientbound::decode(cb::CONTAINER_SET_CONTENT, &out).unwrap()
        else {
            panic!("container_set_content");
        };
        assert_eq!((p.items.len(), p.carried), (2, None));
    }

    #[test]
    fn set_equipment_ends_before_an_unreadable_stack() {
        // Entity 5: main hand (more), head with a mod's stack (more), feet.
        let mut body = vec![5, 0x80];
        body.extend_from_slice(&POTION_767);
        body.push(0x85);
        body.extend_from_slice(&MODDED_767);
        body.extend_from_slice(&[2, 0]);
        let out = ingress_one(cb::SET_EQUIPMENT, &body);
        let mut expected = vec![5, 0];
        expected.extend_from_slice(&POTION_777);
        assert_eq!(out, expected);
        let Clientbound::SetEquipment(p) = Clientbound::decode(cb::SET_EQUIPMENT, &out).unwrap()
        else {
            panic!("set_equipment");
        };
        assert_eq!(p.slots.len(), 1);
        // Nothing readable: the packet is dropped.
        let mut first = vec![5, 0];
        first.extend_from_slice(&MODDED_767);
        assert!(ingress_one(cb::SET_EQUIPMENT, &first).is_empty());
    }

    #[test]
    fn merchant_offers_costs_and_results_are_rewritten() {
        let name = nbt_string("Gem");
        // Container 3, one offer: cost A (item 20, 2 of them, a custom_name
        // 5), the potion as result, no cost B, outOfStock, uses, maxUses, xp,
        // specialPrice, priceMultiplier, demand; then level 2, xp 10,
        // showProgress, canRestock.
        let mut fields = vec![0];
        for v in [1, 12, 2, 0] {
            fields.extend_from_slice(&i32::to_be_bytes(v));
        }
        fields.extend_from_slice(&0.05f32.to_be_bytes());
        fields.extend_from_slice(&i32::to_be_bytes(0));
        let mut body = vec![3, 1, 20, 2, 1, 5];
        body.extend_from_slice(&name);
        body.extend_from_slice(&POTION_767);
        body.push(0);
        body.extend_from_slice(&fields);
        body.extend_from_slice(&[2, 10, 1, 1]);
        let out = ingress_one(cb::MERCHANT_OFFERS, &body);
        let mut expected = vec![3, 1, 20, 2, 1, 6];
        expected.extend_from_slice(&name);
        expected.extend_from_slice(&POTION_777);
        expected.push(0);
        expected.extend_from_slice(&fields);
        expected.extend_from_slice(&[2, 10, 1, 1]);
        assert_eq!(out, expected);
        let Clientbound::MerchantOffers(p) =
            Clientbound::decode(cb::MERCHANT_OFFERS, &out).unwrap()
        else {
            panic!("merchant_offers");
        };
        assert_eq!((p.offers.len(), p.level, p.xp), (1, 2, 10));
    }

    #[test]
    fn set_entity_data_serializers_are_renumbered() {
        // Entity 42: pose (21 -> 20) crouching, a compound tag (16, gone),
        // then a NeoForge serializer (256) ending the list.
        let body = [42, 6, 21, 5, 7, 16, 10, 0, 9, 0x80, 0x02, 1, 2, 0xFF];
        let out = ingress_one(cb::SET_ENTITY_DATA, &body);
        assert_eq!(out, [42, 6, 20, 5, 0xFF]);
        let Clientbound::SetEntityData(p) = Clientbound::decode(cb::SET_ENTITY_DATA, &out).unwrap()
        else {
            panic!("set_entity_data");
        };
        assert_eq!(p.values, vec![(6, 20, crate::entity_data::Value::Id(5))]);
    }

    #[test]
    fn level_particles_put_the_particle_first() {
        // force, at (1, 2, 3), spread 0.5 each way, speed 0.1, 10 of them,
        // then dust 13: red at scale 1.
        let mut body = vec![1];
        for v in [1.0f64, 2.0, 3.0] {
            body.extend_from_slice(&v.to_be_bytes());
        }
        for v in [0.5f32, 0.5, 0.5, 0.1] {
            body.extend_from_slice(&v.to_be_bytes());
        }
        body.extend_from_slice(&10i32.to_be_bytes());
        body.push(13);
        for v in [1.0f32, 0.0, 0.0, 1.0] {
            body.extend_from_slice(&v.to_be_bytes());
        }
        let out = ingress_one(cb::LEVEL_PARTICLES, &body);
        let Clientbound::LevelParticles(p) =
            Clientbound::decode(cb::LEVEL_PARTICLES, &out).unwrap()
        else {
            panic!("level_particles");
        };
        assert_eq!(
            p.particle,
            crate::entity_data::Particle {
                id: 21,
                data: Some(crate::entity_data::ParticleData::ColorScale(
                    0xFFFF0000u32 as i32,
                    1.0
                )),
            }
        );
        assert!(p.override_limiter && p.always_show);
        assert_eq!((p.pos, p.dist), ([1.0, 2.0, 3.0], [0.5; 3]));
        assert_eq!((p.max_speed, p.count, p.randomization), ([0.1; 3], 10, 0));
        // A mod's particle (109) with options we can't read: forwarded as a
        // reserved id without them.
        let n = body.len() - 17;
        let mut modded = body[..n].to_vec();
        modded.extend_from_slice(&[109, 1, 2, 3]);
        let out = ingress_one(cb::LEVEL_PARTICLES, &modded);
        let Clientbound::LevelParticles(p) =
            Clientbound::decode(cb::LEVEL_PARTICLES, &out).unwrap()
        else {
            panic!("level_particles");
        };
        assert_eq!(
            p.particle,
            crate::entity_data::Particle {
                id: crate::entity_data::v767::MODDED_PARTICLE_BASE + 109,
                data: None
            }
        );
        assert_eq!((p.count, p.pos), (10, [1.0, 2.0, 3.0]));
    }

    #[test]
    fn preserves_cbc_level_particle_options() {
        use crate::buf::Writer;
        use crate::entity_data::{
            BlastWave, CBC_AUTOCANNON_PLUME, CBC_BIG_CANNON_PLUME, CBC_CANNON_BLAST_WAVE,
            CBC_DROP_MORTAR_PLUME, CBC_SHELL_BLAST_WAVE, ParticleData,
        };
        let packet = |particle: i32, options: &dyn Fn(&mut Writer)| {
            let mut packet = Writer::new();
            packet.bool(true);
            [1.0, 2.0, 3.0].into_iter().for_each(|v| packet.f64(v));
            [0.0, 0.0, -1.0, 1.0]
                .into_iter()
                .for_each(|v| packet.f32(v));
            packet.i32(0);
            packet.varint(particle);
            options(&mut packet);
            packet.data
        };
        let plume = |size, power, lifetime| ParticleData::CannonPlume {
            size,
            power,
            lifetime,
        };
        let registry_sound = |w: &mut Writer| {
            w.f64(12.5);
            w.varint(41); // minecraft:sound_event id 40
            w.varint(4); // SoundSource.BLOCKS
            [5.0, 0.9, 2.0, 3.0].into_iter().for_each(|v| w.f32(v));
        };
        let direct_sound = |w: &mut Writer| {
            w.f64(0.0);
            w.varint(0);
            w.string("createbigcannons:shell_explosion");
            w.bool(true);
            w.f32(64.0);
            w.varint(4);
            [8.0, 1.0, 0.5, 1.5].into_iter().for_each(|v| w.f32(v));
        };
        let wave = |sound: &str, range, radius, volume, pitch, air, power| {
            ParticleData::BlastWave(BlastWave {
                blast_radius: radius,
                sound: sound.to_owned(),
                fixed_range: range,
                source: 4,
                volume,
                pitch,
                air_absorption: air,
                power,
            })
        };
        let cases = [
            (
                "createbigcannons:big_cannon_plume",
                packet(180, &|w| {
                    w.f32(3.0);
                    w.f32(6.5);
                    w.varint(10);
                }),
                CBC_BIG_CANNON_PLUME,
                plume(3.0, 6.5, 10),
            ),
            (
                "createbigcannons:autocannon_plume",
                packet(180, &|w| w.f32(3.0)),
                CBC_AUTOCANNON_PLUME,
                plume(3.0, 0.0, 0),
            ),
            (
                "createbigcannons:drop_mortar_plume",
                packet(180, &|w| w.f32(1.0)),
                CBC_DROP_MORTAR_PLUME,
                plume(1.0, 0.0, 0),
            ),
            (
                "createbigcannons:cannon_blast_wave",
                packet(180, &registry_sound),
                CBC_CANNON_BLAST_WAVE,
                wave(
                    "createbigcannons:fire_big_cannon",
                    None,
                    12.5,
                    5.0,
                    0.9,
                    2.0,
                    3.0,
                ),
            ),
            (
                "createbigcannons:shell_blast_wave",
                packet(180, &direct_sound),
                CBC_SHELL_BLAST_WAVE,
                wave(
                    "createbigcannons:shell_explosion",
                    Some(64.0),
                    0.0,
                    8.0,
                    1.0,
                    0.5,
                    1.5,
                ),
            ),
        ];
        for (name, wire, id, data) in cases {
            let names = |registry: &str, id| match (registry, id) {
                ("minecraft:particle_type", 180) => Some(name.to_owned()),
                ("minecraft:sound_event", 40) => {
                    Some("createbigcannons:fire_big_cannon".to_owned())
                }
                _ => None,
            };
            let out = p767::level_particles_with_names(&wire, names).unwrap();
            let Clientbound::LevelParticles(p) =
                Clientbound::decode(cb::LEVEL_PARTICLES, &out).unwrap()
            else {
                panic!("level_particles");
            };
            assert_eq!(p.particle.id, id, "{name}");
            assert_eq!(p.particle.data, Some(data), "{name}");
            assert_eq!(
                (p.pos, p.dist, p.max_speed, p.count),
                ([1.0, 2.0, 3.0], [0.0, 0.0, -1.0], [1.0; 3], 0)
            );
            assert!(p767::level_particles_with_names(&wire[..wire.len() - 1], names).is_err());
        }
        let breech = |registry: &str, id| {
            (registry == "minecraft:particle_type" && id == 180)
                .then(|| "createbigcannons:quick_firing_breech_smoke".to_owned())
        };
        let wire = packet(180, &|_| {});
        let out = p767::level_particles_with_names(&wire, breech).unwrap();
        let Clientbound::LevelParticles(p) =
            Clientbound::decode(cb::LEVEL_PARTICLES, &out).unwrap()
        else {
            panic!("breech level particle");
        };
        assert_eq!(p.particle.id, crate::entity_data::CBC_BREECH_SMOKE);
        assert_eq!(p.particle.data, None);
        assert_eq!((p.count, p.dist), (0, [0.0, 0.0, -1.0]));
        let mut extra = wire;
        extra.push(0);
        assert!(p767::level_particles_with_names(&extra, breech).is_err());

        // A blast wave naming a sound the server's registry lacks is dropped.
        let unknown = packet(180, &|w| {
            w.f64(1.0);
            w.varint(99);
            w.varint(4);
            [1.0, 1.0, 1.0, 1.0].into_iter().for_each(|v| w.f32(v));
        });
        let names = |registry: &str, id| {
            (registry == "minecraft:particle_type" && id == 180)
                .then(|| "createbigcannons:cannon_blast_wave".to_owned())
        };
        assert!(p767::level_particles_with_names(&unknown, names).is_err());
    }

    #[test]
    fn reserved_particle_ids_name_the_servers_particle() {
        let base = crate::entity_data::v767::MODDED_PARTICLE_BASE;
        let names = |id| (id == 120).then(|| "mod:spark".to_owned());
        assert_eq!(
            modded_particle_name_in(names, base + 120).as_deref(),
            Some("mod:spark")
        );
        assert_eq!(modded_particle_name_in(names, base + 121), None);
        // 26.3's own ids are not reserved.
        assert_eq!(modded_particle_name_in(names, 120), None);
        assert_eq!(modded_particle_name_in(names, -1), None);
    }

    #[test]
    fn advancement_icons_become_templates() {
        let title = nbt_string("T");
        let id = b"a:b";
        // One advancement with a display: title, description, the potion as
        // icon, frame 0, flags 0, x, y; no requirements, no telemetry; no
        // removals, no progress.
        let mut body = vec![0, 1, id.len() as u8];
        body.extend_from_slice(id);
        body.extend_from_slice(&[0, 1]);
        body.extend_from_slice(&title);
        body.extend_from_slice(&title);
        body.extend_from_slice(&POTION_767);
        body.extend_from_slice(&[0, 0, 0, 0, 0]);
        body.extend_from_slice(&1.5f32.to_be_bytes());
        body.extend_from_slice(&2.5f32.to_be_bytes());
        body.extend_from_slice(&[0, 0, 0, 0]);
        let out = ingress_one(cb::UPDATE_ADVANCEMENTS, &body);
        let mut expected = vec![0, 1, id.len() as u8];
        expected.extend_from_slice(id);
        expected.extend_from_slice(&[0, 1]);
        expected.extend_from_slice(&title);
        expected.extend_from_slice(&title);
        // ItemStackTemplate: the item, then the count.
        expected.extend_from_slice(&[100, 1, 1, 0, 53, 1, 5, 0, 0, 0]);
        expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]);
        expected.extend_from_slice(&1.5f32.to_be_bytes());
        expected.extend_from_slice(&2.5f32.to_be_bytes());
        expected.extend_from_slice(&[0, 0, 1]);
        assert_eq!(out, expected);
        assert!(Clientbound::decode(cb::UPDATE_ADVANCEMENTS, &out).is_ok());
    }

    #[test]
    fn server_names_label_unknown_ids() {
        set_server_names(
            "minecraft:data_component_type\t57\tbettercombat:preset_id\n\
             minecraft:item\t1\tminecraft:stone\n\
             neoforge:entity_data_serializers\t3\ttwilightforest:string_list",
        );
        let table = server_names().read().unwrap();
        assert_eq!(
            table
                .get(&("minecraft:data_component_type".to_owned(), 57))
                .map(String::as_str),
            Some("bettercombat:preset_id")
        );
        assert_eq!(table.len(), 3);
        drop(table);
        // Cooldown: the item id becomes its name; an unnamed item is dropped.
        assert_eq!(
            ingress_one(cb::COOLDOWN, &[1, 20]),
            [wire_string("minecraft:stone"), vec![20]].concat()
        );
        assert!(ingress_one(cb::COOLDOWN, &[2, 20]).is_empty());
    }

    #[test]
    fn cbc_blast_wave_sound_resolves_from_the_server_registries() {
        use crate::buf::Writer;
        use crate::entity_data::ParticleData;
        // The registries.tsv rows a Create Big Cannons server's oracle writes.
        let names = parse_server_names(
            "minecraft:particle_type\t150\tcreatebigcannons:cannon_blast_wave\n\
             minecraft:sound_event\t1614\tcreatebigcannons:fire_big_cannon\n\
             minecraft:block\t1\tminecraft:stone",
        );
        assert_eq!(names.len(), 2);
        let mut wire = Writer::new();
        wire.bool(true);
        [1.0, 2.0, 3.0].into_iter().for_each(|v| wire.f64(v));
        [0.0; 4].into_iter().for_each(|v| wire.f32(v));
        wire.i32(1);
        wire.varint(150);
        wire.f64(20.0);
        wire.varint(1615); // Holder: the registry id + 1
        wire.varint(4); // SoundSource.BLOCKS
        [5.0, 1.0, 2.0, 2.0].into_iter().for_each(|v| wire.f32(v));
        let out = p767::level_particles_with_names(&wire.data, |registry: &str, id| {
            names.get(&(registry.to_owned(), id)).cloned()
        })
        .unwrap();
        let Clientbound::LevelParticles(p) =
            Clientbound::decode(cb::LEVEL_PARTICLES, &out).unwrap()
        else {
            panic!("level_particles");
        };
        let Some(ParticleData::BlastWave(wave)) = p.particle.data else {
            panic!("blast wave: {:?}", p.particle.data);
        };
        assert_eq!(wave.sound, "createbigcannons:fire_big_cannon");
        assert_eq!(
            (wave.blast_radius, wave.volume, wave.air_absorption),
            (20.0, 5.0, 2.0)
        );
    }

    fn wire_string(s: &str) -> Vec<u8> {
        let mut out = vec![s.len() as u8];
        out.extend_from_slice(s.as_bytes());
        out
    }

    #[test]
    fn interact_is_preceded_by_interact_at_with_the_location() {
        // 777 interact: entity 7, main hand, zero LpVec3, secondary.
        let out = egress_bodies(&EgressState::new(), &[26, 7, 0, 0, 1]);
        let mut at = vec![22, 7, 2];
        at.extend_from_slice(&[0; 12]);
        at.extend_from_slice(&[0, 1]);
        assert_eq!(out, vec![at, vec![22, 7, 0, 0, 1]]);
    }

    #[test]
    fn change_game_mode_becomes_the_gamemode_command() {
        let out = egress_one(&[5, 1]);
        let mut want = vec![4];
        want.extend(wire_string("gamemode creative"));
        assert_eq!(out, want);
        assert!(egress_bodies(&EgressState::new(), &[5, 9]).is_empty());
    }

    #[test]
    fn chat_ack_leads_once_enough_signed_lines_are_pending() {
        let state = EgressState::new();
        state
            .pending_chat
            .store(CHAT_ACK_BATCH, std::sync::atomic::Ordering::Relaxed);
        let out = egress_bodies(&state, &[46]);
        assert_eq!(out, vec![vec![3, 64], vec![54, 0]]);
        assert_eq!(egress_bodies(&state, &[46]), vec![vec![54, 0]]);
    }

    #[test]
    fn a_new_login_forgets_the_pending_signed_chat() {
        use std::sync::atomic::Ordering::Relaxed;
        let state = EgressState::new();
        state.pending_chat.store(30, Relaxed);
        state.set_entity_id(7);
        assert_eq!(state.pending_chat.load(Relaxed), 0);
    }

    #[test]
    fn signed_player_chat_is_counted_for_the_ack() {
        let mut signed = vec![0u8; 16];
        signed.extend_from_slice(&[0, 1]);
        let load = || {
            EGRESS_STATE
                .pending_chat
                .load(std::sync::atomic::Ordering::Relaxed)
        };
        let before = load();
        ingress_one(cb::PLAYER_CHAT, &signed);
        assert!(load() > before);
    }

    #[test]
    fn light_update_bit_sets_become_byte_arrays() {
        let mut body = vec![1, 2];
        for long in [3i64, 2] {
            body.push(1);
            body.extend_from_slice(&long.to_be_bytes());
        }
        body.extend_from_slice(&[0, 0, 0, 0]);
        let out = ingress_one(cb::LIGHT_UPDATE, &body);
        assert_eq!(out, [1, 2, 1, 3, 1, 2, 0, 0, 0, 0]);
        assert!(matches!(
            Clientbound::decode(cb::LIGHT_UPDATE, &out),
            Ok(Clientbound::LightUpdate(_))
        ));
    }

    #[test]
    fn explode_picks_the_particle_and_widens_the_knockback() {
        let body = |power: f32| {
            let mut b = Vec::new();
            for v in [1.0f64, 2.0, 3.0] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            b.extend_from_slice(&power.to_be_bytes());
            b.extend_from_slice(&[1, 1, 2, 3]);
            for v in [0.5f32, 0.0, 0.0] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            // destroy, small particle explosion (22), large emitter (21),
            // sound holder id 4.
            b.extend_from_slice(&[1, 22, 21, 5]);
            b
        };
        let particle = |id: u8| {
            let mut w = crate::buf::Writer::new();
            crate::entity_data::v767::particle(&mut crate::buf::Reader::new(&[id]), &mut w)
                .unwrap();
            w.data
        };
        let expect = |power: f32, id: u8| {
            let mut b = Vec::new();
            for v in [1.0f64, 2.0, 3.0] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            b.extend_from_slice(&power.to_be_bytes());
            b.extend_from_slice(&1i32.to_be_bytes());
            b.push(1);
            for v in [0.5f64, 0.0, 0.0] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            b.extend(particle(id));
            b.extend_from_slice(&[5, 0, 1]);
            b
        };
        assert_eq!(ingress_one(cb::EXPLODE, &body(4.0)), expect(4.0, 21));
        assert_eq!(ingress_one(cb::EXPLODE, &body(1.0)), expect(1.0, 22));
        let out = ingress_one(cb::EXPLODE, &body(4.0));
        let Clientbound::Explode(p) = Clientbound::decode(cb::EXPLODE, &out).unwrap() else {
            panic!("explode");
        };
        assert_eq!(
            (p.block_count, p.knockback, p.play_sound),
            (1, Some([0.5, 0.0, 0.0]), true)
        );
        assert_eq!(p.sound.0, 4);
    }

    #[test]
    fn set_time_gets_one_clock_and_a_stopped_rate_for_negative_day_time() {
        let mut body = 100i64.to_be_bytes().to_vec();
        body.extend_from_slice(&(-6000i64).to_be_bytes());
        let out = ingress_one(cb::SET_TIME, &body);
        let Clientbound::SetTime(p) = Clientbound::decode(cb::SET_TIME, &out).unwrap() else {
            panic!("set_time");
        };
        assert_eq!(p.game_time, 100);
        assert_eq!((p.clocks[0].total_ticks, p.clocks[0].rate), (6000, 0.0));
        body.truncate(8);
        body.extend_from_slice(&24000i64.to_be_bytes());
        let out = ingress_one(cb::SET_TIME, &body);
        let Clientbound::SetTime(p) = Clientbound::decode(cb::SET_TIME, &out).unwrap() else {
            panic!("set_time");
        };
        assert_eq!((p.clocks[0].total_ticks, p.clocks[0].rate), (24000, 1.0));
    }

    #[test]
    fn set_player_team_parameters_are_reordered_and_enums_become_ids() {
        // dump layout: options 03, "never", "always", colour 14.
        let mut body = wire_string("t");
        body.push(0);
        body.extend(nbt_string("D"));
        body.push(3);
        body.extend(wire_string("never"));
        body.extend(wire_string("always"));
        body.push(14);
        body.extend(nbt_string("P"));
        body.extend(nbt_string("S"));
        body.push(1);
        body.extend(wire_string("bob"));
        let out = ingress_one(cb::SET_PLAYER_TEAM, &body);
        let Clientbound::SetPlayerTeam(p) = Clientbound::decode(cb::SET_PLAYER_TEAM, &out).unwrap()
        else {
            panic!("set_player_team");
        };
        let params = p.parameters.unwrap();
        assert_eq!(
            (
                params.name_tag_visibility,
                params.collision_rule,
                params.color,
                params.options
            ),
            (1, 0, Some(14), 3)
        );
        assert_eq!(p.players, ["bob"]);
        // Methods without parameters are untouched.
        let mut join = wire_string("t");
        join.push(3);
        join.push(1);
        join.extend(wire_string("bob"));
        assert_eq!(ingress_one(cb::SET_PLAYER_TEAM, &join), join);
    }

    fn command_argument_fixture(id: i32, properties: &[u8]) -> Vec<u8> {
        // Root -> executable argument -> literal, with a redirect to root
        // and ask_server suggestions after the argument's properties.
        let mut body = vec![3, 0, 1, 1, 30, 1, 2, 0];
        body.extend(wire_string("a"));
        let mut parser = crate::buf::Writer::new();
        parser.varint(id);
        body.extend(parser.data);
        body.extend_from_slice(properties);
        body.extend(wire_string("minecraft:ask_server"));
        body.extend_from_slice(&[5, 0]);
        body.extend(wire_string("after"));
        body.push(0);
        body
    }

    fn assert_command_argument(out: &[u8], id: i32, properties: &[u8]) {
        assert_eq!(out, command_argument_fixture(id, properties));
        let Clientbound::Commands(tree) = Clientbound::decode(cb::COMMANDS, out).unwrap() else {
            panic!("commands");
        };
        assert_eq!(tree.root, 0);
        assert_eq!(tree.nodes.len(), 3);
        assert_eq!(tree.nodes[0].children, [1]);
        let argument = &tree.nodes[1];
        assert_eq!(argument.children, [2]);
        assert_eq!(argument.redirect, Some(0));
        assert!(argument.executable);
        assert_eq!(argument.name, "a");
        assert_eq!(argument.parser, Some((id, properties.to_vec())));
        assert_eq!(
            argument.suggestions.as_deref(),
            Some("minecraft:ask_server")
        );
        assert_eq!(tree.nodes[2].name, "after");
        assert!(tree.nodes[2].executable);
    }

    #[test]
    fn all_vanilla_command_arguments_map_to_their_native_parser() {
        // ArgumentTypeInfos.bootstrap in production 1.21.1 id order.
        // The old minecraft:color has the same parser as team_color.
        let names = [
            "brigadier:bool",
            "brigadier:float",
            "brigadier:double",
            "brigadier:integer",
            "brigadier:long",
            "brigadier:string",
            "minecraft:entity",
            "minecraft:game_profile",
            "minecraft:block_pos",
            "minecraft:column_pos",
            "minecraft:vec3",
            "minecraft:vec2",
            "minecraft:block_state",
            "minecraft:block_predicate",
            "minecraft:item_stack",
            "minecraft:item_predicate",
            "minecraft:team_color",
            "minecraft:component",
            "minecraft:style",
            "minecraft:message",
            "minecraft:nbt_compound_tag",
            "minecraft:nbt_tag",
            "minecraft:nbt_path",
            "minecraft:objective",
            "minecraft:objective_criteria",
            "minecraft:operation",
            "minecraft:particle",
            "minecraft:angle",
            "minecraft:rotation",
            "minecraft:scoreboard_slot",
            "minecraft:score_holder",
            "minecraft:swizzle",
            "minecraft:team",
            "minecraft:item_slot",
            "minecraft:item_slots",
            "minecraft:resource_location",
            "minecraft:function",
            "minecraft:entity_anchor",
            "minecraft:int_range",
            "minecraft:float_range",
            "minecraft:dimension",
            "minecraft:gamemode",
            "minecraft:time",
            "minecraft:resource_or_tag",
            "minecraft:resource_or_tag_key",
            "minecraft:resource",
            "minecraft:resource_key",
            "minecraft:template_mirror",
            "minecraft:template_rotation",
            "minecraft:heightmap",
            "minecraft:loot_table",
            "minecraft:loot_predicate",
            "minecraft:loot_modifier",
            "minecraft:uuid",
        ];
        for (id, name) in names.iter().enumerate() {
            let properties = match id {
                1..=6 | 30 => vec![0],
                42 => vec![0, 0, 0, 0],
                43..=46 => wire_string("minecraft:worldgen/biome"),
                _ => Vec::new(),
            };
            let native = crate::packets::play::ARGUMENT_TYPES
                .iter()
                .position(|n| n == name)
                .unwrap() as i32;
            let out = ingress_one(
                cb::COMMANDS,
                &command_argument_fixture(id as i32, &properties),
            );
            assert_command_argument(&out, native, &properties);
        }
    }

    #[test]
    fn command_argument_properties_keep_the_following_nodes_aligned() {
        // Each non-singleton 1.21.1 serializer: numeric min/max flags,
        // string enum VarInt, selector flags, time minimum and registry.
        let fixtures: &[(i32, i32, &[u8])] = &[
            (1, 1, &[1, 0xBF, 0x80, 0, 0]), // float min -1
            (1, 1, &[2, 0x3F, 0x80, 0, 0]), // float max 1
            (1, 1, &[3, 0xBF, 0x80, 0, 0, 0x3F, 0x80, 0, 0]),
            (2, 2, &[1, 0xBF, 0xF0, 0, 0, 0, 0, 0, 0]), // double min -1
            (2, 2, &[2, 0x3F, 0xF0, 0, 0, 0, 0, 0, 0]), // double max 1
            (
                2,
                2,
                &[
                    3, 0xBF, 0xF0, 0, 0, 0, 0, 0, 0, 0x3F, 0xF0, 0, 0, 0, 0, 0, 0,
                ],
            ),
            (3, 3, &[1, 0xFF, 0xFF, 0xFF, 0xFF]), // integer min -1
            (3, 3, &[2, 0, 0, 0, 1]),             // integer max 1
            (3, 3, &[3, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 1]),
            (4, 4, &[1, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
            (4, 4, &[2, 0, 0, 0, 0, 0, 0, 0, 1]),
            (
                4,
                4,
                &[
                    3, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 1,
                ],
            ),
            (5, 5, &[1]),
            (5, 5, &[2]),
            (6, 6, &[1]),
            (6, 6, &[2]),
            (6, 6, &[3]),
            (30, 31, &[1]),
            (42, 43, &[0, 0, 0x01, 0x2C]), // time min 300, big-endian i32
            (43, 44, b"\x0Fminecraft:biome"),
            (44, 45, b"\x0Fminecraft:biome"),
            (45, 46, b"\x0Fminecraft:biome"),
            (46, 47, b"\x0Fminecraft:biome"),
        ];
        for &(id, native, properties) in fixtures {
            let out = ingress_one(cb::COMMANDS, &command_argument_fixture(id, properties));
            assert_command_argument(&out, native, properties);
        }
    }

    #[test]
    fn modded_command_argument_becomes_a_greedy_string() {
        // Server registry table: all singleton templates, including the
        // reported computer (71) and spell (77), plus optional game tests.
        let names = [
            (54, "minecraft:test_argument"),
            (55, "minecraft:test_class"),
            (57, "sable:vec3_absolute"),
            (59, "neoforge:modid"),
            (60, "sophisticatedbackpacks:backpack_uuid"),
            (61, "sophisticatedbackpacks:player_name"),
            (62, "sophisticatedbackpacks:template_name"),
            (63, "sophisticatedbackpacks:backpack_item"),
            (64, "sophisticatedbackpacks:backpack_upgrade_item"),
            (65, "relics:relic_ability"),
            (66, "relics:relic_ability_stat"),
            (67, "curios:slot_type"),
            (68, "accessories:slot_type"),
            (69, "accessories:resource"),
            (70, "computercraft:tracking_field"),
            (71, "computercraft:computer"),
            (73, "sereneseasons:season"),
            (74, "guideme:guide_id"),
            (75, "guideme:page_anchor"),
            (76, "ponder:config_path"),
            (77, "irons_spellbooks:spell"),
        ];
        for (id, name) in names {
            let body = command_argument_fixture(id, &[]);
            let out = p767::commands_with_names(&body, |_| Some(name.to_owned()));
            assert_command_argument(&out, 5, &[2]);
        }
    }

    #[test]
    fn command_argument_registry_names_override_numeric_ids() {
        for (id, name, properties, native) in [
            (71, "minecraft:time", &[0, 0, 1, 44][..], 43),
            (77, "minecraft:resource", &b"\x0Fminecraft:biome"[..], 46),
            (130, "minecraft:color", &[][..], 16),
            (3, "minecraft:uuid", &[][..], 61),
        ] {
            let body = command_argument_fixture(id, properties);
            let out = p767::commands_with_names(&body, |_| Some(name.to_owned()));
            assert_command_argument(&out, native, properties);
        }
        // A mod occupying a vanilla id must not consume that vanilla parser's properties.
        let body = command_argument_fixture(3, &[]);
        let out = p767::commands_with_names(&body, |_| Some("computercraft:computer".into()));
        assert_command_argument(&out, 5, &[2]);
    }

    fn server_command_argument_name(id: i32) -> Option<String> {
        match id {
            56 => Some("sable:sub_level".into()),
            58 => Some("neoforge:enum".into()),
            71 => Some("computercraft:computer".into()),
            72 => Some("computercraft:repeat".into()),
            _ => None,
        }
    }

    fn repeat_command_properties(child: i32, properties: &[u8]) -> Vec<u8> {
        let mut w = crate::buf::Writer::new();
        w.bool(true);
        w.varint(child);
        w.bytes(properties);
        // A literal component encoded as a network NBT string.
        w.bytes(b"\x08\x00\x05error");
        w.data
    }

    #[test]
    fn modded_command_properties_keep_the_following_nodes_aligned() {
        let fixtures = [
            (56, vec![3]),
            (58, wire_string("example.Mode")),
            (72, repeat_command_properties(71, &[])),
            (
                72,
                repeat_command_properties(3, &[3, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 1]),
            ),
            (72, repeat_command_properties(56, &[3])),
            (
                72,
                repeat_command_properties(58, &wire_string("example.Mode")),
            ),
            (
                72,
                repeat_command_properties(72, &repeat_command_properties(71, &[])),
            ),
        ];
        for (id, properties) in fixtures {
            let body = command_argument_fixture(id, &properties);
            let out = p767::commands_with_names(&body, server_command_argument_name);
            assert_command_argument(&out, 5, &[2]);
            for len in 0..body.len() {
                assert_eq!(
                    p767::commands_with_names(&body[..len], server_command_argument_name),
                    body[..len]
                );
            }
        }
    }

    #[test]
    fn unknown_command_layout_uses_a_complete_greedy_tree() {
        for id in [71, 77, 130] {
            // Unknown properties are unframed and may contain node-like bytes.
            let body = command_argument_fixture(id, &[30, 1, 2, 0, 255]);
            for name in [None, Some("example:unknown".to_owned())] {
                let out = p767::commands_with_names(&body, |_| name.clone());
                let Clientbound::Commands(tree) = Clientbound::decode(cb::COMMANDS, &out).unwrap()
                else {
                    panic!("commands");
                };
                assert_eq!(tree.root, 0);
                assert_eq!(tree.nodes.len(), 2);
                assert_eq!(tree.nodes[0].children, [1]);
                assert!(tree.nodes[1].executable);
                assert_eq!(tree.nodes[1].parser, Some((5, vec![2])));
            }
        }
        let body = command_argument_fixture(72, &repeat_command_properties(130, &[255]));
        let out = p767::commands_with_names(&body, server_command_argument_name);
        assert!(Clientbound::decode(cb::COMMANDS, &out).is_ok());

        // Deeply nested mod templates must not exhaust the stack.
        let body = command_argument_fixture(72, &[0; 130]);
        let out = p767::commands_with_names(&body, |_| Some("computercraft:repeat".into()));
        assert!(Clientbound::decode(cb::COMMANDS, &out).is_ok());
    }

    #[test]
    fn modded_enum_command_argument_consumes_its_class_name() {
        let body = command_argument_fixture(77, &wire_string("example.Mode"));
        let out = p767::commands_with_names(&body, |id| {
            assert_eq!(id, 77);
            Some("neoforge:enum".to_owned())
        });
        assert_command_argument(&out, 5, &[2]);
    }

    #[test]
    fn truncated_command_properties_keep_the_original_body() {
        // No partially rewritten tree escapes on a truncated serializer.
        for (id, properties) in [(42, &[0, 0, 1, 44][..]), (46, b"\x0Fminecraft:biome")] {
            let body = command_argument_fixture(id, properties);
            for len in 0..body.len() {
                assert_eq!(ingress_one(cb::COMMANDS, &body[..len]), body[..len]);
            }
        }
        let body = command_argument_fixture(77, &wire_string("example.Mode"));
        for len in 0..body.len() {
            assert_eq!(
                p767::commands_with_names(&body[..len], |_| Some("neoforge:enum".to_owned())),
                body[..len]
            );
        }
    }

    #[test]
    fn place_ghost_recipe_is_ignored() {
        assert_eq!(p767::cb(55), 9999);
    }
}
