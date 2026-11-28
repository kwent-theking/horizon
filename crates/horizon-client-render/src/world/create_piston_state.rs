/// Render state for the piston-owned head and extension poles of a placed Create piston.
#[derive(Clone, Copy, Debug)]
pub struct CreatePistonState {
    pub extension_length: i32,
    pub synchronized_offset: f64,
    pub previous_offset: f64,
    pub offset: f64,
    pub running: bool,
    pub prediction_ticks_left: u8,
    pub sequenced_offset_limit: Option<f64>,
}

impl CreatePistonState {
    pub fn render_reach(&self) -> f32 {
        if !self.running {
            return 0.0;
        }
        let front = self.previous_offset.max(self.offset);
        let back = f64::from(self.extension_length) - self.previous_offset.min(self.offset);
        front.max(back) as f32
    }
}
