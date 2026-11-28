//! Particle definitions of content mods (`assets/<ns>/particles/<name>.json`):
//! the sprite list of each modded particle type, and the vanilla type a
//! modded one falls back to when it has no usable sprites.
use horizon_client_core::particle_type::ParticleType;
use horizon_world::content_view;
use std::path::{Path, PathBuf};

/// A modded particle: its registry name (`ns:name`) and atlas sprite names.
pub struct Definition {
    pub name: String,
    pub sprites: Vec<String>,
}

/// The atlas name of a definition's texture id: `ns:particle/<path>`, the
/// namespace defaulting to the particle's own.
fn atlas_name(texture: &str, namespace: &str) -> String {
    let (ns, path) = texture.split_once(':').unwrap_or((namespace, texture));
    format!("{ns}:particle/{path}")
}

/// The atlas names of the `textures` list of a particle definition.
pub fn sprite_names(json: &str, namespace: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    value["textures"]
        .as_array()
        .map(|textures| {
            textures
                .iter()
                .filter_map(|t| t.as_str())
                .map(|t| atlas_name(t, namespace))
                .collect()
        })
        .unwrap_or_default()
}

/// Every definition of every namespace but `minecraft` under `root/assets`.
pub fn scan(root: &Path) -> Vec<Definition> {
    let mut entries = Vec::new();
    let Ok(namespaces) = content_view::read_dir(root.join("assets")) else {
        return Vec::new();
    };
    for ns in namespaces {
        let namespace = ns.file_name().to_string_lossy().into_owned();
        if namespace == "minecraft" {
            continue;
        }
        let Ok(files) = content_view::read_dir(ns.path().join("particles")) else {
            continue;
        };
        for file in files {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            entries.push((namespace.clone(), stem, path));
        }
    }
    let parsed = crate::par::par_map(&entries, |(namespace, _, path)| {
        content_view::read_to_string(path)
            .ok()
            .map(|json| sprite_names(&json, namespace))
            .filter(|sprites| !sprites.is_empty())
    });
    entries
        .into_iter()
        .zip(parsed)
        .filter_map(|((namespace, stem, _), sprites)| {
            sprites.map(|sprites| Definition {
                name: format!("{namespace}:{stem}"),
                sprites,
            })
        })
        .collect()
}

/// The texture files the definitions under `root` list (the ones that exist),
/// for the atlas to pack as `ns:particle/...` sprites.
pub fn texture_files(root: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = scan(root)
        .iter()
        .flat_map(|d| d.sprites.iter())
        .filter_map(|sprite| {
            let (ns, path) = sprite.split_once(':')?;
            Some(root.join(format!("assets/{ns}/textures/{path}.png")))
        })
        .collect();
    candidates.sort();
    candidates.dedup();
    let exists = crate::par::par_map(&candidates, |p| content_view::is_file(p));
    candidates
        .into_iter()
        .zip(exists)
        .filter_map(|(p, ok)| ok.then_some(p))
        .collect()
}

/// The vanilla type that stands in for a modded particle without sprites.
pub fn fallback_type(name: &str) -> ParticleType {
    let path = name.split_once(':').map_or(name, |(_, path)| path);
    let has = |words: &[&str]| words.iter().any(|w| path.contains(w));
    if has(&["smoke", "soot", "ash"]) {
        ParticleType::Smoke
    } else if has(&["flame", "fire", "ember"]) {
        ParticleType::Flame
    } else {
        ParticleType::Poof
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textures_become_namespaced_particle_sprites() {
        let json = r#"{"textures":["cataclysm:amethyst_crash_0","bare_1"]}"#;
        assert_eq!(
            sprite_names(json, "mymod"),
            [
                "cataclysm:particle/amethyst_crash_0",
                "mymod:particle/bare_1"
            ]
        );
    }

    #[test]
    fn malformed_or_empty_definitions_have_no_sprites() {
        assert!(sprite_names("not json", "m").is_empty());
        assert!(sprite_names(r#"{"other":1}"#, "m").is_empty());
        assert!(sprite_names(r#"{"textures":[]}"#, "m").is_empty());
    }

    #[test]
    fn fallback_follows_the_name_and_defaults_to_poof() {
        assert_eq!(fallback_type("mod:black_smoke"), ParticleType::Smoke);
        assert_eq!(fallback_type("mod:fire_burst"), ParticleType::Flame);
        assert_eq!(fallback_type("mod:sparkle"), ParticleType::Poof);
    }

    #[test]
    fn scan_and_texture_files_dedup_and_keep_sorted_existing_paths() {
        let root = std::env::temp_dir().join(format!(
            "horizon-particle-defs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let write = |rel: &str, body: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        write(
            "assets/minecraft/particles/ignored.json",
            r#"{"textures":["vanilla_0"]}"#,
        );
        write(
            "assets/moda/particles/spark.json",
            r#"{"textures":["spark_0","shared_1","missing_0"]}"#,
        );
        write(
            "assets/modb/particles/glow.json",
            r#"{"textures":["moda:shared_1","glow_0"]}"#,
        );
        write("assets/modb/particles/broken.json", "{");
        write("assets/moda/textures/particle/spark_0.png", "x");
        write("assets/moda/textures/particle/shared_1.png", "x");
        write("assets/modb/textures/particle/glow_0.png", "x");
        write("assets/minecraft/textures/particle/vanilla_0.png", "x");

        let mut defs: Vec<_> = scan(&root)
            .into_iter()
            .map(|d| (d.name, d.sprites))
            .collect();
        defs.sort();
        assert_eq!(
            defs,
            vec![
                (
                    "moda:spark".to_owned(),
                    vec![
                        "moda:particle/spark_0".to_owned(),
                        "moda:particle/shared_1".to_owned(),
                        "moda:particle/missing_0".to_owned(),
                    ],
                ),
                (
                    "modb:glow".to_owned(),
                    vec![
                        "moda:particle/shared_1".to_owned(),
                        "modb:particle/glow_0".to_owned(),
                    ],
                ),
            ]
        );
        let mut expected = vec![
            root.join("assets/moda/textures/particle/shared_1.png"),
            root.join("assets/moda/textures/particle/spark_0.png"),
            root.join("assets/modb/textures/particle/glow_0.png"),
        ];
        expected.sort();
        assert_eq!(texture_files(&root), expected);
        let _ = std::fs::remove_dir_all(&root);
    }
}
