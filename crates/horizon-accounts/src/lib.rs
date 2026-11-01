//! `horizon-accounts`: in-game account switcher for the Horizon client.
//!
//! EN: a port of The-Fireplace/In-Game-Account-Switcher for the Horizon Rust
//! client, with a CLI front-end instead of an in-game screen (Horizon's
//! in-game UI is a separate concern; this crate is the logic layer, ready to
//! be embedded). Flow, endpoints and file formats are lifted from
//! `horizon-release/horizon-account-switcher.py`:
//!
//! 1. MSA device-code sign-in (`login.microsoftonline.com/consumers`,
//!    scope `XboxLive.signin offline_access`);
//! 2. XBL -> XSTS -> `api.minecraftservices.com/launcher/login`
//!    (legacy `/authentication/login_with_xbox` fallback);
//! 3. profile from `/minecraft/profile`;
//! 4. accounts stored in `accounts.json` (tokens as-is, OS per-user ACL is
//!    the protection boundary — no DPAPI layer);
//! 5. a fresh `session-<hex>.json` (`name`/`uuid`/`access_token`/
//!    `user_type:"msa"`/`xuid`) for `horizon-client --session-file`; the
//!    client refuses files older than 120 s and deletes them after reading.
//!
//! The library never prints: user instructions go through `notify`
//! callbacks. The binary (`horizon-switcher`) is the printing front-end.
//!
//! Networking lives behind the [`http::Http`] trait; with the default
//! `client` feature it is ureq-over-TLS, without it the crate compiles
//! fully offline with stub transports (tests always use fakes).
//!
//! RU: порт In-Game-Account-Switcher для клиента Horizon — логика входа и
//! переключения аккаунтов Microsoft с CLI-обёрткой. Библиотека ничего не
//! печатает (инструкция — через callback `notify`), сеть — только за трейтом
//! [`http::Http`], без feature `client` крейт собирается оффлайн.

pub mod error;
pub mod http;
pub mod json;
pub mod msa;
pub mod profile;
pub mod session_file;
pub mod store;
pub mod xbox;

pub use error::AcctError;
pub use http::{default_http, Http};
pub use msa::MsaTokens;
pub use profile::Profile;
pub use store::{Account, AccountStore};
