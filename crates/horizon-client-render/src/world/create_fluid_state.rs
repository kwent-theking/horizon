use super::LerpedValue;

#[derive(Clone, Debug, PartialEq)]
pub struct CreateFluid {
    pub id: String,
    pub amount: u32,
    pub tint: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreateFluidTank {
    pub fluid: Option<CreateFluid>,
    pub level: LerpedValue,
    pub width: u8,
    pub height: u8,
    pub window: bool,
    pub creative: bool,
    pub controller: Option<[i32; 3]>,
    pub boiler_active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreateFluidFlow {
    pub fluid: CreateFluid,
    pub inward: bool,
    pub progress: LerpedValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateVesselKind {
    Spout,
    Drain,
    FragileTank,
    LevititeTank,
}

impl CreateVesselKind {
    pub fn block(self) -> &'static str {
        match self {
            Self::Spout => "create:spout",
            Self::Drain => "create:item_drain",
            Self::FragileTank => "create_dragons_plus:fragile_fluid_tank",
            Self::LevititeTank => "create_dragons_plus:levitite_fragile_fluid_tank",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CreateFluidState {
    Tank(CreateFluidTank),
    Basin(Vec<CreateFluid>),
    Vessel {
        fluid: Option<CreateFluid>,
        level: LerpedValue,
        processing: i32,
        kind: CreateVesselKind,
        stream_fluid: Option<CreateFluid>,
    },
    Pipe(Box<[Option<CreateFluidFlow>; 6]>),
}

impl CreateFluidState {
    pub fn matches_block(&self, name: &str) -> bool {
        let expected = match self {
            Self::Tank(tank) if tank.creative => "create:creative_fluid_tank",
            Self::Tank(_) => "create:fluid_tank",
            Self::Basin(_) => "create:basin",
            Self::Vessel { kind, .. } => kind.block(),
            Self::Pipe(_) => "create:glass_fluid_pipe",
        };
        expected == name
    }
}
