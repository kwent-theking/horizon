//! How far the first start has got, for the loading screen. The content root
//! build and the resource loader report here from their own threads; the
//! window reads [`fraction`] and [`line`] every frame.
//!
//! A cold start spends minutes in steps a warm one passes in seconds, so the
//! bar is weighted by what each step took in a measured cold start: the
//! slow steps take most of the bar, and a step a warm start skips just jumps.
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

/// The steps of a start, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    ModList,
    ModLinks,
    ModTables,
    ModFiles,
    ModTags,
    Menu,
    Models,
    Blocks,
    ItemModels,
    Items,
    World,
}

const STEPS: [Step; 11] = [
    Step::ModList,
    Step::ModLinks,
    Step::ModTables,
    Step::ModFiles,
    Step::ModTags,
    Step::Menu,
    Step::Models,
    Step::Blocks,
    Step::ItemModels,
    Step::Items,
    Step::World,
];

impl Step {
    fn index(self) -> usize {
        STEPS.iter().position(|&s| s == self).unwrap_or(0)
    }

    /// Seconds the step takes in a cold start of the 128-mod pack: measured
    /// on 2026-11-22, with the unpacking and the item model, model and recipe
    /// reads cut to what is left once models, item definitions, blockstates
    /// and recipes are served from the archives (~40% of the files written).
    fn weight(self) -> f64 {
        match self {
            Step::ModList => 9.0,
            Step::ModLinks => 20.0,
            Step::ModTables => 12.0,
            Step::ModFiles => 230.0,
            Step::ModTags => 11.0,
            Step::Menu => 25.0,
            Step::Models => 40.0,
            Step::Blocks => 28.0,
            Step::ItemModels => 15.0,
            Step::Items => 53.0,
            Step::World => 30.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Step::ModList => "Reading the mods",
            Step::ModLinks => "Linking the game files",
            Step::ModTables => "Writing the block tables",
            Step::ModFiles => "Unpacking the mods",
            Step::ModTags => "Merging tags",
            Step::Menu => "Loading the menu",
            Step::Models => "Building block models",
            Step::Blocks => "Loading blocks",
            Step::ItemModels => "Loading item models",
            Step::Items => "Loading items",
            Step::World => "Loading recipes and world data",
        }
    }
}

/// Where a start is: the step running (`None` before the first, `Some(STEPS.len())`
/// when all are done) and, within it, `done` of `total` units (files) when it counts any.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapshot {
    pub step: Option<usize>,
    pub done: u64,
    pub total: u64,
}

impl Snapshot {
    /// The share of a cold start behind it, 0 to 1.
    pub fn fraction(self) -> f32 {
        let Some(step) = self.step else {
            return 0.0;
        };
        let total: f64 = STEPS.iter().map(|s| s.weight()).sum();
        let before: f64 = STEPS.iter().take(step).map(|s| s.weight()).sum();
        let within = match STEPS.get(step) {
            Some(current) if self.total > 0 => {
                current.weight() * (self.done.min(self.total) as f64 / self.total as f64)
            }
            _ => 0.0,
        };
        ((before + within) / total) as f32
    }

    /// The step's name, its count when it has one, and the share done.
    pub fn line(self) -> String {
        let percent = (self.fraction() * 100.0).floor() as u32;
        match self.step.and_then(|step| STEPS.get(step)) {
            None if self.step.is_some() => format!("Ready ({percent}%)"),
            None => "Starting".to_owned(),
            Some(step) if self.total > 0 => format!(
                "{} {}/{} ({percent}%)",
                step.label(),
                self.done.min(self.total),
                self.total
            ),
            Some(step) => format!("{} ({percent}%)", step.label()),
        }
    }
}

/// The step running plus one: 0 before the first.
static STEP: AtomicUsize = AtomicUsize::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
static TOTAL: AtomicU64 = AtomicU64::new(0);

/// `step` starts, with `total` units to count (0: none). Steps only move
/// forward: a later loader going over an earlier step changes nothing.
pub fn begin(step: Step, total: u64) {
    let next = step.index() + 1;
    if STEP.fetch_max(next, Relaxed) <= next {
        DONE.store(0, Relaxed);
        TOTAL.store(total, Relaxed);
    }
}

/// `units` more of the running step's total are done.
pub fn advance(units: u64) {
    DONE.fetch_add(units, Relaxed);
}

/// Every step is done.
pub fn finish() {
    STEP.store(STEPS.len() + 1, Relaxed);
    TOTAL.store(0, Relaxed);
}

/// Where the start is now.
pub fn snapshot() -> Snapshot {
    Snapshot {
        step: STEP.load(Relaxed).checked_sub(1),
        done: DONE.load(Relaxed),
        total: TOTAL.load(Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(step: Step, done: u64, total: u64) -> Snapshot {
        Snapshot {
            step: Some(step.index()),
            done,
            total,
        }
    }

    #[test]
    fn the_bar_follows_the_cold_start_weights() {
        assert_eq!(Snapshot { step: None, done: 0, total: 0 }.fraction(), 0.0);
        assert_eq!(at(Step::ModList, 0, 0).fraction(), 0.0);
        let half_files = at(Step::ModFiles, 50, 100).fraction();
        let start_files = at(Step::ModFiles, 0, 100).fraction();
        let after_files = at(Step::ModTags, 0, 0).fraction();
        assert!(start_files < half_files && half_files < after_files);
        // Unpacking is the biggest step of a cold start.
        assert!(after_files - start_files > 0.4);
        let done = Snapshot {
            step: Some(STEPS.len()),
            done: 0,
            total: 0,
        };
        assert!((done.fraction() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn counts_never_overshoot_their_step() {
        assert_eq!(at(Step::ModFiles, 500, 100).fraction(), at(Step::ModTags, 0, 0).fraction());
    }

    #[test]
    fn the_line_names_the_step_and_its_count() {
        assert_eq!(Snapshot { step: None, done: 0, total: 0 }.line(), "Starting");
        let line = at(Step::ModFiles, 12, 80).line();
        assert!(line.starts_with("Unpacking the mods 12/80 ("), "{line}");
        assert!(at(Step::Models, 0, 0).line().starts_with("Building block models ("));
        let done = Snapshot {
            step: Some(STEPS.len()),
            done: 0,
            total: 0,
        };
        assert_eq!(done.line(), "Ready (100%)");
    }
}
