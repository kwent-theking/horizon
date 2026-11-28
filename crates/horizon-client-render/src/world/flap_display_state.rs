/// Synchronized rows on the controller of a Create display board.
#[derive(Clone, Debug, Default)]
pub struct FlapDisplayState {
    pub width: u8,
    pub height: u8,
    pub rows: Vec<FlapDisplayRow>,
}

#[derive(Clone, Debug)]
pub struct FlapDisplayRow {
    pub key: String,
    pub color: u32,
    pub glowing: bool,
    pub sections: Vec<FlapDisplaySection>,
}

#[derive(Clone, Debug)]
pub struct FlapDisplaySection {
    pub width: f32,
    pub gap: f32,
    /// Zero for a single proportional flap, else the advance of one flap.
    pub cell_width: f32,
    pub characters: Vec<char>,
    pub transition: Option<FlapDisplayTransition>,
}

#[derive(Clone, Debug)]
pub struct FlapDisplayTransition {
    pub since: i64,
    pub previous: Vec<char>,
    pub options: Vec<Vec<char>>,
}
