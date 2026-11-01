//! `horizon-via`: protocol translation for the Horizon client.
//!
//! Let me state the goal first, in one paragraph, because ten modules will be
//! written by parallel agents and every one of them must agree on it: the
//! Horizon client speaks Minecraft protocol 26.3. Most public servers speak
//! something older — 1.8.8, 1.12.2, 1.16.5, 1.20.x. `horizon-via` sits between
//! the socket and the client: on the wire side it speaks the server's version,
//! on the client side it speaks 26.3, and in between it rewrites every packet
//! that differs — ids, field order, formats, NBT, registries. This is what
//! ViaVersion/ViaFabricPlus do for the Java client, ported "in full" to Rust.
//!
//! RU: клиент Horizon говорит по протоколу 26.3. Большинство публичных серверов
//! говорят на более старом — 1.8.8, 1.12.2, 1.16.5, 1.20.x. `horizon-via` стоит
//! между сокетом и клиентом: наружу говорит на версии сервера, внутрь — на 26.3,
//! и переписывает каждый отличающийся пакет. Это порт ViaVersion/ViaFabricPlus.
//!
//! ## Architecture (the contract every module codes against)
//!
//! ```text
//!   server (v N)                    client (26.3)
//!      | raw bytes                       ^ typed packets (26.3 codec)
//!      v                                 |
//!   [ frame codec vN ] -> [ Decoder vN->N+1 ]* -> [ re-encode? no ]
//!      reverse: [ Encoder N+1->N ]* <- client packets
//! ```
//!
//! Translation happens in *chains*: a 1.8 packet climbs 1.8 -> 1.9 -> ...
//! -> 26.3 through small per-step rewrites (exactly ViaVersion's model), so
//! each coder owns ONE step, not the whole distance. Step modules live in
//! `steps/` and implement [`Step`].
//!
//! ## Hard rules for every module (coders: read this)
//!
//! 1. `#![forbid(unsafe_code)]` — already enforced in Cargo.toml; do not add `unsafe`.
//! 2. Every public item carries a doc comment (EN + short RU note where it helps).
//! 3. Unit tests: every rewritten packet has a round-trip or golden-bytes test.
//!    Tests must not open sockets, files outside `target/`, or sleep.
//! 4. No `println!` in library code — use the `log`-style `tr` helper below
//!    (kept local until wiring day, to stay dependency-free).
//! 5. Errors: return `Result<_, ViaError>`; never `unwrap()` on user data
//!    (a malformed packet from a hostile server must never panic the client).
//! 6. Do NOT run `cargo build` in your task: you write code + tests, the
//!    orchestrator builds once at the end. A "signal" from you = the module's
//!    file list and a one-paragraph self-check, not a build log.
//!
//! EN: the crate compiles standalone first; wiring into `horizon-protocol`
//! happens later through the two traits at the bottom of this file.

#![forbid(unsafe_code)]

pub mod protocol_const;
pub mod versions;
pub mod registry_map;
pub mod nbt_compat;
pub mod frame;
pub mod state;
pub mod errors;
pub mod compression;
pub mod probe;
pub mod auth_compat;
pub mod item_remap;
pub mod chat_legacy;
pub mod steps;

/// Which side a translated packet is flowing to.
/// RU: направление пакета при трансляции.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Server -> client (decoding an older packet up to 26.3).
    Clientbound,
    /// Client -> server (encoding a 26.3 packet down to the server's version).
    Serverbound,
}

impl Direction {
    /// The opposite direction.
    /// RU: обратное направление.
    pub fn flip(self) -> Self {
        match self {
            Direction::Clientbound => Direction::Serverbound,
            Direction::Serverbound => Direction::Clientbound,
        }
    }
}

/// One version-to-version translation step, e.g. 1.16.5 -> 1.17.
///
/// A step receives the *decoded fields* of a packet as it exists on the
/// wire at version `from`, and must produce the same logical packet as it
/// exists at version `to`. Steps are chained by the (later) pipeline in
/// `horizon-protocol`; here they only declare what they translate.
///
/// RU: один шаг трансляции между соседними версиями. Шаг получает разобранные
/// поля пакета в версии `from` и должен выдать тот же логический пакет
/// в версии `to`. Шаги соединяются в цепочку позже; здесь они только объявляют,
/// что переводят.
pub trait Step {
    /// `from` protocol version number (e.g. 754 for 1.16.5).
    /// RU: номер протокола исходной версии.
    const FROM: i32;
    /// `to` protocol version number.
    /// RU: номер протокола целевой версии.
    const TO: i32;

    /// Human-readable name, e.g. `"1.16.5 -> 1.17 (block ids)"`.
    /// RU: читаемое имя шага.
    fn name(&self) -> &'static str {
        "unnamed step"
    }
}

/// The wire-side endpoints the future wiring will connect to.
/// EN: kept here so `horizon-protocol` can depend on this crate without cycles.
/// RU: точки подключения, через которые позже horizon-protocol завяжет пайплайн.
pub trait ServerboundInjector {
    /// Feed a translated client packet (at the server's version) to the wire.
    /// RU: отправить транслированный serverbound-пакет в сокет.
    fn send_raw(&mut self, bytes: &[u8]) -> Result<(), errors::ViaError>;
}

/// Sink for packets that were translated UP to 26.3 and are handed to the client.
/// RU: приёмник пакетов, транслированных до 26.3 и отданных клиенту.
pub trait ClientboundSink {
    fn deliver(&mut self, bytes: &[u8]) -> Result<(), errors::ViaError>;
}
