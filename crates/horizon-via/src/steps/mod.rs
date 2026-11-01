//! Per-version translation steps (the ladder rungs).
//!
//! EN: each file here owns one family of PLAY-state steps and implements
//! [`crate::Step`]. The (future) pipeline in `horizon-protocol` chains them
//! bottom-up; within this crate they only declare and parse.
//!
//! RU: пошаговые трансляции по семьям версий. Пайплайн соединит их позже;
//! внутри крейта шаги только объявляют себя и разбирают пакеты.
//!
//! FIXME(wiring): the orchestrator declares each step module here as
//! parallel coders land them; keep the list alphabetical to make merge
//! conflicts one-line affairs.

pub mod play_ancient;
pub mod play_classic;
