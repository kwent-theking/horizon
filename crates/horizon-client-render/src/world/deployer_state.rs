use horizon_items::items::ItemStack;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeployerMode {
    #[default]
    Use,
    Punch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeployerPhase {
    #[default]
    Waiting,
    Expanding,
    Retracting,
    Dumping,
}

/// Client animation state for a placed Create deployer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeployerState {
    pub mode: DeployerMode,
    pub phase: DeployerPhase,
    pub timer: i32,
    pub reach: f32,
    pub fist_bump: bool,
    pub held_item: Option<ItemStack>,
}
