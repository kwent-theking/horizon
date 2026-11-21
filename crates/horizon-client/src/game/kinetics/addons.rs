//! Create addon kinetic animation specifications.
use super::specs::{AnimBlockSpec, AxisSource, Motion, PartModel, PartSpec, ToothPhase};
use glam::Vec3;

pub static ELECTRIC_MOTOR: AnimBlockSpec = AnimBlockSpec {
    block_name: "createaddition:electric_motor",
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

pub static ALTERNATOR: AnimBlockSpec = AnimBlockSpec {
    block_name: "createaddition:alternator",
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

pub static ROLLING_MILL: AnimBlockSpec = AnimBlockSpec {
    block_name: "createaddition:rolling_mill",
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

pub static EXTRUDER: AnimBlockSpec = AnimBlockSpec {
    block_name: "create_mechanical_extruder:mechanical_extruder",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial("create_mechanical_extruder:block/mechanical_extruder/pole"),
        motion: Motion::Oscillate {
            direction: AxisSource::Fixed(Vec3::NEG_Y),
            amplitude: 0.4,
            freq_mult: 1.0,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static BRASS_EXTRUDER: AnimBlockSpec = AnimBlockSpec {
    block_name: "create_mechanical_extruder:mechanical_brass_extruder",
    omit_chunk_mesh: false,
    parts: &[PartSpec {
        model: PartModel::Partial(
            "create_mechanical_extruder:block/mechanical_brass_extruder/pole",
        ),
        motion: Motion::Oscillate {
            direction: AxisSource::Fixed(Vec3::NEG_Y),
            amplitude: 0.4,
            freq_mult: 1.0,
        },
        pivot: Vec3::splat(0.5),
    }],
};

pub static MECHANICAL_GRINDSTONE: AnimBlockSpec = AnimBlockSpec {
    block_name: "create_enchantment_industry:mechanical_grindstone",
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

pub static BASIC_MOTOR: AnimBlockSpec = AnimBlockSpec {
    block_name: "create_new_age:basic_motor",
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

pub static ADVANCED_MOTOR: AnimBlockSpec = AnimBlockSpec {
    block_name: "create_new_age:advanced_motor",
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

pub static REINFORCED_MOTOR: AnimBlockSpec = AnimBlockSpec {
    block_name: "create_new_age:reinforced_motor",
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

pub static GENERATOR_COIL: AnimBlockSpec = AnimBlockSpec {
    block_name: "create_new_age:generator_coil",
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

/// Looks up the kinetic animation specification for Create addon blocks.
pub fn addon_spec(name: &str) -> Option<&'static AnimBlockSpec> {
    match name {
        "createaddition:electric_motor" => Some(&ELECTRIC_MOTOR),
        "createaddition:alternator" => Some(&ALTERNATOR),
        "createaddition:rolling_mill" => Some(&ROLLING_MILL),
        "create_mechanical_extruder:mechanical_extruder" => Some(&EXTRUDER),
        "create_mechanical_extruder:mechanical_brass_extruder" => Some(&BRASS_EXTRUDER),
        "create_enchantment_industry:mechanical_grindstone" => Some(&MECHANICAL_GRINDSTONE),
        "create_new_age:basic_motor" => Some(&BASIC_MOTOR),
        "create_new_age:advanced_motor" => Some(&ADVANCED_MOTOR),
        "create_new_age:reinforced_motor" => Some(&REINFORCED_MOTOR),
        "create_new_age:generator_coil" => Some(&GENERATOR_COIL),
        _ => None,
    }
}
