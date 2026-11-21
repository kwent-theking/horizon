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
            axis: AxisSource::BlockFacingZ,
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

pub static DEPLOYER: AnimBlockSpec = AnimBlockSpec {
    block_name: "create:deployer",
    omit_chunk_mesh: false,
    parts: &[
        PartSpec {
            model: PartModel::Partial("create:block/deployer/pole"),
            motion: Motion::Oscillate {
                direction: AxisSource::BlockFacingZ,
                amplitude: 0.5,
                freq_mult: 1.0,
            },
            pivot: Vec3::splat(0.5),
        },
        PartSpec {
            model: PartModel::Partial("create:block/deployer/hand_punching"),
            motion: Motion::Oscillate {
                direction: AxisSource::BlockFacingZ,
                amplitude: 0.5,
                freq_mult: 1.0,
            },
            pivot: Vec3::splat(0.5),
        },
    ],
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
    parts: &[
        PartSpec {
            model: PartModel::Partial("create:block/spout/middle"),
            motion: Motion::Oscillate {
                direction: AxisSource::Fixed(Vec3::NEG_Y),
                amplitude: 0.12,
                freq_mult: 1.0,
            },
            pivot: Vec3::splat(0.5),
        },
        PartSpec {
            model: PartModel::Partial("create:block/spout/bottom"),
            motion: Motion::Oscillate {
                direction: AxisSource::Fixed(Vec3::NEG_Y),
                amplitude: 0.24,
                freq_mult: 1.0,
            },
            pivot: Vec3::splat(0.5),
        },
    ],
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
        "create:hand_crank" => Some(&HAND_CRANK),
        "create:turntable" => Some(&TURNTABLE),
        "create:flywheel" => Some(&FLYWHEEL),
        "create:powered_shaft" => Some(&POWERED_SHAFT),
        "create:mechanical_bearing"
        | "create:windmill_bearing"
        | "create:clockwork_bearing" => Some(&BEARING_SPEC),
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
        "create:mechanical_arm" => Some(&MECHANICAL_ARM),
        "create:spout" => Some(&SPOUT),
        _ => None,
    }
}
