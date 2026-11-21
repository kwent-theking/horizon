//! The registry of player-animation clips, read from mod content.
//!
//! `assets/<namespace>/player_animations/**/*.json` (and the legacy
//! `player_animation`) hold Emotecraft-style and Gecko-style files; a clip's id
//! is `namespace:name`. Namespaces and files load in sorted order and a later
//! clip replaces an earlier one with the same id, so the result does not depend
//! on directory iteration order.

use super::format::{AnimationClip, parse_emotecraft, parse_gecko};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

const FOLDERS: [&str; 2] = ["player_animation", "player_animations"];

#[derive(Clone, Debug, Default)]
pub struct AnimationLibrary {
    clips: HashMap<String, Arc<AnimationClip>>,
}

/// What a [`AnimationLibrary::load_resources`] run found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    pub files: usize,
    pub clips: usize,
    /// Ids that a later file replaced.
    pub replaced: Vec<String>,
    pub errors: Vec<String>,
}

impl AnimationLibrary {
    pub fn get(&self, id: &str) -> Option<Arc<AnimationClip>> {
        self.clips.get(id).cloned()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.clips.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// Registers `clip` as `namespace:name`; true when it replaced one.
    pub fn insert(&mut self, namespace: &str, clip: AnimationClip) -> bool {
        let id = format!("{namespace}:{}", clip.name);
        self.clips.insert(id, Arc::new(clip)).is_some()
    }

    /// Parses one file's text (either schema) into this library.
    pub fn load_text(
        &mut self,
        namespace: &str,
        file_stem: &str,
        text: &str,
        report: &mut LoadReport,
    ) {
        let parsed = if text.contains("\"emote\"") {
            parse_emotecraft(text).map(|mut clip| {
                if clip.name.is_empty() {
                    clip.name = file_stem.to_owned();
                }
                vec![clip]
            })
        } else {
            parse_gecko(text)
        };
        match parsed {
            Ok(clips) => {
                report.files += 1;
                for clip in clips {
                    let id = format!("{namespace}:{}", clip.name);
                    if self.insert(namespace, clip) {
                        report.replaced.push(id);
                    }
                    report.clips += 1;
                }
            }
            Err(e) => report
                .errors
                .push(format!("{namespace}:{file_stem}: {e}")),
        }
    }

    /// Reads every namespace under `root/assets`.
    pub fn load_resources(root: &Path) -> (Self, LoadReport) {
        let mut library = Self::default();
        let mut report = LoadReport::default();
        for namespace_dir in sorted_dirs(&root.join("assets")) {
            let Some(namespace) = namespace_dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            for folder in FOLDERS {
                for file in json_files(&namespace_dir.join(folder)) {
                    let stem = file
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default();
                    match fs::read_to_string(&file) {
                        Ok(text) => library.load_text(namespace, stem, &text, &mut report),
                        Err(e) => report.errors.push(format!("{}: {e}", file.display())),
                    }
                }
            }
        }
        (library, report)
    }
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_json(dir, &mut files);
    files.sort();
    files
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() {
            collect_json(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GECKO: &str = r#"{"animations":{
        "scythe_backpedal":{"animation_length":1,"bones":{"head":{"rotation":[1,0,0]}}},
        "other":{"animation_length":1}}}"#;
    const GECKO_LATER: &str =
        r#"{"animations":{"scythe_backpedal":{"animation_length":2,"bones":{}}}}"#;
    const EMOTE: &str =
        r#"{"name":"waving","emote":{"endTick":10,"moves":[{"tick":0,"head":{"yaw":1}}]}}"#;

    #[test]
    fn ids_combine_namespace_and_clip_name() {
        let mut library = AnimationLibrary::default();
        let mut report = LoadReport::default();
        library.load_text("irons_spellbooks", "casting", GECKO, &mut report);
        library.load_text("irons_spellbooks", "waving_file", EMOTE, &mut report);
        assert!(library.contains("irons_spellbooks:scythe_backpedal"));
        assert!(library.contains("irons_spellbooks:waving"));
        assert_eq!(report.files, 2);
        assert_eq!(report.clips, 3);
    }

    #[test]
    fn unnamed_emotes_take_the_file_name() {
        let mut library = AnimationLibrary::default();
        let mut report = LoadReport::default();
        let text = r#"{"emote":{"endTick":10,"moves":[]}}"#;
        library.load_text("bettercombat", "two_handed_slam", text, &mut report);
        assert!(library.contains("bettercombat:two_handed_slam"));
    }

    #[test]
    fn a_later_file_replaces_a_duplicate_id() {
        let mut library = AnimationLibrary::default();
        let mut report = LoadReport::default();
        library.load_text("irons_spellbooks", "casting_animations", GECKO, &mut report);
        library.load_text("irons_spellbooks", "scythe", GECKO_LATER, &mut report);
        assert_eq!(report.replaced, vec!["irons_spellbooks:scythe_backpedal"]);
        let clip = library.get("irons_spellbooks:scythe_backpedal").unwrap();
        assert_eq!(clip.end_tick, 40);
        assert_eq!(library.len(), 2);
    }

    #[test]
    fn bad_files_are_reported_not_fatal() {
        let mut library = AnimationLibrary::default();
        let mut report = LoadReport::default();
        library.load_text("x", "broken", "{not json", &mut report);
        assert_eq!(report.errors.len(), 1);
        assert!(library.is_empty());
    }

    #[test]
    fn loads_a_resource_root_in_sorted_order() {
        let root = std::env::temp_dir().join(format!("horizon-player-anim-{}", std::process::id()));
        let dir = root.join("assets/irons_spellbooks/player_animations");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("casting_animations.json"), GECKO).unwrap();
        fs::write(dir.join("scythe.json"), GECKO_LATER).unwrap();
        let bc = root.join("assets/bettercombat/player_animations");
        fs::create_dir_all(&bc).unwrap();
        fs::write(bc.join("one_handed_punch.json"), EMOTE.replace("waving", "one_handed_punch"))
            .unwrap();
        let (library, report) = AnimationLibrary::load_resources(&root);
        let _ = fs::remove_dir_all(&root);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.files, 3);
        assert_eq!(library.get("irons_spellbooks:scythe_backpedal").unwrap().end_tick, 40);
        assert!(library.contains("bettercombat:one_handed_punch"));
    }
}
