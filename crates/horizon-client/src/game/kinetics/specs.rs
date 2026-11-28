//! Core Create kinetic animation specifications.
use glam::Vec3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartModel {
    /// Render the blockstate's own model from catalog (for shaft, cogwheel, crushing wheel, etc.)
    BlockState,
    /// Render a specific partial model from ModelLibrary (e.g. "create:block/encased_fan/propeller")
    Partial(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToothPhase {
    None,
    SmallCog,
    LargeCog,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AxisSource {
    /// Read from "axis" property ("x", "y", "z")
    BlockAxis,
    /// Native +Y partial model rotated to align with "facing" (e.g. water_wheel)
    BlockFacingY,
    /// Native +Z partial model rotated to align with "facing" (e.g. fan propeller, drill, shaft_half)
    BlockFacingZ,
    /// Native +Z partial model rotated opposite to "facing" (e.g. wall/surface-mounted hand_crank)
    BlockFacingOppositeZ,
    /// Native +Y partial model oriented by "face" ("floor"/"ceiling"/"wall") and "facing" (e.g. steam_engine).
    BlockAttachFaceY,
    /// Native +Y partial model along the shaft that "facing" and "axis_along_first"
    /// pick (`DirectionalAxisKineticBlock`, e.g. gauges).
    BlockDirectionalAxis,
    /// Blockstate model rotated around the axis of its "facing" property (e.g. valve handles).
    BlockFacingAxis,
    /// Fixed axis
    Fixed(Vec3),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    Spin {
        axis: AxisSource,
        speed_mult: f32,
        tooth_phase: ToothPhase,
    },
    Oscillate {
        direction: AxisSource,
        amplitude: f32,
        freq_mult: f32,
    },
    SpinAndOscillate {
        axis: AxisSource,
        speed_mult: f32,
        osc_direction: AxisSource,
        amplitude: f32,
        freq_mult: f32,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct PartSpec {
    pub model: PartModel,
    pub motion: Motion,
    pub pivot: Vec3,
}

#[derive(Clone, Copy, Debug)]
pub struct AnimBlockSpec {
    pub block_name: &'static str,
    pub omit_chunk_mesh: bool,
    pub parts: &'static [PartSpec],
}

pub static SHAFT: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:shaft",
    omit_chunk_mesh: true,
    parts: &[PartSpec {
        model: PartModel::BlockState,
        motion: Motion::Spin {
            axis: AxisSource::BlockAxis,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static COGWHEEL: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:cogwheel",
    omit_chunk_mesh: true,
    parts: &[PartSpec {
        model: PartModel::BlockState,
        motion: Motion::Spin {
            axis: AxisSource::BlockAxis,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::SmallCog,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static LARGE_COGWHEEL: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:large_cogwheel",
    omit_chunk_mesh: true,
    parts: &[PartSpec {
        model: PartModel::BlockState,
        motion: Motion::Spin {
            axis: AxisSource::BlockAxis,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::LargeCog,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static CRUSHING_WHEEL: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:crushing_wheel",
    omit_chunk_mesh: true,
    parts: &[PartSpec {
        model: PartModel::BlockState,
        motion: Motion::Spin {
            axis: AxisSource::BlockAxis,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static ENCASED_FAN: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:encased_fan",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/encased_fan/propeller"),
        motion: Motion::Spin {
            // `EncasedFanRenderer`: `partialFacing(ENCASED_FAN_INNER, state,
            // direction.getOpposite())`; `FanVisual`: `rotateToFace(SOUTH, opposite)`.
            axis: AxisSource::BlockFacingOppositeZ,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static MECHANICAL_DRILL: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:mechanical_drill",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/mechanical_drill/head"),
        motion: Motion::Spin {
            axis: AxisSource::BlockFacingZ,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static MECHANICAL_PRESS: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:mechanical_press",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/mechanical_press/head"),
        motion: Motion::Oscillate {
            direction: AxisSource::Fixed(Vec3::NEG_Y),
            amplitude: 0.6,
            freq_mult: 1.0,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static MECHANICAL_MIXER: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:mechanical_mixer",
    omit_chunk_mesh: false,
    parts: &[
        PartSpec {
            model: PartModel::Partial("create:block/mechanical_mixer/pole"),
            motion: Motion::Oscillate {
                direction: AxisSource::Fixed(Vec3::NEG_Y),
                amplitude: 0.25,
                freq_mult: 0.5,
            },
            pivot: Vec3::splat(0.5),
        },
        PartSpec {
            model: PartModel::Partial("create:block/mechanical_mixer/head"),
            motion: Motion::SpinAndOscillate {
                axis: AxisSource::Fixed(Vec3::Y),
                speed_mult: 2.0,
                osc_direction: AxisSource::Fixed(Vec3::NEG_Y),
                amplitude: 0.25,
                freq_mult: 0.5,
            },
            pivot: Vec3::splat(0.5),
        },
    ],
};

pub static MILLSTONE: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:millstone",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/millstone/inner"),
        motion: Motion::Spin {
            axis: AxisSource::Fixed(Vec3::Y),
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static WATER_WHEEL: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:water_wheel",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/water_wheel/wheel"),
        motion: Motion::Spin {
            axis: AxisSource::BlockFacingY,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static LARGE_WATER_WHEEL: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:large_water_wheel",
    omit_chunk_mesh: true,
    parts: &[PartSpec {
        model: PartModel::BlockState,
        motion: Motion::Spin {
            axis: AxisSource::BlockAxis,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

static DIRECTIONAL_SHAFT_PARTS: &[PartSpec] = &[PartSpec {
    model: PartModel::Partial("create:block/shaft"),
    motion: Motion::Spin {
        axis: AxisSource::BlockDirectionalAxis,
        speed_mult: 1.0,
        tooth_phase: ToothPhase::SmallCog,
    },
    pivot: Vec3::splat(0.5),
}];

pub static DEPLOYER: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:deployer",
    omit_chunk_mesh: false,
    parts: DIRECTIONAL_SHAFT_PARTS,
};

static MECHANICAL_PISTON: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:mechanical_piston",
    omit_chunk_mesh: false,
    parts: DIRECTIONAL_SHAFT_PARTS,
};

pub static HAND_CRANK: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:hand_crank",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/hand_crank/handle"),
        motion: Motion::Spin {
            axis: AxisSource::BlockFacingOppositeZ,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static TURNTABLE: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:turntable",
    omit_chunk_mesh: true,
    parts: &[PartSpec {
        model: PartModel::BlockState,
        motion: Motion::Spin {
            axis: AxisSource::Fixed(Vec3::Y),
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static FLYWHEEL: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:flywheel",
    omit_chunk_mesh: true,
    parts: &[PartSpec {
        model: PartModel::BlockState,
        motion: Motion::Spin {
            axis: AxisSource::BlockAxis,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static POWERED_SHAFT: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:powered_shaft",
    omit_chunk_mesh: true,
    parts: &[PartSpec {
        model: PartModel::BlockState,
        motion: Motion::Spin {
            axis: AxisSource::BlockAxis,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static BEARING_SPEC: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:mechanical_bearing",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/bearing/top"),
        motion: Motion::Spin {
            axis: AxisSource::BlockFacingY,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static ENCASED_SHAFT_SPEC: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:andesite_encased_shaft",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/shaft"),
        motion: Motion::Spin {
            axis: AxisSource::BlockAxis,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static GANTRY_SHAFT: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:gantry_shaft",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/shaft"),
        motion: Motion::Spin {
            axis: AxisSource::BlockFacingY,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static ENCASED_COGWHEEL_SPEC: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:andesite_encased_cogwheel",
    omit_chunk_mesh: false,
    parts: &[
        PartSpec {
            model: PartModel::Partial("create:block/cogwheel_shaftless"),
            motion: Motion::Spin {
                axis: AxisSource::BlockAxis,
                speed_mult: 1.0,
                tooth_phase: ToothPhase::SmallCog,
            },
            pivot: Vec3::splat(0.5),
        },
        PartSpec {
            model: PartModel::Partial("create:block/shaft"),
            motion: Motion::Spin {
                axis: AxisSource::BlockAxis,
                speed_mult: 1.0,
                tooth_phase: ToothPhase::None,
            },
            pivot: Vec3::splat(0.5),
        },
    ],
};

pub static ENCASED_LARGE_COGWHEEL_SPEC: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:andesite_encased_large_cogwheel",
    omit_chunk_mesh: false,
    parts: &[
        PartSpec {
            model: PartModel::Partial("create:block/large_cogwheel_shaftless"),
            motion: Motion::Spin {
                axis: AxisSource::BlockAxis,
                speed_mult: 1.0,
                tooth_phase: ToothPhase::LargeCog,
            },
            pivot: Vec3::splat(0.5),
        },
        PartSpec {
            model: PartModel::Partial("create:block/shaft"),
            motion: Motion::Spin {
                axis: AxisSource::BlockAxis,
                speed_mult: 1.0,
                tooth_phase: ToothPhase::None,
            },
            pivot: Vec3::splat(0.5),
        },
    ],
};

pub static STEAM_ENGINE: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:steam_engine",
    omit_chunk_mesh: false,
    parts: &[
        PartSpec {
            model: PartModel::Partial("create:block/steam_engine/piston"),
            motion: Motion::Oscillate {
                direction: AxisSource::BlockAttachFaceY,
                amplitude: 0.25,
                freq_mult: 1.0,
            },
            pivot: Vec3::splat(0.5),
        },
        PartSpec {
            model: PartModel::Partial("create:block/steam_engine/linkage"),
            motion: Motion::Oscillate {
                direction: AxisSource::BlockAttachFaceY,
                amplitude: 0.25,
                freq_mult: 1.0,
            },
            pivot: Vec3::splat(0.5),
        },
    ],
};

pub static MECHANICAL_ARM: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:mechanical_arm",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/mechanical_arm/cog"),
        motion: Motion::Spin {
            axis: AxisSource::Fixed(Vec3::Y),
            speed_mult: 1.0,
            tooth_phase: ToothPhase::SmallCog,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static SPOUT: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:spout",
    omit_chunk_mesh: false,
    parts: &[],
};

/// `PumpRenderer`: the cog turns about the facing axis. Its model is a
/// disc symmetric across its axis, so either way along it looks the same.
pub static MECHANICAL_PUMP: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:mechanical_pump",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/mechanical_pump/cog"),
        motion: Motion::Spin {
            axis: AxisSource::BlockFacingZ,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

/// Speed and stress gauges' shaft (`GaugeRenderer` draws a shaft along the
/// gauge's axis); their heads and dials are drawn by `instruments`.
pub static GAUGE: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:speedometer",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/shaft"),
        motion: Motion::Spin {
            axis: AxisSource::BlockDirectionalAxis,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

/// The cuckoo clock's half shaft out of its back (`CuckooClockRenderer`);
/// its hands are drawn by `instruments`.
pub static CUCKOO_CLOCK: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:cuckoo_clock",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/shaft_half"),
        motion: Motion::Spin {
            axis: AxisSource::BlockFacingOppositeZ,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

/// Blocks whose moving parts `machines` draws; their shafts are drawn there
/// too, so the generic part list is empty.
pub static MACHINE_SPEC: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:mechanical_saw",
    omit_chunk_mesh: false,
    parts: &[],
};

/// `CreativeMotorRenderer`: a half shaft out of its facing.
pub static CREATIVE_MOTOR: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:creative_motor",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create:block/shaft_half"),
        motion: Motion::Spin {
            axis: AxisSource::BlockFacingZ,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

/// `HandCrankRenderer` for valve handles: the full blockstate spins around its facing axis.
pub static VALVE_HANDLE: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:copper_valve_handle",
    omit_chunk_mesh: true,
    parts: &[PartSpec {
        model: PartModel::BlockState,
        motion: Motion::Spin {
            axis: AxisSource::BlockFacingAxis,
            speed_mult: 1.0,
            tooth_phase: ToothPhase::None,
        },
        pivot: Vec3::splat(0.5),
    }],
};

/// Looks up the kinetic animation specification for Create core blocks.
pub fn core_spec(name: &str) -> Option<&'static AnimBlockSpec> {
    match name {
        "create:shaft" => Some(&SHAFT),
        "create:cogwheel" => Some(&COGWHEEL),
        "create:large_cogwheel" => Some(&LARGE_COGWHEEL),
        "create:crushing_wheel" => Some(&CRUSHING_WHEEL),
        "create:encased_fan" => Some(&ENCASED_FAN),
        "create:mechanical_drill" => Some(&MECHANICAL_DRILL),
        "create:mechanical_press" => Some(&MECHANICAL_PRESS),
        "create:mechanical_mixer" => Some(&MECHANICAL_MIXER),
        "create:millstone" => Some(&MILLSTONE),
        "create:water_wheel" => Some(&WATER_WHEEL),
        "create:large_water_wheel" => Some(&LARGE_WATER_WHEEL),
        "create:deployer" => Some(&DEPLOYER),
        "create:mechanical_piston" | "create:sticky_mechanical_piston" => Some(&MECHANICAL_PISTON),
        super::extra_addons::ORE_KINETIC_INPUT
        | super::extra_addons::PRINTER
        | super::extra_addons::CARBON_BRUSHES => Some(&MACHINE_SPEC),
        "create:hand_crank" => Some(&HAND_CRANK),
        "create:turntable" => Some(&TURNTABLE),
        "create:flywheel" => Some(&FLYWHEEL),
        "create:powered_shaft" => Some(&POWERED_SHAFT),
        "create:mechanical_bearing" | "create:windmill_bearing" | "create:clockwork_bearing" => {
            Some(&BEARING_SPEC)
        }
        "create:andesite_encased_shaft"
        | "create:brass_encased_shaft"
        | "create:clutch"
        | "create:gearshift"
        | "create:encased_chain_drive"
        | "create:adjustable_chain_gearshift" => Some(&ENCASED_SHAFT_SPEC),
        "create:gantry_shaft" => Some(&GANTRY_SHAFT),
        "create:andesite_encased_cogwheel" | "create:brass_encased_cogwheel" => {
            Some(&ENCASED_COGWHEEL_SPEC)
        }
        "create:andesite_encased_large_cogwheel" | "create:brass_encased_large_cogwheel" => {
            Some(&ENCASED_LARGE_COGWHEEL_SPEC)
        }
        "create:steam_engine" => Some(&STEAM_ENGINE),
        "create:creative_motor" => Some(&CREATIVE_MOTOR),
        "create:mechanical_arm" => Some(&MECHANICAL_ARM),
        "create:mechanical_saw"
        | "create:mechanical_harvester"
        | "create:rope_pulley"
        | "create:hose_pulley"
        | "create:elevator_pulley"
        | "create:gearbox"
        | "create:sequenced_gearshift"
        | "create:rotation_speed_controller"
        | "create:copper_backtank"
        | "create:netherite_backtank"
        | "create:mechanical_crafter"
        | "create:display_board"
        | "create:gantry_carriage"
        | "create:steam_whistle" => Some(&MACHINE_SPEC),
        "create:spout" => Some(&SPOUT),
        "create:mechanical_pump" => Some(&MECHANICAL_PUMP),
        "create:speedometer" | "create:stressometer" => Some(&GAUGE),
        "create:cuckoo_clock" | "create:mysterious_cuckoo_clock" => Some(&CUCKOO_CLOCK),
        _ if super::levers::is_valve_handle(name) => Some(&VALVE_HANDLE),
        _ if super::levers::is_aux_block(name) => Some(&MACHINE_SPEC),
        _ if super::logistics::is_logistics_block(name) => Some(&MACHINE_SPEC),
        _ if super::contraptions::is_contraption_block(name) => Some(&MACHINE_SPEC),
        _ if super::indicators::is_nixie_block(name) => Some(&MACHINE_SPEC),
        _ => None,
    }
}
