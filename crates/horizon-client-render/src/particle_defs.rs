//! Particle definitions of content mods (`assets/<ns>/particles/<name>.json`):
//! the sprite list of each modded particle type, and the vanilla type a
//! modded one falls back to when it has no usable sprites.
use horizon_client_core::particle_type::ParticleType;
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
    let mut out = Vec::new();
    let Ok(namespaces) = std::fs::read_dir(root.join("assets")) else {
        return out;
    };
    for ns in namespaces.flatten() {
        let namespace = ns.file_name().to_string_lossy().into_owned();
        if namespace == "minecraft" {
            continue;
        }
        let Ok(files) = std::fs::read_dir(ns.path().join("particles")) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let (Some(stem), Ok(json)) = (path.file_stem(), std::fs::read_to_string(&path)) else {
                continue;
            };
            let sprites = sprite_names(&json, &namespace);
            if !sprites.is_empty() {
                out.push(Definition {
                    name: format!("{namespace}:{}", stem.to_string_lossy()),
                    sprites,
                });
            }
        }
    }
    out
}

/// The texture files the definitions under `root` list (the ones that exist),
/// for the atlas to pack as `ns:particle/...` sprites.
pub fn texture_files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = scan(root)
        .iter()
        .flat_map(|d| d.sprites.iter())
        .filter_map(|sprite| {
            let (ns, path) = sprite.split_once(':')?;
            Some(root.join(format!("assets/{ns}/textures/{path}.png")))
        })
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files.dedup();
    files
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
            ["cataclysm:particle/amethyst_crash_0", "mymod:particle/bare_1"]
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
}
