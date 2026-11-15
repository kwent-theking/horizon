//! Entity occlusion affects mesh submission, never entity ticking or interaction.
use super::{Game, Vertex};
use horizon_client::chunk::MIN_Y;
use horizon_client::{
    blocks::{RenderShape, StateFacts},
    entity_culling::Culler,
};

fn opaque(facts: &StateFacts) -> bool {
    facts.render_shape == RenderShape::Model
        && facts.can_occlude
        && facts.solid_render
        && facts.full_collision
}

impl Game<'_> {
    pub(super) fn prepare_entity_culling(&mut self, camera: glam::DVec3) {
        // Spans tell the pack's passes which entities are hidden; without them, nothing changes.
        self.pack_shadow_casters = match (&self.renderer, &self.pack_ids) {
            (Some(r), Some(_)) => r.pack_shadow_casters(),
            _ => (false, false),
        };
        let at = camera.to_array().map(|v| v.floor() as i32);
        self.entity_culler = (self.settings.horizon.entity_culling
            // Packs can replace an opaque block's model or texture.
            && self.settings.resource_packs.iter().all(|id| id == "vanilla")
            && self.player.is_some()
            && !self.spectator()
            && !opaque(self.res.facts().get(self.world.block(at[0], at[1], at[2]))))
        .then(|| Culler::new(camera.to_array(), 8_192));
    }

    /// Use complete generated geometry so equipment, animation and custom
    /// models cannot stick out of an assumed collision-sized culling box.
    pub(super) fn cull_entity_vertices(&self, vertices: &mut Vec<Vertex>, start: usize) {
        let Some(culler) = &self.entity_culler else {
            return;
        };
        let hidden = culler.hidden(vertices, start, |pos| self.occludes(pos));
        // A shader pack's shadow pass still draws what the camera cannot see.
        if hidden && !(self.pack_shadow_casters.0 && self.hide_spans_from(start, vertices.len())) {
            vertices.truncate(start);
        }
    }
}

impl Game<'_> {
    /// A fully built, uploaded opaque block that can hide an entity behind it.
    fn occludes(&self, pos: [i32; 3]) -> bool {
        let state = self.world.block(pos[0], pos[1], pos[2]);
        if !opaque(self.res.facts().get(state)) {
            return false;
        }
        let Some(y) = pos[1].checked_sub(MIN_Y) else {
            return false;
        };
        let section = (pos[0] >> 4, y.div_euclid(16), pos[2] >> 4);
        // Newly edited or not-yet-uploaded terrain is not an occluder.
        self.renderer
            .as_ref()
            .is_some_and(|r| r.has_section(section))
            && !self.world.dirty_sections.contains(&section)
            && !self.streaming.is_meshing(section)
            && !self.pending_uploads.iter().any(|(key, _)| *key == section)
    }

    /// Whether a mob standing at `feet` (interpolated, `width` by `height`)
    /// is hidden behind opaque blocks before its pose and mesh are built.
    /// A shader pack's shadow pass still needs what the camera cannot see.
    pub(super) fn mob_hidden_before_pose(&self, feet: glam::DVec3, width: f64, height: f64) -> bool {
        if self.pack_shadow_casters.0 {
            return false;
        }
        let Some(culler) = &self.entity_culler else {
            return false;
        };
        let (min, max) = pose_bounds(feet, width, height);
        culler.occluded(min, max, |pos| self.occludes(pos))
    }
}

/// Margin around a mob's collision box that its posed model stays inside:
/// equipment, wings, tentacles and a dragon's limbs reach past the box.
fn pose_margin(width: f64, height: f64) -> f64 {
    (width.max(height) * 0.5).max(1.0)
}

/// Factor on a modded mob's box for the box its model may fill: GeckoLib models
/// can be far bigger than the template hitbox they run on.
const MODDED_MODEL_EXTENT: f64 = 4.0;

/// The width and height of the box a mob's drawn model can fill, from its
/// collision box and its `scale` attribute (`model_scale`, 0 when unset); and
/// whether the model's size is unknown (a modded type), where the occlusion
/// test before the pose must not run.
pub(super) fn drawn_extent(kind: &str, width: f64, height: f64, model_scale: f32) -> (f64, f64, bool) {
    let scale = if model_scale > 0.0 { f64::from(model_scale) } else { 1.0 };
    let modded = !kind.starts_with("minecraft:");
    let grow = if modded { MODDED_MODEL_EXTENT } else { 1.0 };
    let factor = scale.max(1.0) * grow;
    (width * factor, height * factor, modded)
}

/// The whole box (not the position) a mob's model can occupy, in world space.
pub(super) fn pose_bounds(feet: glam::DVec3, width: f64, height: f64) -> ([f64; 3], [f64; 3]) {
    let margin = pose_margin(width, height);
    let half = width / 2.0 + margin;
    (
        [feet.x - half, feet.y - margin, feet.z - half],
        [feet.x + half, feet.y + height + margin, feet.z + half],
    )
}

impl Game<'_> {
    /// Entity.setViewScale (LevelExtractor.extractVisibleEntities): the entity
    /// distance option times the render distance's boost — 1 at 8 chunks, 2.5
    /// at 20 or more. The render distance is the client setting, capped by
    /// the server's view distance when connected
    /// (Options.getEffectiveRenderDistance).
    pub(super) fn view_scale(&self) -> f64 {
        let distance = match &self.remote {
            Some(r) => self.settings.render_distance.min(r.view_distance),
            None => self.settings.render_distance,
        };
        entity_view_scale(distance, self.settings.entity_distance)
    }
}

/// Entity.setViewScale: `Mth.clamp(renderDistance / 8, 1, 2.5) *
/// entityDistanceScaling`. Every entity's draw distance is scaled by it, so
/// a small mob is drawn nearer than a big one and a longer render distance
/// draws both further away.
pub(super) fn entity_view_scale(render_distance: i32, entity_distance: f32) -> f64 {
    (f64::from(render_distance) / 8.0).clamp(1.0, 2.5) * f64::from(entity_distance)
}

/// Entity.shouldRenderAtSqrDistance's default range: 64 times the bounding
/// box's mean edge, scaled by the view scale.
pub(super) fn default_range(size: glam::DVec3, view_scale: f64) -> f64 {
    let edge = (size.x + size.y + size.z) / 3.0;
    let edge = if edge.is_nan() { 1.0 } else { edge };
    edge * 64.0 * view_scale
}

impl Game<'_> {
    /// EntityRenderer.shouldRender: the entity within `range` blocks of the
    /// camera, measured from its current `position`, and its culling box
    /// (`lo`..`hi`, interpolated, in world space) grown by 0.5 in the view.
    pub(super) fn entity_in_view(
        &self,
        camera: glam::DVec3,
        position: glam::DVec3,
        range: f64,
        lo: glam::DVec3,
        hi: glam::DVec3,
    ) -> bool {
        if (position - camera).length_squared() >= range * range {
            return false;
        }
        self.view_frustum.as_ref().is_none_or(|view| {
            view.intersects(
                (lo - camera - glam::DVec3::splat(0.5)).as_vec3(),
                (hi - camera + glam::DVec3::splat(0.5)).as_vec3(),
            )
        })
    }
}

/// An entity's position as a `DVec3`.
pub(super) fn at(p: horizon_core::geometry::Vec3) -> glam::DVec3 {
    glam::DVec3::new(p.x, p.y, p.z)
}

impl Game<'_> {
    /// `entity_in_view` for an entity whose box stands on its position,
    /// `width` wide and `height` tall, interpolated by `alpha` and grown by
    /// `grow` (a renderer's getBoundingBoxForCulling).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn standing_entity_in_view(
        &self,
        camera: glam::DVec3,
        previous: glam::DVec3,
        position: glam::DVec3,
        alpha: f64,
        width: f64,
        height: f64,
        range: f64,
        grow: f64,
    ) -> bool {
        let p = previous.lerp(position, alpha);
        let half = glam::DVec3::new(width / 2.0 + grow, grow, width / 2.0 + grow);
        let lo = p - half;
        let hi = p + glam::DVec3::new(half.x, height + grow, half.z);
        self.entity_in_view(camera, position, range, lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-pose box covers the collision box with room for a model that
    /// reaches past it, and a big mob's margin grows with its size.
    #[test]
    fn the_pose_box_covers_the_whole_mob() {
        let (min, max) = pose_bounds(glam::DVec3::new(10.0, 64.0, -3.0), 0.6, 1.8);
        assert_eq!(min, [10.0 - 1.3, 63.0, -3.0 - 1.3]);
        assert_eq!(max, [10.0 + 1.3, 64.0 + 1.8 + 1.0, -3.0 + 1.3]);
        let (dragon_min, dragon_max) = pose_bounds(glam::DVec3::ZERO, 16.0, 8.0);
        assert_eq!(dragon_max[0] - dragon_min[0], 16.0 + 16.0);
    }

    /// A modded model may be far bigger than its hitbox, so it is never
    /// occlusion-culled before the pose; a scaled mob's box grows with it.
    #[test]
    fn the_drawn_extent_grows_for_scaled_and_modded_mobs() {
        assert_eq!(drawn_extent("minecraft:zombie", 0.6, 1.95, 0.0), (0.6, 1.95, false));
        assert_eq!(drawn_extent("minecraft:zombie", 0.6, 2.0, 0.5), (0.6, 2.0, false));
        assert_eq!(drawn_extent("minecraft:player", 0.6, 1.8, 5.0), (3.0, 9.0, false));
        let (w, h, unknown) = drawn_extent("born_in_chaos_v1:boss", 0.9, 0.9, 0.0);
        assert!(unknown && w >= 3.0 && h >= 3.0);
    }

    /// A solid wall across the whole box hides it; a box that sticks out
    /// past the wall's edge, or a missing wall, leaves it drawn.
    #[test]
    fn a_mob_is_hidden_only_when_a_wall_covers_its_whole_box() {
        let culler = Culler::new([0.5, 65.0, 0.5], 8_192);
        let wall = |pos: [i32; 3]| pos[0] == 3 && (55..=75).contains(&pos[1]) && (-10..=10).contains(&pos[2]);
        let (min, max) = pose_bounds(glam::DVec3::new(10.0, 64.0, 0.5), 0.6, 1.8);
        assert!(culler.occluded(min, max, wall));
        let (min, max) = pose_bounds(glam::DVec3::new(10.0, 64.0, 80.0), 0.6, 1.8);
        assert!(!culler.occluded(min, max, wall));
        assert!(!culler.occluded(min, max, |_| false));
    }

    /// `Entity.setViewScale`: the entity distance option times the render
    /// distance's boost — 1 at eight chunks, 1.5 at twelve, 2.5 at twenty.
    #[test]
    fn the_view_scale_follows_the_render_distance() {
        assert_eq!(entity_view_scale(4, 1.0), 1.0);
        assert_eq!(entity_view_scale(8, 1.0), 1.0);
        assert_eq!(entity_view_scale(12, 1.0), 1.5);
        assert_eq!(entity_view_scale(20, 1.0), 2.5);
        assert_eq!(entity_view_scale(32, 1.0), 2.5);
        // The entity distance option scales it down.
        assert_eq!(entity_view_scale(12, 0.5), 0.75);
    }

    /// `Entity.shouldRenderAtSqrDistance`: a chicken's box (0.4 by 0.7 by
    /// 0.4) is drawn up to 48 blocks away at the vanilla defaults, not the
    /// 32 blocks a missing view scale leaves (#2273).
    #[test]
    fn a_chicken_is_drawn_forty_eight_blocks_away() {
        let range = default_range(glam::DVec3::new(0.4, 0.7, 0.4), entity_view_scale(12, 1.0));
        assert_eq!(range, 48.0);
    }

    /// `Entity.shouldRenderAtSqrDistance` on a broken box: the size falls
    /// back to one block.
    #[test]
    fn a_broken_bounding_box_falls_back_to_one_block() {
        let range = default_range(
            glam::DVec3::new(f64::NAN, 0.7, 0.4),
            entity_view_scale(12, 1.0),
        );
        assert_eq!(range, 96.0);
    }
}
