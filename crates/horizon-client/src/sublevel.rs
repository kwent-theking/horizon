//! Sub-level management for Sable (moving chunk groups).
//! Управление под-уровнями Sable (движущиеся группы чанков).
//!
//! Wire/protocol facts come from `agent2_sable_protocol_report.md`:
//! - Plot parking lot: `SubLevelContainer` origin = (10000, 10000) in PLOT units,
//!   LOG_PLOT_SIZE = 7 -> 128 chunks per plot axis.
//! - Global chunk coord = local + ((10000 + px) << 7, (10000 + pz) << 7)  (`LevelPlot.toGlobal`).
//! - In-bounds check: plot = ((x >> 7) - 10000, (z >> 7) - 10000), both in [0, 128).
//! - Pose3d on the wire: position (3x f64) + orientation quat (4x f32, x,y,z,w)
//!   + rotationPoint (3x f64). `scale` is NOT serialized (client-side constant 1,1,1).
//! - Snapshot entries are keyed by plotCoordinate, so the manager keeps a plot -> id index.
//!
//! EXPECTED CONTRACT of `crate::sable` (written in parallel by Agent 3) — if the field
//! names differ, fix `apply_snapshot` only:
//! ```ignore
//! pub struct SableSnapshot { pub interpolation_tick: i32, pub entries: Vec<SableSnapshotEntry> }
//! pub struct SableSnapshotEntry {
//!     pub plot_x: i32, pub plot_z: i32,
//!     pub position: [f64; 3], pub rotation: [f32; 4], pub rotation_point: [f64; 3],
//!     pub linear_velocity: [f32; 3], pub angular_velocity: [f32; 3],
//! }
//! ```

use std::collections::HashMap;

/// Plot parking-lot constants (SubLevelContainer).
/// Константы «парковки плот» (SubLevelContainer).
pub const LOG_PLOT_SIZE: i32 = 7;
pub const PLOT_SIZE: i32 = 1 << LOG_PLOT_SIZE; // 128 chunks per plot axis
pub const PLOT_ORIGIN: i32 = 10_000; // in plot units

/// Number of block states in one 16x16x16 section.
/// Число блоков в одной секции 16x16x16.
pub const SECTION_VOLUME: usize = 16 * 16 * 16;

/// One decoded chunk section belonging to a sub-level (local section coords).
/// Одна декодированная секция чанка под-уровня (локальные координаты секции).
#[derive(Clone, Debug)]
pub struct SectionData {
    /// 4096 palette-resolved block state ids, indexed `y*256 + z*16 + x` (local).
    /// 4096 state-id блоков, индекс `y*256 + z*16 + x` (локально).
    pub blocks: Vec<u32>,
    /// Optional combined light nibbles (block/sky) if the light packet was decoded.
    /// Опциональные данные света (block/sky), если light-пакет был декодирован.
    pub light: Option<Vec<u8>>,
}

impl SectionData {
    /// New section filled with `air_state` (usually 0).
    /// Новая секция, заполненная `air_state` (обычно 0).
    pub fn new(air_state: u32) -> Self {
        Self {
            blocks: vec![air_state; SECTION_VOLUME],
            light: None,
        }
    }
}

/// Sub-level pose: a mirror of Sable's serializable `Pose3d` (without `scale`).
/// Поза под-уровня: зеркало сериализуемого `Pose3d` из Sable (без `scale`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubLevelPose {
    /// World position, x/y/z (doubles on the wire).
    pub position: [f64; 3],
    /// Orientation quaternion, (x, y, z, w) — wire order per SableBufferUtils.
    /// Кватернион ориентации, (x, y, z, w) — порядок из SableBufferUtils.
    pub rotation: [f32; 4],
    /// Point the rotation is applied around.
    /// Точка, вокруг которой применяется вращение.
    pub rotation_point: [f64; 3],
}

impl Default for SubLevelPose {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0], // identity quaternion
            rotation_point: [0.0; 3],
        }
    }
}

/// A single tracked sub-level.
/// Один отслеживаемый под-уровень.
#[derive(Clone, Debug)]
pub struct SubLevel {
    /// Sub-level UUID packed as u128 (most-significant long in high 64 bits).
    /// UUID под-уровня как u128 (старший long в старших 64 битах).
    pub id: u128,
    /// Plot coordinates (px, pz) in the parking-lot grid.
    /// Координаты плота (px, pz) в сетке «парковки».
    pub plot: (i32, i32),
    /// Latest known pose (snapshot at `game_tick`).
    /// Последняя известная поза (снапшот на `game_tick`).
    pub current_pose: SubLevelPose,
    /// Pose one snapshot ago — interpolation source.
    /// Поза на снапшот раньше — источник интерполяции.
    pub previous_pose: SubLevelPose,
    /// Server game tick of `current_pose` (snapshot interpolationTick).
    /// Серверный тик `current_pose` (interpolationTick снапшота).
    pub game_tick: i32,
    /// Linear velocity, usable for extrapolation between snapshots.
    /// Линейная скорость, годится для экстраполяции между снапшотами.
    pub linear_velocity: [f32; 3],
    /// Angular velocity (radians/tick around each axis).
    /// Угловая скорость (радиан/тик вокруг каждой оси).
    pub angular_velocity: [f32; 3],
    /// Sub-level became static (chunks finalized) — `sable:finalize_sub_level`.
    /// Под-уровень стал статичным — `sable:finalize_sub_level`.
    pub finalized: bool,
    /// Movement finished — pose pinned at `current_pose` (`stop_moving` / snapshot_info stopped).
    /// Движение завершено — поза зафиксирована на `current_pose`.
    pub stopped: bool,
    /// Decoded chunk sections by LOCAL section coords (sx, sy, sz).
    /// Декодированные секции чанков по ЛОКАЛЬНЫМ координатам секций (sx, sy, sz).
    pub sections: HashMap<(i32, i32, i32), SectionData>,
}

impl SubLevel {
    /// Create a sub-level from `start_tracking_sub_level` data.
    /// Создать под-уровень из данных `start_tracking_sub_level`.
    ///
    /// Seeds the interpolator exactly like the vanilla client:
    /// (game_tick - 1, last_pose) and (game_tick, pose).
    pub fn new(
        id: u128,
        plot: (i32, i32),
        last_pose: SubLevelPose,
        pose: SubLevelPose,
        game_tick: i32,
    ) -> Self {
        Self {
            id,
            plot,
            current_pose: pose,
            previous_pose: last_pose,
            game_tick,
            linear_velocity: [0.0; 3],
            angular_velocity: [0.0; 3],
            finalized: false,
            stopped: false,
            sections: HashMap::new(),
        }
    }

    /// Global chunk X of a local chunk coordinate in this sub-level's plot.
    /// Глобальный X чанка для локальной координаты в плотe этого под-уровня.
    pub fn to_global_chunk_x(&self, local_x: i32) -> i32 {
        local_x + ((PLOT_ORIGIN + self.plot.0) << LOG_PLOT_SIZE)
    }

    /// Global chunk Z of a local chunk coordinate in this sub-level's plot.
    /// Глобальный Z чанка для локальной координаты в плотe этого под-уровня.
    pub fn to_global_chunk_z(&self, local_z: i32) -> i32 {
        local_z + ((PLOT_ORIGIN + self.plot.1) << LOG_PLOT_SIZE)
    }
}

/// Owns every tracked sub-level plus a plot -> id index (snapshots are keyed by plot).
/// Хранит все отслеживаемые под-уровни и индекс plot -> id (снапшоты ключуются плотом).
#[derive(Default)]
pub struct SubLevelManager {
    /// id (UUID as u128) -> sub-level.
    pub levels: HashMap<u128, SubLevel>,
    /// (px, pz) -> id, kept in sync with `levels`.
    /// (px, pz) -> id, синхронизируется с `levels`.
    plot_index: HashMap<(i32, i32), u128>,
}

impl SubLevelManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: u128) -> Option<&SubLevel> {
        self.levels.get(&id)
    }

    pub fn get_mut(&mut self, id: u128) -> Option<&mut SubLevel> {
        self.levels.get_mut(&id)
    }

    /// Look up by plot coordinates (snapshot/stop/finalize packets carry plotCoordinate).
    /// Поиск по координатам плота (пакеты snapshot/stop/finalize несут plotCoordinate).
    pub fn get_by_plot(&self, plot: (i32, i32)) -> Option<&SubLevel> {
        self.plot_index.get(&plot).and_then(|id| self.levels.get(id))
    }

    pub fn get_by_plot_mut(&mut self, plot: (i32, i32)) -> Option<&mut SubLevel> {
        let id = *self.plot_index.get(&plot)?;
        self.levels.get_mut(&id)
    }

    /// Insert (or replace) a sub-level; keeps the plot index consistent.
    /// Вставить (или заменить) под-уровень; поддерживает индекс плотов.
    pub fn insert(&mut self, level: SubLevel) {
        let id = level.id;
        let plot = level.plot;
        if let Some(old) = self.levels.insert(id, level) {
            // Replace stale plot mapping if the plot changed.
            // Убрать устаревшую запись, если плот изменился.
            if old.plot != plot {
                self.plot_index.remove(&old.plot);
            }
        }
        self.plot_index.insert(plot, id);
    }

    /// Remove a sub-level (`sable:stop_tracking_sub_level`), keeping its chunks out of memory.
    /// Удалить под-уровень (`sable:stop_tracking_sub_level`).
    pub fn remove(&mut self, id: u128) -> Option<SubLevel> {
        let level = self.levels.remove(&id)?;
        if self.plot_index.get(&level.plot) == Some(&id) {
            self.plot_index.remove(&level.plot);
        }
        Some(level)
    }

    /// Apply a decoded `sable:snapshot_packet`: advance previous/current poses per entry.
    /// Применить декодированный `sable:snapshot_packet`: сдвинуть previous/current позы.
    ///
    /// Entries are matched by plot coordinates. Unknown plots are ignored (we never saw
    /// `start_tracking_sub_level` for them). A received snapshot implies movement, so
    /// `stopped` is cleared.
    pub fn apply_snapshot(&mut self, snapshot: &crate::sable::SableSnapshot) {
        for entry in &snapshot.entries {
            let (px, pz) = crate::sable::plot_coord_parts(entry.plot_coordinate);
            let Some(level) = self.get_by_plot_mut((px, pz)) else {
                continue;
            };
            level.previous_pose = level.current_pose;
            level.current_pose = SubLevelPose {
                position: entry.position,
                rotation: entry.rotation,
                rotation_point: entry.rotation_point,
            };
            level.game_tick = snapshot.interpolation_tick;
            level.linear_velocity = entry.linear_velocity;
            level.angular_velocity = entry.angular_velocity;
            level.stopped = false;
        }
    }

    /// Render pose at partial tick `alpha` in [0, 1] between previous and current snapshots.
    /// Поза для рендера на частичном тике `alpha` в [0, 1] между снапшотами.
    ///
    /// Position/rotation_point lerp linearly, orientation slerps (matches
    /// `SubLevelSnapshotInterpolator.getSampleAt`). If `stopped`, the pose is pinned
    /// at `current_pose` regardless of alpha.
    pub fn interpolated_pose(&self, id: u128, alpha: f32) -> Option<SubLevelPose> {
        let level = self.levels.get(&id)?;
        if level.stopped {
            return Some(level.current_pose);
        }
        let t = alpha.clamp(0.0, 1.0);
        Some(SubLevelPose {
            position: lerp3(level.previous_pose.position, level.current_pose.position, t),
            rotation: slerp(
                level.previous_pose.rotation,
                level.current_pose.rotation,
                t,
            ),
            rotation_point: lerp3(
                level.previous_pose.rotation_point,
                level.current_pose.rotation_point,
                t,
            ),
        })
    }

    /// Given GLOBAL chunk coords, return the id of the sub-level that owns them, or None
    /// if the chunk is not inside the plot parking lot (normal overworld chunk).
    /// По ГЛОБАЛЬНЫМ координатам чанка вернуть id владеющего под-уровня либо None,
    /// если чанк вне «парковки» (обычный чанк мира).
    ///
    /// Mirrors `SubLevelContainer.inBounds(x, z)`.
    pub fn route_chunk(&mut self, chunk_x: i32, chunk_z: i32) -> Option<u128> {
        let plot = global_to_plot(chunk_x, chunk_z)?;
        self.plot_index.get(&plot).copied()
    }
}

/// Global chunk coord -> plot coord if inside the parking lot, else None.
/// Глобальная координата чанка -> координата плота, если внутри «парковки».
pub fn global_to_plot(chunk_x: i32, chunk_z: i32) -> Option<(i32, i32)> {
    let px = (chunk_x >> LOG_PLOT_SIZE) - PLOT_ORIGIN;
    let pz = (chunk_z >> LOG_PLOT_SIZE) - PLOT_ORIGIN;
    if (0..PLOT_SIZE).contains(&px) && (0..PLOT_SIZE).contains(&pz) {
        Some((px, pz))
    } else {
        None
    }
}

/// Global chunk coord -> LOCAL chunk coord within its plot, if inside the parking lot.
/// Глобальная координата чанка -> ЛОКАЛЬНАЯ внутри плота, если внутри «парковки».
pub fn global_to_local_chunk(chunk_x: i32, chunk_z: i32) -> Option<((i32, i32), (i32, i32))> {
    let plot = global_to_plot(chunk_x, chunk_z)?;
    let local_x = chunk_x - ((PLOT_ORIGIN + plot.0) << LOG_PLOT_SIZE);
    let local_z = chunk_z - ((PLOT_ORIGIN + plot.1) << LOG_PLOT_SIZE);
    Some((plot, (local_x, local_z)))
}

/// Pack two Java UUID longs (as read from the wire) into a u128.
/// Упаковать два long Java-UUID (из сети) в u128.
pub const fn uuid_from_longs(most: i64, least: i64) -> u128 {
    ((most as u64 as u128) << 64) | (least as u64 as u128)
}

/// Decode Java `ChunkPos.asLong` (low 32 bits = x, high 32 bits = z).
/// Декодировать Java `ChunkPos.asLong` (младшие 32 бита = x, старшие = z).
pub const fn plot_from_long(packed: i64) -> (i32, i32) {
    ((packed as i32, (packed >> 32) as i32))
}

/// Linear interpolation of two 3D points.
/// Линейная интерполяция двух 3D-точек.
pub fn lerp3(a: [f64; 3], b: [f64; 3], t: f32) -> [f64; 3] {
    let t = t.clamp(0.0, 1.0) as f64;
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Spherical linear interpolation between two quaternions (x, y, z, w).
/// Сферическая линейная интерполяция двух кватернионов (x, y, z, w).
///
/// Handles the double-cover (dot < 0) by negating `b`, and falls back to normalized
/// lerp when the quaternions are nearly parallel to avoid division by ~0.
pub fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    let mut b = b;
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];

    // Take the short path: q and -q encode the same rotation.
    // Короткий путь: q и -q задают одно вращение.
    if dot < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
        dot = -dot;
    }

    const DOT_THRESHOLD: f32 = 0.9995;
    if dot > DOT_THRESHOLD {
        // Nearly identical orientations: nlerp is stable and visually identical.
        // Почти совпадающие ориентации: nlerp стабилен и визуально неотличим.
        return normalize4([
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
            a[3] + (b[3] - a[3]) * t,
        ]);
    }

    let theta0 = dot.acos();
    let theta = theta0 * t;
    let sin_theta = theta.sin();
    let sin_theta0 = theta0.sin();
    let s0 = theta.cos() - dot * sin_theta / sin_theta0;
    let s1 = sin_theta / sin_theta0;

    [
        s0 * a[0] + s1 * b[0],
        s0 * a[1] + s1 * b[1],
        s0 * a[2] + s1 * b[2],
        s0 * a[3] + s1 * b[3],
    ]
}

/// Normalize a quaternion; degenerate (near-zero) input falls back to identity.
/// Нормализовать кватернион; вырожденный вход -> единичный кватернион.
fn normalize4(q: [f32; 4]) -> [f32; 4] {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len < 1e-10 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn lerp3_midpoint() {
        let m = lerp3([0.0, 0.0, 0.0], [10.0, 20.0, -30.0], 0.5);
        assert!(approx_eq(m[0], 5.0) && approx_eq(m[1], 10.0) && approx_eq(m[2], -15.0));
    }

    #[test]
    fn lerp3_clamps_alpha() {
        let m = lerp3([0.0; 3], [8.0; 3], 2.0);
        assert!(approx_eq(m[0], 8.0));
    }

    #[test]
    fn slerp_identity_stays_identity() {
        let i = [0.0f32, 0.0, 0.0, 1.0];
        let r = slerp(i, i, 0.37);
        assert!((r[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn slerp_90deg_yaw_midpoint_is_45deg() {
        // 90° around Y: q = (0, sin45, 0, cos45). Midpoint must be 45° around Y.
        let s = (std::f32::consts::FRAC_PI_4).sin();
        let c = (std::f32::consts::FRAC_PI_4).cos();
        let a = [0.0, 0.0, 0.0, 1.0];
        let b = [0.0, s, 0.0, c];
        let m = slerp(a, b, 0.5);
        let s22 = (std::f32::consts::FRAC_PI_8).sin();
        let c22 = (std::f32::consts::FRAC_PI_8).cos();
        assert!((m[1] - s22).abs() < 1e-5, "got {:?} want y={}", m, s22);
        assert!((m[3] - c22).abs() < 1e-5);
        // Unit length preserved.
        let len = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2] + m[3] * m[3]).sqrt();
        assert!((len - 1.0).abs() < 1e-5);
    }

    #[test]
    fn slerp_takes_short_path() {
        // b == -a encodes the same rotation; result must not spin the long way.
        let a = [0.0f32, 0.0, 0.0, 1.0];
        let b = [-0.0f32, -0.0, -0.0, -1.0];
        let m = slerp(a, b, 0.5);
        assert!((m[3].abs() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn plot_roundtrip() {
        // Plot (3, 7), local chunk (5, 2).
        let gx = 5 + ((PLOT_ORIGIN + 3) << LOG_PLOT_SIZE);
        let gz = 2 + ((PLOT_ORIGIN + 7) << LOG_PLOT_SIZE);
        assert_eq!(global_to_plot(gx, gz), Some((3, 7)));
        let (plot, local) = global_to_local_chunk(gx, gz).unwrap();
        assert_eq!(plot, (3, 7));
        assert_eq!(local, (5, 2));
    }

    #[test]
    fn outside_parking_lot_is_none() {
        assert_eq!(global_to_plot(0, 0), None); // normal overworld chunk
        assert_eq!(global_to_plot((PLOT_ORIGIN + PLOT_SIZE) << LOG_PLOT_SIZE, 0), None);
        assert_eq!(global_to_plot(-1, -1), None);
    }

    #[test]
    fn plot_from_long_matches_chunkpos_aslong() {
        // ChunkPos.asLong(x, z) = ((z & 0xFFFFFFFF) << 32) | (x & 0xFFFFFFFF)
        let packed: i64 = ((-7i64 & 0xFFFF_FFFF) << 32) | (12i64 & 0xFFFF_FFFF);
        assert_eq!(plot_from_long(packed), (12, -7));
    }

    #[test]
    fn uuid_from_longs_roundtrip() {
        let id = uuid_from_longs(0x0102_0304_0506_0708, -1);
        assert_eq!((id >> 64) as u64, 0x0102_0304_0506_0708);
        assert_eq!(id as u64, u64::MAX);
    }

    #[test]
    fn manager_insert_remove_keeps_plot_index() {
        let mut m = SubLevelManager::new();
        let id = 42u128;
        m.insert(SubLevel::new(
            id,
            (1, 2),
            SubLevelPose::default(),
            SubLevelPose::default(),
            0,
        ));
        assert!(m.get(id).is_some());
        assert!(m.get_by_plot((1, 2)).is_some());

        let gx = ((PLOT_ORIGIN + 1) << LOG_PLOT_SIZE) + 10;
        let gz = ((PLOT_ORIGIN + 2) << LOG_PLOT_SIZE) + 10;
        assert_eq!(m.route_chunk(gx, gz), Some(id));

        m.remove(id);
        assert!(m.get(id).is_none());
        assert!(m.get_by_plot((1, 2)).is_none());
        assert_eq!(m.route_chunk(gx, gz), None);
    }

    #[test]
    fn manager_route_unknown_plot_is_none() {
        let mut m = SubLevelManager::new();
        let gx = (PLOT_ORIGIN << LOG_PLOT_SIZE) + 1; // plot (0, 0) — inside lot, no owner
        let gz = (PLOT_ORIGIN << LOG_PLOT_SIZE) + 1;
        assert_eq!(m.route_chunk(gx, gz), None);
    }

    #[test]
    fn interpolated_pose_stopped_is_pinned() {
        let mut m = SubLevelManager::new();
        let id = 7u128;
        let prev = SubLevelPose {
            position: [0.0, 0.0, 0.0],
            ..Default::default()
        };
        let cur = SubLevelPose {
            position: [10.0, 0.0, 0.0],
            ..Default::default()
        };
        let mut level = SubLevel::new(id, (0, 0), prev, cur, 100);
        level.stopped = true;
        m.insert(level);

        let p = m.interpolated_pose(id, 0.5).unwrap();
        assert!(approx_eq(p.position[0], 10.0));

        // Moving: alpha interpolates.
        m.get_mut(id).unwrap().stopped = false;
        let p = m.interpolated_pose(id, 0.25).unwrap();
        assert!(approx_eq(p.position[0], 2.5));
    }

    #[test]
    fn section_data_new_is_air() {
        let s = SectionData::new(0);
        assert_eq!(s.blocks.len(), SECTION_VOLUME);
        assert!(s.blocks.iter().all(|&b| b == 0));
        assert!(s.light.is_none());
    }
}
