/// Client-side chaser for the moving mouth of a placed steam whistle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WhistleState {
    pub previous: f32,
    pub progress: f32,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackSignalState {
    Red,
    Yellow,
    Green,
    Invalid,
}

/// One ComputerCraft-controlled Nixie lamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NixieLamp {
    pub rgb: u32,
    pub blink_period: i8,
    pub blink_off_time: i8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NixieState {
    pub characters: [char; 2],
    pub computer_signal: Option<[NixieLamp; 2]>,
}

impl Default for NixieState {
    fn default() -> Self {
        Self {
            characters: ['0'; 2],
            computer_signal: None,
        }
    }
}
