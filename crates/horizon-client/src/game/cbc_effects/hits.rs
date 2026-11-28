//! CBC 5.11.7 block-hit resources and ClientboundPlayBlockHitEffectPacket.
use super::{Game, LocalSound};
use glam::DVec3;
use horizon_client::{
    net::CBC_BLOCK_HIT_PACKET,
    particles::{CbcImpact, CbcParticle, CbcShard, ParticleOptions, ParticleType},
    settings::SOUND_SOURCES,
};
use horizon_core::random::{LegacyRandom, RandomSource};
use horizon_protocol::buf::Reader;
use horizon_world::{content_view, registry::BlockStateId};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

const SHOW_PROJECTILE_SPLASHES: bool = true;
const SHOW_PROJECTILE_IMPACTS: bool = true;
const BLOCKS: usize = 4;

#[derive(Debug)]
struct Hit {
    state: i32,
    entity: i32,
    deflect: bool,
    force: bool,
    at: [f64; 3],
    velocity: DVec3,
}

impl Hit {
    fn decode(data: &[u8]) -> Option<Self> {
        let mut r = Reader::new(data);
        if r.varint().ok()? != i32::from(CBC_BLOCK_HIT_PACKET) {
            return None;
        }
        let state = r.varint().ok()?;
        let entity = r.varint().ok()?;
        let deflect = r.bool().ok()?;
        let force = r.bool().ok()?;
        let at = [r.f64().ok()?, r.f64().ok()?, r.f64().ok()?];
        let velocity = DVec3::new(
            r.f32().ok()?.into(),
            r.f32().ok()?.into(),
            r.f32().ok()?.into(),
        );
        (r.remaining() == 0
            && state >= 0
            && entity >= 0
            && at.iter().all(|v| v.is_finite())
            && velocity.is_finite())
        .then_some(Self {
            state,
            entity,
            deflect,
            force,
            at,
            velocity,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default)]
struct Projectile {
    effect_multiplier: f32,
    maximum_effect_scale: f32,
    pitch_modifier: f32,
    volume: f32,
}

impl Default for Projectile {
    fn default() -> Self {
        Self {
            maximum_effect_scale: 10.0,
            ..Self::UNLISTED
        }
    }
}

impl Projectile {
    const UNLISTED: Self = Self {
        effect_multiplier: 1.0,
        maximum_effect_scale: 3.0,
        pitch_modifier: -0.5,
        volume: 3.5,
    };

    fn velocity(self, v: DVec3) -> DVec3 {
        let length = v.length();
        if length < 1e-5 {
            return DVec3::ZERO;
        }
        let magnitude =
            (length * f64::from(self.effect_multiplier)).min(f64::from(self.maximum_effect_scale));
        (v / length) * magnitude
    }

    fn pitch(self, base: f32, variation: f32) -> f32 {
        ((base + self.pitch_modifier).clamp(0.0, 2.0) + variation).clamp(0.0, 2.0)
    }

    fn count(self, base: i32) -> i32 {
        (base as f32 * self.effect_multiplier).ceil() as i32
    }
}

#[derive(Deserialize)]
struct HitSound {
    sound: String,
    source: Option<String>,
    pitch: f32,
    pitch_variation: f32,
}

impl HitSound {
    fn source(&self) -> Option<usize> {
        let name = self.source.as_deref().unwrap_or("block");
        SOUND_SOURCES[..10].iter().position(|s| *s == name)
    }
}

#[derive(Deserialize)]
struct BlockEffect {
    impact_particles: Vec<Value>,
    deflect_particles: Vec<Value>,
    impact_sound: HitSound,
    deflect_sound: HitSound,
}

#[derive(Default)]
struct BlockEffects {
    direct: HashMap<String, BlockEffect>,
    tags: Vec<(String, BlockEffect)>,
}

impl BlockEffects {
    fn resolve<'a, T>(
        &'a self,
        name: &str,
        in_tag: impl Fn(&str) -> bool,
        mut decode: impl FnMut(&'a BlockEffect) -> Option<T>,
    ) -> Option<T> {
        self.direct.get(name).and_then(&mut decode).or_else(|| {
            self.tags
                .iter()
                .rev()
                .filter(|(tag, _)| in_tag(tag))
                .find_map(|(_, effect)| decode(effect))
        })
    }
}

#[derive(Default)]
pub(super) struct Effects {
    root: Option<PathBuf>,
    atlas_generation: u64,
    blocks: BlockEffects,
    fluids: BlockEffects,
    projectiles: HashMap<String, Projectile>,
}

fn visit_json(dir: &Path, base: &Path, visit: &mut impl FnMut(&str, &[u8])) {
    let Ok(mut entries) = content_view::read_dir(dir) else {
        return;
    };
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if entry.is_dir() {
            visit_json(&path, base, visit);
        } else if path.extension().is_some_and(|ext| ext == "json")
            && let Ok(relative) = path.strip_prefix(base)
            && let Ok(bytes) = content_view::read(&path)
        {
            visit(
                &relative
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/"),
                &bytes,
            );
        }
    }
}

impl Effects {
    fn load(root: &Path, atlas_generation: u64) -> Self {
        let mut this = Self {
            root: Some(root.to_owned()),
            atlas_generation,
            ..Self::default()
        };
        let Ok(mut namespaces) = content_view::read_dir(root.join("assets")) else {
            return this;
        };
        namespaces.sort_by_key(|entry| entry.path());
        for ns in namespaces
            .into_iter()
            .filter(content_view::DirEntry::is_dir)
        {
            let path = ns.path();
            let Some(namespace) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            for (folder, holder) in [("block", &mut this.blocks), ("fluid", &mut this.fluids)] {
                let dir = path.join("block_hit_effects").join(folder);
                visit_json(&dir, &dir, &mut |key, bytes| {
                    let Ok(effect) = serde_json::from_slice::<BlockEffect>(bytes) else {
                        log::warn!("Invalid CBC block hit effect {namespace}:{key}");
                        return;
                    };
                    if effect.impact_sound.source().is_none()
                        || effect.deflect_sound.source().is_none()
                    {
                        log::warn!("Invalid CBC sound source in {namespace}:{key}");
                        return;
                    }
                    if let Some(tag) = key.strip_prefix("tags/") {
                        holder.tags.push((format!("{namespace}:{tag}"), effect));
                    } else {
                        holder.direct.insert(format!("{namespace}:{key}"), effect);
                    }
                });
            }
            let dir = path.join("projectile_hit_effects");
            visit_json(&dir, &dir, &mut |key, bytes| {
                if let Ok(mut value) = serde_json::from_slice::<Projectile>(bytes) {
                    value.effect_multiplier = value.effect_multiplier.max(0.0);
                    value.maximum_effect_scale = value.maximum_effect_scale.max(0.0);
                    value.volume = value.volume.max(0.0);
                    this.projectiles.insert(format!("{namespace}:{key}"), value);
                }
            });
        }
        this
    }
}

#[derive(Deserialize)]
struct ParticleBlock {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties", default)]
    properties: BTreeMap<String, String>,
}

fn field<T: serde::de::DeserializeOwned>(value: &Value, name: &str) -> Option<T> {
    T::deserialize(value.get(name)?).ok()
}

fn rgb(value: &Value) -> Option<[f32; 3]> {
    Some([field(value, "r")?, field(value, "g")?, field(value, "b")?])
}

fn color(value: &Value) -> Option<[f32; 3]> {
    if let Some(n) = value.as_u64() {
        return Some([16, 8, 0].map(|shift| ((n >> shift) & 255) as f32 / 255.0));
    }
    <[f32; 3]>::deserialize(value).ok()
}

fn namespaced(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

impl Game<'_> {
    pub(in crate::game) fn cbc_block_hit(&mut self, data: &[u8]) {
        let (Some(hit), Some(remote)) = (Hit::decode(data), self.remote.as_mut()) else {
            return;
        };
        let generation = self.gui.atlas.generation;
        if remote.cbc_effects.hit_effects.root.as_deref() != Some(self.options.resources.as_path())
            || remote.cbc_effects.hit_effects.atlas_generation != generation
        {
            remote.cbc_effects.hit_effects = Effects::load(&self.options.resources, generation);
        }
        let remote = self.remote.as_ref().unwrap();
        let Some(entity) = remote
            .builtin
            .get("minecraft:entity_type")
            .and_then(|ids| ids.name(hit.entity))
            .filter(|name| !name.is_empty())
        else {
            return;
        };
        let state = if remote.block_map.is_empty() {
            BlockStateId(hit.state as u32)
        } else {
            let Some(&state) = remote.block_map.get(hit.state as usize) else {
                return;
            };
            BlockStateId(state)
        };
        let Some(block) = self.pack.assets.registry.state(state) else {
            return;
        };
        let name = if remote.block_map.is_empty() {
            &block.name
        } else {
            crate::game::server_states::server_block_name(hit.state).unwrap_or(&block.name)
        };
        let facts = self.res.facts().get(state);
        // Waterlogged blocks have a fluid state but are not LiquidBlocks.
        let liquid = facts
            .classes
            .iter()
            .any(|class| class.rsplit('.').next() == Some("LiquidBlock"))
            || (block.properties.contains_key("level")
                && (fluid_splash(name).is_some()
                    || remote
                        .builtin
                        .get("minecraft:fluid")
                        .is_some_and(|ids| ids.id(name).is_some())));
        if (liquid && !SHOW_PROJECTILE_SPLASHES) || (!liquid && !SHOW_PROJECTILE_IMPACTS) {
            return;
        }
        let effects = &remote.cbc_effects.hit_effects;
        let projectile = effects
            .projectiles
            .get(entity)
            .copied()
            .unwrap_or(Projectile::UNLISTED);
        let velocity = projectile.velocity(hit.velocity);
        let rules = if liquid {
            &effects.fluids
        } else {
            &effects.blocks
        };
        let block_id = remote
            .builtin
            .get("minecraft:block")
            .and_then(|ids| ids.id(name));
        let in_tag = |tag: &str| {
            if let (Some(tags), Some(id)) = (&remote.registries.block_tags, block_id) {
                tags.get(tag).is_some_and(|entries| entries.contains(&id))
            } else {
                self.res
                    .items()
                    .block_tags
                    .get(tag)
                    .is_some_and(|entries| entries.contains(name))
            }
        };
        let effect = rules.resolve(name, in_tag, |effect| {
            let (active, inactive, sound) = if hit.deflect {
                (
                    &effect.deflect_particles,
                    &effect.impact_particles,
                    &effect.deflect_sound,
                )
            } else {
                (
                    &effect.impact_particles,
                    &effect.deflect_particles,
                    &effect.impact_sound,
                )
            };
            let decode = |json: &Value| self.cbc_hit_particle(json, state, hit.at, projectile);
            // Java's resource codec rejects the entire rule if either list is invalid.
            if !inactive.iter().all(|json| decode(json).is_some()) {
                return None;
            }
            let options = active.iter().map(decode).collect::<Option<Vec<_>>>()?;
            Some((options, sound))
        });
        if let Some((options, sound)) = effect {
            let source = sound.source().unwrap_or(BLOCKS);
            let event = namespaced(&sound.sound);
            let (pitch, variation) = (sound.pitch, sound.pitch_variation);
            for option in options {
                self.particles
                    .add_with(&option, hit.at, velocity.to_array(), hit.force, false);
            }
            let pitch = projectile.pitch(
                pitch,
                self.particles.cbc_level_random().next_float() * variation,
            );
            self.audio.play_cbc_hit(
                source,
                &event,
                hit.at,
                projectile.volume,
                pitch,
                self.particles.cbc_level_random(),
                true,
            );
        } else if liquid {
            if velocity.y < velocity.length() * 0.15 {
                return;
            }
            let Some((color, light, scale, base, event, source)) = fluid_splash(name) else {
                return;
            };
            let pitch =
                projectile.pitch(base, self.particles.cbc_level_random().next_float() * 0.3);
            self.audio.play_cbc_hit(
                source,
                event,
                hit.at,
                projectile.volume,
                pitch,
                self.particles.cbc_level_random(),
                false,
            );
            let v = DVec3::new(
                velocity.x * scale * 0.5,
                velocity.y * scale,
                velocity.z * scale * 0.5,
            );
            self.particles.add_with(
                &impact(CbcImpact::Splash { color, light }),
                hit.at,
                v.to_array(),
                true,
                false,
            );
        } else {
            let pos = hit.at.map(|v| v.floor() as i32);
            let mut debris = impact(CbcImpact::Matter {
                state,
                deflect: hit.deflect,
                force: hit.force,
            });
            debris.block = self.cbc_hit_sprite(state, pos);
            self.particles
                .add_with(&debris, hit.at, velocity.to_array(), hit.force, false);
            if hit.deflect {
                let sound = self.res.facts().get(state).sounds.clone();
                self.audio.play_cbc_hit(
                    BLOCKS,
                    &sound.break_sound,
                    hit.at,
                    2.0,
                    sound.pitch,
                    self.particles.cbc_level_random(),
                    false,
                );
            } else if projectile.effect_multiplier >= 1.0 {
                self.particles.add_with(
                    &impact(CbcImpact::SmokeBurst),
                    hit.at,
                    velocity.to_array(),
                    hit.force,
                    false,
                );
                let sound = projectile_impact_sound(self.particles.cbc_level_random());
                self.audio.play_cbc_hit(
                    sound.source,
                    sound.event,
                    hit.at,
                    sound.volume,
                    sound.pitch,
                    self.particles.cbc_level_random(),
                    false,
                );
            }
        }
    }

    fn cbc_hit_sprite(&self, state: BlockStateId, pos: [i32; 3]) -> Option<(String, u32)> {
        let block = self.pack.assets.registry.state(state)?;
        if self.res.facts().is_air(state) || block.name == "minecraft:moving_piston" {
            return None;
        }
        self.particle_sprite(state, pos)
    }

    fn cbc_hit_particle(
        &self,
        json: &Value,
        hit: BlockStateId,
        at: [f64; 3],
        projectile: Projectile,
    ) -> Option<ParticleOptions> {
        let kind = namespaced(json["type"].as_str()?);
        let registry = self.pack.assets.registry;
        let state = || {
            let block = field::<ParticleBlock>(json, "block_state")?;
            let name = namespaced(&block.name);
            let mut state = registry.state(registry.default_state(&name)?)?.clone();
            for (key, value) in block.properties {
                state.properties.insert(key, value);
            }
            registry.id(&state)
        };
        let pos = at.map(|v| v.floor() as i32);
        if let Some(name) = kind.strip_prefix("createbigcannons:") {
            let shard = match name {
                "splinter" | "splinter_burst" => Some(CbcShard::Splinter),
                "glass_shard" | "glass_burst" => Some(CbcShard::Glass),
                "leaf" | "leaf_burst" => Some(CbcShard::Leaf),
                _ => None,
            };
            if let Some(kind) = shard {
                let mut state = state()?;
                let particle = if name.ends_with("_burst") {
                    let mut count = field(json, "count")?;
                    if self.res.facts().is_air(state) {
                        state = hit;
                        count = projectile.count(count);
                    }
                    CbcImpact::ShardBurst { state, kind, count }
                } else {
                    CbcImpact::Shard(kind)
                };
                let mut option = impact(particle);
                option.block = self.cbc_hit_sprite(state, pos);
                return Some(option);
            }
            let particle = match name {
                "block" => CbcImpact::Block,
                "debris_matter" => CbcImpact::Matter {
                    state: state()?,
                    deflect: field(json, "deflect")?,
                    force: field(json, "force_display")?,
                },
                "debris_smoke_burst" => CbcImpact::SmokeBurst,
                "debris_smoke" => {
                    return Some(ParticleOptions::cbc(CbcParticle::DebrisSmoke {
                        scale: field(json, "scale")?,
                    }));
                }
                "spark_burst" => CbcImpact::SparkBurst {
                    color: field(json, "color")?,
                    deflect: field(json, "deflect")?,
                    count: projectile.count(field(json, "count")?),
                },
                "spark" => CbcImpact::Spark {
                    color: field(json, "scale")?,
                },
                "projectile_splash" => CbcImpact::Splash {
                    color: rgb(json)?,
                    light: field(json, "light")?,
                },
                "splash_spray" => CbcImpact::Spray {
                    color: rgb(json)?,
                    light: field(json, "light")?,
                    size: field(json, "size")?,
                    lifetime: field(json, "lifetime")?,
                },
                _ => {
                    return self
                        .particles
                        .has_modded(&kind)
                        .then(|| ParticleOptions::modded(&kind));
                }
            };
            let mut option = impact(particle);
            if matches!(particle, CbcImpact::Block | CbcImpact::Matter { .. }) {
                option.block = self.cbc_hit_sprite(state()?, pos);
            }
            return Some(option);
        }
        use ParticleType as P;
        let Some(particle) = ParticleType::from_name(&kind) else {
            return self
                .particles
                .has_modded(&kind)
                .then(|| ParticleOptions::modded(&kind));
        };
        Some(match particle {
            P::Block | P::BlockCrumble | P::FallingDust | P::DustPillar | P::BlockMarker => {
                let mut option = ParticleOptions::simple(particle);
                option.block = self.cbc_hit_sprite(state()?, pos);
                option
            }
            P::Dust => ParticleOptions::dust(color(&json["color"])?, field(json, "scale")?),
            P::DustColorTransition => ParticleOptions::dust_transition(
                color(&json["from_color"])?,
                color(&json["to_color"])?,
                field(json, "scale")?,
            ),
            P::Item => {
                let item = json["item"]
                    .as_str()
                    .or_else(|| json["item"]["id"].as_str())?;
                self.item_particle(&horizon_client::items::ItemStack::new(&namespaced(item), 1))
            }
            P::EntityEffect => {
                let argb: i32 = field(json, "color")?;
                let rgba = [16, 8, 0, 24].map(|shift| ((argb >> shift) & 255) as f32 / 255.0);
                ParticleOptions::colored(particle, rgba)
            }
            P::SculkCharge => ParticleOptions::sculk_charge(field(json, "roll")?),
            P::Shriek => ParticleOptions::shriek(field(json, "delay")?),
            _ => ParticleOptions::simple(particle),
        })
    }
}

fn impact(particle: CbcImpact) -> ParticleOptions {
    ParticleOptions::cbc(CbcParticle::Impact(particle))
}

type FluidSplash = ([f32; 3], f32, f64, f32, &'static str, usize);

fn projectile_impact_sound(random: &mut LegacyRandom) -> LocalSound {
    LocalSound {
        // The registered CBC event references mortar_stone_explode in sounds.json.
        event: "createbigcannons:projectile_impact",
        source: BLOCKS,
        volume: 3.0,
        pitch: 1.0 + random.next_float() * 0.4,
    }
}

fn fluid_splash(name: &str) -> Option<FluidSplash> {
    const COLD: &str = "createbigcannons:projectile_splash";
    const HOT: &str = "createbigcannons:hot_projectile_splash";
    Some(match name {
        "minecraft:water" => ([0.92, 0.97, 1.0], 0.0, 0.5, 0.31, COLD, BLOCKS),
        "minecraft:lava" => ([1.0, 0.35, 0.05], 1.0, 0.35, 0.5, HOT, BLOCKS),
        "create:honey" => ([0.95, 0.67, 0.07], 0.0, 0.2, 0.0, COLD, BLOCKS),
        "create:chocolate" => ([0.56, 0.25, 0.21], 0.0, 0.4, 0.31, COLD, BLOCKS),
        "createbigcannons:molten_cast_iron" => ([0.84, 0.43, 0.24], 0.25, 0.2, 0.5, HOT, BLOCKS),
        "createbigcannons:molten_bronze" => ([0.78, 0.51, 0.31], 0.25, 0.2, 0.5, HOT, BLOCKS),
        "createbigcannons:molten_steel" => ([0.42, 0.43, 0.40], 0.25, 0.2, 0.5, HOT, BLOCKS),
        "createbigcannons:molten_nethersteel" => ([0.51, 0.37, 0.36], 0.25, 0.2, 0.5, HOT, BLOCKS),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::sound_events::SoundRegistry;
    use horizon_protocol::buf::Writer;

    #[test]
    fn cbc_fallback_projectile_impact_plays_the_block_sound_alias() {
        let sound = projectile_impact_sound(&mut LegacyRandom::new(7));
        let pitch = 1.0 + LegacyRandom::new(7).next_float() * 0.4;
        // BackupBlockHitEffects.java:27-29, CBCSoundEvents.java:86-88.
        assert_eq!(
            (
                sound.event,
                SOUND_SOURCES[sound.source],
                sound.volume,
                sound.pitch
            ),
            ("createbigcannons:projectile_impact", "block", 3.0, pitch),
        );

        let mut registry = SoundRegistry::default();
        registry.add_file(
            "createbigcannons",
            br#"{
                "projectile_impact": {"sounds": [{"type": "event", "name": "createbigcannons:mortar_stone_explode"}]},
                "mortar_stone_explode": {"sounds": ["createbigcannons:mortar_stone_explode"]}
            }"#,
            &|_, _| true,
        );
        let resolved = registry
            .sound(sound.event, &mut LegacyRandom::new(7))
            .unwrap();
        // CBCSoundEvents.java:70-73,86-88.
        assert_eq!(resolved.location, "createbigcannons:mortar_stone_explode");
    }

    #[test]
    fn cbc_fallback_hot_splashes_use_the_wrapped_event() {
        for fluid in [
            "minecraft:lava",
            "createbigcannons:molten_cast_iron",
            "createbigcannons:molten_bronze",
            "createbigcannons:molten_steel",
            "createbigcannons:molten_nethersteel",
        ] {
            let (_, _, _, base_pitch, event, source) = fluid_splash(fluid).unwrap();
            // BackupBlockHitEffects.java:58-59,82-107, CBCSoundEvents.java:57-59.
            // Create 6.0.10 AllSoundEvents.java:425,576-583,602-604,633-638.
            assert_eq!(
                (event, SOUND_SOURCES[source], base_pitch),
                ("createbigcannons:hot_projectile_splash", "block", 0.5)
            );
        }
    }

    #[test]
    fn cbc_hit_packet_reads_state_entity_flags_and_mixed_precision_vectors() {
        let mut w = Writer::new();
        for value in [i32::from(CBC_BLOCK_HIT_PACKET), 3000, 350] {
            w.varint(value);
        }
        w.bool(true);
        w.bool(false);
        let at = [-15.125, 100.875, 1024.0625];
        for v in at {
            w.f64(v);
        }
        let velocity = [-0.5, 2.25, 1.0];
        for v in velocity {
            w.f32(v);
        }
        let hit = Hit::decode(&w.data).unwrap();
        assert_eq!(
            (hit.state, hit.entity, hit.deflect, hit.force),
            (3000, 350, true, false)
        );
        assert_eq!(hit.at, at);
        assert_eq!(hit.velocity.to_array(), velocity.map(f64::from));
        assert!(Hit::decode(&w.data[..w.data.len() - 1]).is_none());
        w.data.push(0);
        assert!(Hit::decode(&w.data).is_none());
    }

    #[test]
    fn cbc_resources_use_namespaces_and_prefer_direct_effects_to_tag_effects() {
        let root =
            std::env::temp_dir().join(format!("horizon-cbc-hit-rules-{}", std::process::id()));
        let write = |path: &str, json: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, json).unwrap();
        };
        let effect = |id: &str| {
            serde_json::json!({
                "impact_particles": [{"type": "minecraft:poof"}],
                "deflect_particles": [],
                "impact_sound": {"sound": id, "pitch": 1, "pitch_variation": 0.25},
                "deflect_sound": {"sound": id, "pitch": 1, "pitch_variation": 0.25}
            })
            .to_string()
        };
        write(
            "assets/minecraft/block_hit_effects/block/tags/leaves.json",
            &effect("test:tag"),
        );
        write(
            "assets/other/block_hit_effects/block/wood/oak.json",
            &effect("test:block"),
        );
        write(
            "assets/other/block_hit_effects/fluid/water.json",
            &effect("test:fluid"),
        );
        write(
            "assets/other/projectile_hit_effects/shell.json",
            r#"{"effect_multiplier":-1}"#,
        );
        let effects = Effects::load(&root, 0);
        assert_eq!(
            effects
                .blocks
                .resolve("other:wood/oak", |_| true, Some)
                .unwrap()
                .impact_sound
                .sound,
            "test:block"
        );
        let selected = effects
            .blocks
            .resolve(
                "other:wood/oak",
                |_| true,
                |rule| (rule.impact_sound.sound != "test:block").then_some(rule),
            )
            .unwrap();
        assert_eq!(selected.impact_sound.sound, "test:tag");
        let leaf = effects
            .blocks
            .resolve(
                "minecraft:oak_leaves",
                |name| name == "minecraft:leaves",
                Some,
            )
            .unwrap();
        assert_eq!(leaf.impact_sound.sound, "test:tag");
        assert_eq!(leaf.impact_particles[0]["type"], "minecraft:poof");
        assert!(
            effects
                .blocks
                .resolve("other:water", |_| false, Some)
                .is_none()
        );
        assert!(
            effects
                .fluids
                .resolve("other:water", |_| false, Some)
                .is_some()
        );
        assert_eq!(effects.projectiles["other:shell"].effect_multiplier, 0.0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cbc_projectile_settings_compose_with_the_packet_velocity_and_hit_pitch() {
        let p: Projectile = serde_json::from_str(
            r#"{"effect_multiplier":0.2,"maximum_effect_scale":2,"pitch_modifier":1,"volume":1.5}"#,
        )
        .unwrap();
        let upwards = DVec3::Y * 5e-5;
        assert!(
            p.velocity(upwards)
                .abs_diff_eq(upwards * f64::from(p.effect_multiplier), 1e-15)
        );
        let fast = DVec3::new(30.0, 40.0, 0.0);
        let slow = fast / 10.0;
        assert!(
            p.velocity(fast)
                .abs_diff_eq(fast.normalize() * f64::from(p.maximum_effect_scale), 1e-12)
        );
        assert!(p.velocity(slow).abs_diff_eq(
            slow.normalize() * (slow.length() * f64::from(p.effect_multiplier)),
            1e-12
        ));
        assert_eq!(p.pitch(0.9, 0.3), 2.0);
        assert_eq!(Projectile::UNLISTED.pitch(0.31, 0.2), 0.2);
        assert!(
            Projectile::default().maximum_effect_scale > Projectile::UNLISTED.maximum_effect_scale
        );
    }
}
