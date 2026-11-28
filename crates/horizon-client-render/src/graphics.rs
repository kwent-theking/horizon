//! Horizon's own graphics settings, beyond vanilla's options: kept in
//! `horizon-graphics.txt` next to `options.txt` so that file stays exactly
//! vanilla's. Every default reproduces vanilla's rendering; only choices the
//! player makes here depart from it.
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU32, Ordering},
};

pub const FILE: &str = "horizon-graphics.txt";
/// Written beside it before HDR output is switched on and removed once the first HDR frame is
/// presented. Found at the next start, it means HDR output took the game down, so HDR starts off.
pub const HDR_MARKER: &str = "horizon-hdr-pending";

/// What the system reports for the display HDR output goes to, in nits in HDR output's
/// scale: where its SDR white sits (the desktop's white) and the peak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reported {
    pub paper_white: u32,
    pub peak: u32,
}

static REPORTED: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Sets what the system reports (the renderer, when it opens its surface).
pub fn report(reported: Option<Reported>) {
    let (paper_white, peak) = reported.map_or((0, 0), |r| (r.paper_white, r.peak));
    REPORTED[0].store(paper_white, Ordering::Relaxed);
    REPORTED[1].store(peak, Ordering::Relaxed);
}
/// What the system reports, if it does.
pub fn reported() -> Option<Reported> {
    let [paper_white, peak] = REPORTED.each_ref().map(|n| n.load(Ordering::Relaxed));
    (peak > 0).then_some(Reported { paper_white, peak })
}

/// Simple-level presets. `Custom` leaves every option as the player set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Preset {
    Potato,
    Low,
    Medium,
    High,
    Ultra,
    #[default]
    Custom,
}
impl Preset {
    pub const ALL: [Preset; 6] = [
        Preset::Potato,
        Preset::Low,
        Preset::Medium,
        Preset::High,
        Preset::Ultra,
        Preset::Custom,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Preset::Potato => "potato",
            Preset::Low => "low",
            Preset::Medium => "medium",
            Preset::High => "high",
            Preset::Ultra => "ultra",
            Preset::Custom => "custom",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Preset::Potato => "Potato",
            Preset::Low => "Low",
            Preset::Medium => "Medium",
            Preset::High => "High",
            Preset::Ultra => "Ultra",
            Preset::Custom => "Custom",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "potato" => Preset::Potato,
            "low" | "performance" => Preset::Low,
            "medium" | "balanced" => Preset::Medium,
            "high" | "quality" => Preset::High,
            "ultra" => Preset::Ultra,
            _ => Preset::Custom,
        }
    }
}

/// Distance-based animation and detail reduction for distant mobs (never hides
/// mobs past culling; leaves nearby mobs at full framerate and fidelity).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EntityLod {
    #[default]
    Off,
    Balanced,
    Aggressive,
}
impl EntityLod {
    pub const ALL: [EntityLod; 3] = [EntityLod::Off, EntityLod::Balanced, EntityLod::Aggressive];
    pub fn name(self) -> &'static str {
        match self {
            EntityLod::Off => "off",
            EntityLod::Balanced => "balanced",
            EntityLod::Aggressive => "aggressive",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            EntityLod::Off => "Off",
            EntityLod::Balanced => "Balanced",
            EntityLod::Aggressive => "Aggressive",
        }
    }
    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|v| v.name() == s)
            .unwrap_or_default()
    }
}

/// How the swapchain presents frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PresentMode {
    /// Vanilla's choice: Fifo with VSync on, else the first of Immediate,
    /// Mailbox and Fifo the surface supports. The other modes apply only
    /// with VSync off: VSync on always presents with Fifo.
    #[default]
    Auto,
    Fifo,
    Mailbox,
    Immediate,
}
impl PresentMode {
    pub const ALL: [PresentMode; 4] = [
        PresentMode::Auto,
        PresentMode::Fifo,
        PresentMode::Mailbox,
        PresentMode::Immediate,
    ];
    pub fn name(self) -> &'static str {
        match self {
            PresentMode::Auto => "auto",
            PresentMode::Fifo => "fifo",
            PresentMode::Mailbox => "mailbox",
            PresentMode::Immediate => "immediate",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PresentMode::Auto => "Auto (VSync)",
            PresentMode::Fifo => "FIFO",
            PresentMode::Mailbox => "Mailbox",
            PresentMode::Immediate => "Immediate",
        }
    }
    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .unwrap_or_default()
    }
}

/// The graphics API the renderer runs on. Windows picks Vulkan, DirectX 12 or
/// OpenGL; Linux picks Vulkan or OpenGL; macOS always uses its one backend
/// (Metal). Applies at the next start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GraphicsApi {
    /// Vulkan, then DirectX 12 (Windows), then OpenGL when neither has a surface, adapter or device.
    #[default]
    Auto,
    Vulkan,
    Dx12,
    /// OpenGL, for GPUs with no Vulkan or DirectX 12 driver (Windows and Linux).
    OpenGl,
}
impl GraphicsApi {
    pub const ALL: [GraphicsApi; 4] = [
        GraphicsApi::Auto,
        GraphicsApi::Vulkan,
        GraphicsApi::Dx12,
        GraphicsApi::OpenGl,
    ];
    /// The APIs this system offers, in the order the setting cycles through
    /// them: DirectX 12 is Windows' third backend, and macOS has no choice.
    pub fn choices() -> &'static [GraphicsApi] {
        const WINDOWS: [GraphicsApi; 4] = [
            GraphicsApi::Auto,
            GraphicsApi::Vulkan,
            GraphicsApi::Dx12,
            GraphicsApi::OpenGl,
        ];
        const LINUX: [GraphicsApi; 3] =
            [GraphicsApi::Auto, GraphicsApi::Vulkan, GraphicsApi::OpenGl];
        const MACOS: [GraphicsApi; 1] = [GraphicsApi::Auto];
        if cfg!(windows) {
            &WINDOWS
        } else if cfg!(target_os = "macos") {
            &MACOS
        } else {
            &LINUX
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            GraphicsApi::Auto => "auto",
            GraphicsApi::Vulkan => "vulkan",
            GraphicsApi::Dx12 => "dx12",
            GraphicsApi::OpenGl => "opengl",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            GraphicsApi::Auto => "Auto",
            GraphicsApi::Vulkan => "Vulkan",
            GraphicsApi::Dx12 => "DirectX 12",
            GraphicsApi::OpenGl => "OpenGL",
        }
    }
    /// The API a name stands for, ignoring case; `None` for anything else.
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|api| api.name().eq_ignore_ascii_case(s.trim()))
    }
    /// `HORIZON_GRAPHICS_API` (auto, vulkan, dx12 or opengl) if set, else the setting.
    pub fn with_env(self) -> Self {
        std::env::var("HORIZON_GRAPHICS_API")
            .ok()
            .and_then(|v| Self::from_name(&v))
            .unwrap_or(self)
    }
}

/// What the frame limiter does in the last half millisecond before a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pacing {
    /// Sleep the whole wait: no CPU spent spinning, a little less precise.
    #[default]
    Efficient,
    /// Vanilla's `FramerateLimiter`: sleep, then spin until the deadline.
    Precise,
}

/// The performance overlay drawn over the HUD.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Overlay {
    #[default]
    Off,
    /// Frames per second and frame time.
    Fps,
    /// The numbers and a graph of recent frame times.
    Graph,
}

/// How finely far terrain is drawn (`FarDetail` in the LOD options).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FarDetail {
    /// Each tile level starts at eight times its size from the camera.
    #[default]
    Full,
    /// Coarser levels start at half that distance: about a quarter of the
    /// far quads, for GPUs that far terrain slows down. Changes the picture.
    Reduced,
}
impl FarDetail {
    pub fn name(self) -> &'static str {
        match self {
            FarDetail::Full => "full",
            FarDetail::Reduced => "reduced",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            FarDetail::Full => "Full",
            FarDetail::Reduced => "Reduced",
        }
    }
    fn parse(s: &str) -> Self {
        if s == "reduced" {
            FarDetail::Reduced
        } else {
            FarDetail::Full
        }
    }
}

/// TAA upscaling: the world renders this much smaller than the resolution scale's size,
/// and TAA builds the full-size picture from the jittered frames (FSR's and DLSS's ratios).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Upscaling {
    #[default]
    Native,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
}
impl Upscaling {
    pub const ALL: [Upscaling; 5] = [
        Upscaling::Native,
        Upscaling::Quality,
        Upscaling::Balanced,
        Upscaling::Performance,
        Upscaling::UltraPerformance,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Upscaling::Native => "native",
            Upscaling::Quality => "quality",
            Upscaling::Balanced => "balanced",
            Upscaling::Performance => "performance",
            Upscaling::UltraPerformance => "ultraPerformance",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Upscaling::Native => "Native",
            Upscaling::Quality => "Quality",
            Upscaling::Balanced => "Balanced",
            Upscaling::Performance => "Performance",
            Upscaling::UltraPerformance => "Ultra Performance",
        }
    }
    /// Output pixels per rendered pixel, along each axis.
    pub fn ratio(self) -> f32 {
        match self {
            Upscaling::Native => 1.0,
            Upscaling::Quality => 1.5,
            Upscaling::Balanced => 1.7,
            Upscaling::Performance => 2.0,
            Upscaling::UltraPerformance => 3.0,
        }
    }
    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|u| u.name() == s)
            .unwrap_or_default()
    }
}

static REDUCED_SUGGESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Whether the settings suggest Reduced far detail: set when the renderer
/// starts, on the GPUs `lod_capped` names (far terrain slows them most).
pub fn suggest_reduced_far_detail(on: bool) {
    REDUCED_SUGGESTED.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub fn reduced_far_detail_suggested() -> bool {
    REDUCED_SUGGESTED.load(std::sync::atomic::Ordering::Relaxed)
}

static HDR_OFFERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Whether the display path offers HDR output (HDR10 or scRGB): set when the renderer
/// opens its surface; the settings open HDR's screen only then.
pub fn offer_hdr(on: bool) {
    HDR_OFFERED.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub fn hdr_offered() -> bool {
    HDR_OFFERED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The farthest LOD Distance, in chunks, on integrated GPUs other than
/// Apple's: they share the CPU's memory bandwidth, and past it far terrain
/// costs them more frame time than it is worth.
pub const INTEGRATED_LOD_DISTANCE: u32 = 128;
/// Whether `INTEGRATED_LOD_DISTANCE` applies to this adapter. Apple's GPUs
/// are integrated too, with memory and bandwidth enough for every distance.
pub fn lod_capped(info: &wgpu::AdapterInfo) -> bool {
    info.device_type == wgpu::DeviceType::IntegratedGpu && !info.name.starts_with("Apple")
}
/// A LOD distance as far terrain uses it on this adapter.
pub fn lod_distance_capped(distance: u32, info: &wgpu::AdapterInfo) -> u32 {
    let distance = distance.min(1024);
    if lod_capped(info) {
        distance.min(INTEGRATED_LOD_DISTANCE)
    } else {
        distance
    }
}
impl Graphics {
    /// `lod_distance` as far terrain uses it on this adapter.
    pub fn lod_distance_on(&self, info: &wgpu::AdapterInfo) -> u32 {
        lod_distance_capped(self.lod_distance, info)
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Graphics {
    pub preset: Preset,
    /// Distant surface terrain range, in chunks; 0 disables LODs.
    pub lod_distance: u32,
    /// How finely far terrain is drawn; Full unless the player picks Reduced.
    pub far_detail: FarDetail,
    /// Skip sections hidden behind closed ones (vanilla's occlusion graph).
    pub cave_culling: bool,
    /// Skip drawing entities hidden behind opaque terrain.
    pub entity_culling: bool,
    /// Meshed sections uploaded per frame; 0 uploads all that are ready.
    pub chunk_updates: u32,
    /// Section mesh bytes uploaded per frame, in KiB; 0 is unlimited.
    pub upload_budget_kib: u32,
    pub sky: bool,
    pub fog: bool,
    pub weather: bool,
    pub animated_textures: bool,
    pub present_mode: PresentMode,
    pub pacing: Pacing,
    /// Chunk worker threads; 0 uses every core but one. Applies at the next start.
    pub workers: u32,
    pub overlay: Overlay,
    /// World resolution as a percentage of the window's (50..=200).
    pub resolution_scale: u32,
    /// FXAA over the world.
    pub fxaa: bool,
    /// TAA over the world (FXAA is off while it is on).
    pub taa: bool,
    /// TAA upscaling; anything but `Native` comes with `taa` on.
    pub upscaling: Upscaling,
    /// HDR output (HDR10, or scRGB), where the display path offers it.
    pub hdr: bool,
    /// HDR paper white (where SDR white sits), in nits; 0 is Auto: the desktop's SDR
    /// white, as the system reports it ([`Graphics::paper_white_nits`]).
    pub hdr_paper_white: u32,
    /// HDR peak brightness the world's highlights reach, in nits; 0 is Auto:
    /// what the system reports for the display ([`Graphics::peak_nits`]).
    pub hdr_peak: u32,
    /// HDR black level: the display's darkest black, in hundredths of a nit (0 for
    /// an OLED's true black). Shadows are lifted onto it, and the HDR metadata says it.
    pub hdr_black_level: u32,
    /// HDR brightness of the HUD and menus, in percent of paper white (25..=100); the
    /// first-person hand stays at the world's.
    pub hdr_ui_brightness: u32,
    /// Battery mode: at most 60 frames per second in a world and 30 in menus
    /// or behind other windows, and half the worker threads. Off by default;
    /// it changes when frames are drawn, never what they show or the ticks.
    pub battery: bool,
    /// The graphics API (Windows only; see `GraphicsApi`).
    pub api: GraphicsApi,
    /// The shader pack picked in `shaderpacks/` (`shader_pack_list`); empty: none. Its key is
    /// not `shaderPack`, which versions before the clean-room rewrite saved and which stays
    /// ignored.
    pub shader_pack: String,
    /// Maximum side length in pixels for mod textures (0 disables downscaling).
    pub mod_texture_cap: u32,
    /// Maximum mipmap levels uploaded per sprite layer (0 uploads the full chain).
    pub max_mipmap_levels: u32,
    /// Use BC1/BC7 block compression on static textures when the adapter supports it.
    pub texture_compression: bool,
    /// Distance-based animation and detail reduction for distant mobs.
    pub entity_lod: EntityLod,
    /// Driver identifier for which the legacy driver warning has already been shown.
    pub driver_warned: String,
}
impl Default for Graphics {
    fn default() -> Self {
        Self {
            preset: Preset::Custom,
            lod_distance: 0,
            far_detail: FarDetail::Full,
            cave_culling: true,
            entity_culling: false,
            chunk_updates: 0,
            upload_budget_kib: 0,
            sky: true,
            fog: true,
            weather: true,
            animated_textures: true,
            present_mode: PresentMode::Auto,
            pacing: Pacing::Efficient,
            workers: 0,
            overlay: Overlay::Off,
            resolution_scale: 100,
            fxaa: false,
            taa: false,
            upscaling: Upscaling::Native,
            hdr: false,
            hdr_paper_white: 0,
            hdr_peak: 0,
            hdr_black_level: 0,
            hdr_ui_brightness: 100,
            battery: false,
            api: GraphicsApi::Auto,
            shader_pack: String::new(),
            mod_texture_cap: 0,
            max_mipmap_levels: 0,
            texture_compression: false,
            entity_lod: EntityLod::Off,
            driver_warned: String::new(),
        }
    }
}
fn bool_text(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}
impl Graphics {
    pub fn load(dir: &Path) -> Self {
        let mut g: Self = fs::read_to_string(dir.join(FILE))
            .map(|t| Self::parse(&t))
            .unwrap_or_default();
        if fs::remove_file(dir.join(HDR_MARKER)).is_ok() && g.hdr {
            g.hdr = false;
            g.save(dir);
            log::warn!(
                "the last start ended before HDR output showed a frame; HDR is off now (graphics option hdr, in {FILE})"
            );
        }
        g
    }
    pub fn parse(text: &str) -> Self {
        let mut g = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            let flag = |d: bool| match value {
                "true" => true,
                "false" => false,
                _ => d,
            };
            let number = |d: u32| value.parse().unwrap_or(d);
            match key.trim() {
                "preset" => g.preset = Preset::parse(value),
                "lodDistance" => g.lod_distance = number(g.lod_distance).min(1024),
                "farDetail" => g.far_detail = FarDetail::parse(value),
                "caveCulling" => g.cave_culling = flag(g.cave_culling),
                "entityCulling" => g.entity_culling = flag(g.entity_culling),
                "chunkUpdates" => g.chunk_updates = number(g.chunk_updates).min(512),
                "uploadBudgetKiB" => g.upload_budget_kib = number(g.upload_budget_kib).min(65536),
                "sky" => g.sky = flag(g.sky),
                "fog" => g.fog = flag(g.fog),
                "weather" => g.weather = flag(g.weather),
                "animatedTextures" => g.animated_textures = flag(g.animated_textures),
                "presentMode" => g.present_mode = PresentMode::parse(value),
                "framePacing" => {
                    g.pacing = if value == "precise" {
                        Pacing::Precise
                    } else {
                        Pacing::Efficient
                    }
                }
                "workers" => g.workers = number(g.workers).min(64),
                "resolutionScale" => g.resolution_scale = number(g.resolution_scale).clamp(50, 200),
                "fxaa" => g.fxaa = flag(g.fxaa),
                "taa" => g.taa = flag(g.taa),
                "upscaling" => g.upscaling = Upscaling::parse(value),
                "hdr" => g.hdr = flag(g.hdr),
                "hdrPaperWhite" => {
                    g.hdr_paper_white = match number(g.hdr_paper_white) {
                        0 => 0,
                        n => n.clamp(80, 480),
                    }
                }
                "hdrPeak" => {
                    g.hdr_peak = match number(g.hdr_peak) {
                        0 => 0,
                        n => n.clamp(200, 2000),
                    }
                }
                "hdrBlackLevel" => g.hdr_black_level = number(g.hdr_black_level).min(100),
                "hdrUiBrightness" => {
                    g.hdr_ui_brightness = number(g.hdr_ui_brightness).clamp(25, 100)
                }
                "batteryMode" => g.battery = flag(g.battery),
                "graphicsApi" => g.api = GraphicsApi::from_name(value).unwrap_or_default(),
                "pickedShaderPack" => g.shader_pack = value.to_owned(),
                "modTextureCap" => {
                    let cap = number(g.mod_texture_cap);
                    g.mod_texture_cap = match cap {
                        0 => 0,
                        _ => cap.clamp(16, 1024),
                    };
                }
                "maxMipmapLevels" => g.max_mipmap_levels = number(g.max_mipmap_levels).min(4),
                "textureCompression" => g.texture_compression = flag(g.texture_compression),
                "entityLod" => g.entity_lod = EntityLod::parse(value),
                "driverWarned" => g.driver_warned = value.to_owned(),
                "overlay" => {
                    g.overlay = match value {
                        "fps" => Overlay::Fps,
                        "graph" => Overlay::Graph,
                        _ => Overlay::Off,
                    }
                }
                _ => {}
            }
        }
        g
    }
    pub fn serialize(&self) -> String {
        format!(
            "preset:{}\nlodDistance:{}\nfarDetail:{}\ncaveCulling:{}\nentityCulling:{}\nchunkUpdates:{}\nuploadBudgetKiB:{}\nsky:{}\nfog:{}\nweather:{}\nanimatedTextures:{}\npresentMode:{}\nframePacing:{}\nworkers:{}\noverlay:{}\nresolutionScale:{}\nfxaa:{}\ntaa:{}\nupscaling:{}\nhdr:{}\nhdrPaperWhite:{}\nhdrPeak:{}\nhdrBlackLevel:{}\nhdrUiBrightness:{}\nbatteryMode:{}\ngraphicsApi:{}\npickedShaderPack:{}\nmodTextureCap:{}\nmaxMipmapLevels:{}\ntextureCompression:{}\nentityLod:{}\ndriverWarned:{}\n",
            self.preset.name(),
            self.lod_distance,
            self.far_detail.name(),
            bool_text(self.cave_culling),
            bool_text(self.entity_culling),
            self.chunk_updates,
            self.upload_budget_kib,
            bool_text(self.sky),
            bool_text(self.fog),
            bool_text(self.weather),
            bool_text(self.animated_textures),
            self.present_mode.name(),
            match self.pacing {
                Pacing::Efficient => "efficient",
                Pacing::Precise => "precise",
            },
            self.workers,
            match self.overlay {
                Overlay::Off => "off",
                Overlay::Fps => "fps",
                Overlay::Graph => "graph",
            },
            self.resolution_scale,
            bool_text(self.fxaa),
            bool_text(self.taa),
            self.upscaling.name(),
            bool_text(self.hdr),
            self.hdr_paper_white,
            self.hdr_peak,
            self.hdr_black_level,
            self.hdr_ui_brightness,
            bool_text(self.battery),
            self.api.name(),
            self.shader_pack,
            self.mod_texture_cap,
            self.max_mipmap_levels,
            bool_text(self.texture_compression),
            self.entity_lod.name(),
            self.driver_warned,
        )
    }
    pub fn save(&self, dir: &Path) {
        let _ = fs::write(dir.join(FILE), self.serialize());
    }
    /// Paper white, in nits: the player's, or on Auto the desktop's SDR white as the
    /// system reports it, else 203 (BT.2408's reference white).
    pub fn paper_white_nits(&self) -> u32 {
        match self.hdr_paper_white {
            0 => reported().map_or(203, |r| r.paper_white),
            n => n,
        }
    }
    /// The peak the world's highlights reach, in nits: the player's, or on Auto
    /// what the system reports, else 1000.
    pub fn peak_nits(&self) -> u32 {
        match self.hdr_peak {
            0 => reported().map_or(1000, |r| r.peak),
            n => n,
        }
    }
    /// Worker threads for chunk generation and meshing: every core but one, as vanilla does. Only
    /// a low preset (Potato, Low) lets slow or single-channel memory bring that down.
    pub fn worker_threads(&self) -> usize {
        self.worker_threads_with(
            std::thread::available_parallelism().map_or(2, |n| n.get()),
            horizon_telemetry::system::memory_info,
        )
    }
    fn worker_threads_with(
        &self,
        cores: usize,
        memory: impl FnOnce() -> horizon_telemetry::system::MemoryInfo,
    ) -> usize {
        if self.workers > 0 {
            return self.workers as usize;
        }
        let auto = if matches!(self.preset, Preset::Potato | Preset::Low) {
            recommended_worker_threads(cores, memory())
        } else {
            cores.saturating_sub(1).max(1)
        };
        if self.battery { auto.div_ceil(2) } else { auto }
    }
    /// The frame cap battery mode adds: 60 in a world, 30 in menus without a
    /// world or while another window has focus.
    pub fn battery_limit(&self, limit: i32, in_world: bool, focused: bool) -> i32 {
        if !self.battery {
            limit
        } else if in_world && focused {
            limit.min(60)
        } else {
            limit.min(30)
        }
    }

    /// Logs a one-time warning when the active GPU driver is known to be legacy
    /// and persists `driverWarned` next to `options.txt` when `dir` exists.
    pub fn note_driver_warning(&mut self, dir: &Path, info: &wgpu::AdapterInfo) -> Option<String> {
        let warning = check_legacy_driver(info, std::env::consts::OS)?;
        let id = driver_id(info);
        if self.driver_warned != id {
            log::warn!("Horizon driver check: {warning}");
            self.driver_warned = id;
            if dir.is_dir() {
                self.save(dir);
            }
        }
        Some(warning)
    }
}

/// Every setting controlled by a named [`Preset`] in one tunable table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PresetSpec {
    pub preset: Preset,
    pub render_distance: i32,
    pub simulation_distance: i32,
    pub clouds: &'static str,
    pub particles: u8,
    pub entity_shadows: bool,
    pub biome_blend: i32,
    pub mipmap_levels: i32,
    pub entity_distance: f32,
    pub cutout_leaves: bool,
    pub weather_radius: i32,
    pub resolution_scale: u32,
    pub mod_texture_cap: u32,
    pub max_mipmap_levels: u32,
    pub texture_compression: bool,
    pub entity_lod: EntityLod,
    pub chunk_updates: u32,
    pub upload_budget_kib: u32,
}

/// Master table of named presets (tune here without touching apply/recommendation code).
pub const PRESET_TABLE: [PresetSpec; 5] = [
    PresetSpec {
        preset: Preset::Potato,
        render_distance: 6,
        simulation_distance: 5,
        clouds: "false",
        particles: 2,
        entity_shadows: false,
        biome_blend: 0,
        mipmap_levels: 0,
        entity_distance: 0.5,
        cutout_leaves: false,
        weather_radius: 3,
        resolution_scale: 75,
        mod_texture_cap: 32,
        max_mipmap_levels: 1,
        texture_compression: true,
        entity_lod: EntityLod::Aggressive,
        chunk_updates: 4,
        upload_budget_kib: 512,
    },
    PresetSpec {
        preset: Preset::Low,
        render_distance: 8,
        simulation_distance: 6,
        clouds: "false",
        particles: 2,
        entity_shadows: false,
        biome_blend: 0,
        mipmap_levels: 2,
        entity_distance: 0.75,
        cutout_leaves: false,
        weather_radius: 3,
        resolution_scale: 100,
        mod_texture_cap: 64,
        max_mipmap_levels: 2,
        texture_compression: true,
        entity_lod: EntityLod::Balanced,
        chunk_updates: 8,
        upload_budget_kib: 1024,
    },
    PresetSpec {
        preset: Preset::Medium,
        render_distance: 12,
        simulation_distance: 8,
        clouds: "fast",
        particles: 1,
        entity_shadows: true,
        biome_blend: 1,
        mipmap_levels: 2,
        entity_distance: 0.85,
        cutout_leaves: true,
        weather_radius: 5,
        resolution_scale: 100,
        mod_texture_cap: 128,
        max_mipmap_levels: 3,
        texture_compression: true,
        entity_lod: EntityLod::Balanced,
        chunk_updates: 16,
        upload_budget_kib: 2048,
    },
    PresetSpec {
        preset: Preset::High,
        render_distance: 16,
        simulation_distance: 12,
        clouds: "true",
        particles: 0,
        entity_shadows: true,
        biome_blend: 2,
        mipmap_levels: 4,
        entity_distance: 1.0,
        cutout_leaves: true,
        weather_radius: 10,
        resolution_scale: 100,
        mod_texture_cap: 256,
        max_mipmap_levels: 0,
        texture_compression: false,
        entity_lod: EntityLod::Off,
        chunk_updates: 0,
        upload_budget_kib: 0,
    },
    PresetSpec {
        preset: Preset::Ultra,
        render_distance: 24,
        simulation_distance: 12,
        clouds: "true",
        particles: 0,
        entity_shadows: true,
        biome_blend: 3,
        mipmap_levels: 4,
        entity_distance: 1.5,
        cutout_leaves: true,
        weather_radius: 10,
        resolution_scale: 100,
        mod_texture_cap: 0,
        max_mipmap_levels: 0,
        texture_compression: false,
        entity_lod: EntityLod::Off,
        chunk_updates: 0,
        upload_budget_kib: 0,
    },
];

pub fn preset_spec(preset: Preset) -> Option<&'static PresetSpec> {
    PRESET_TABLE.iter().find(|row| row.preset == preset)
}

/// Chooses automatic chunk worker threads based on CPU threads and DRAM bandwidth topology.
/// Slow DDR3 or single-channel buses stall when many threads contend on memory bandwidth.
pub fn recommended_worker_threads(
    logical_cores: usize,
    memory: horizon_telemetry::system::MemoryInfo,
) -> usize {
    // Vanilla's count (Util.maxAllowedExecutorThreads: every core but one).
    let base = logical_cores.saturating_sub(1).max(1);
    if memory.kind.is_ddr3_or_slower() && memory.single_channel {
        base.min(2)
    } else if memory.kind.is_ddr3_or_slower() || memory.single_channel {
        base.min((logical_cores / 2).clamp(2, 3))
    } else {
        base
    }
}

/// Recommends chunk section updates and upload budget (KiB) per frame for hardware profiles.
/// On slow DDR3, single-channel memory, or budget GPUs (like RX 580/Polaris), uncapped uploads (0, 0)
/// cause PCIe and DRAM memory bus saturation, leading to severe frame pacing spikes.
pub fn recommended_chunk_budget(profile: &HardwareProfile) -> (u32, u32) {
    if profile.memory_kind.is_ddr3_or_slower() && profile.single_channel {
        (8, 1024)
    } else if profile.memory_kind.is_ddr3_or_slower() || profile.single_channel {
        (16, 2048)
    } else if classify_gpu_tier(&profile.gpu_name, profile.device_type) == GpuTier::Entry {
        (8, 1024)
    } else if classify_gpu_tier(&profile.gpu_name, profile.device_type) == GpuTier::Budget {
        (16, 2048)
    } else {
        (0, 0)
    }
}

/// A preset's values for vanilla's options and Horizon's own.
pub fn apply_preset(preset: Preset, s: &mut crate::settings::Settings) {
    let profile = hardware_advice().map(|advice| advice.profile);
    apply_preset_for(preset, s, profile.as_ref());
}

/// A low preset's chunk budget, tightened to [`recommended_chunk_budget`] for the hardware when
/// that asks for less than the preset does (0 is unlimited).
fn low_spec_budget(preset: (u32, u32), recommended: (u32, u32)) -> (u32, u32) {
    let tighter = |preset: u32, recommended: u32| match (preset, recommended) {
        (0, r) | (r, 0) => r,
        (p, r) => p.min(r),
    };
    (
        tighter(preset.0, recommended.0),
        tighter(preset.1, recommended.1),
    )
}

fn apply_preset_for(
    preset: Preset,
    s: &mut crate::settings::Settings,
    profile: Option<&HardwareProfile>,
) {
    let Some(spec) = preset_spec(preset) else {
        return;
    };
    s.render_distance = spec.render_distance;
    s.simulation_distance = spec.simulation_distance;
    s.clouds = spec.clouds.into();
    s.particles = spec.particles;
    s.entity_shadows = spec.entity_shadows;
    s.biome_blend = spec.biome_blend;
    s.mipmap_levels = spec.mipmap_levels;
    s.entity_distance = spec.entity_distance;
    s.cutout_leaves = spec.cutout_leaves;
    s.weather_radius = spec.weather_radius;
    s.graphics_preset = "custom".into();
    let g = &mut s.horizon;
    g.preset = preset;
    g.cave_culling = true;
    g.animated_textures = true;
    let budget = match profile {
        Some(profile) if matches!(preset, Preset::Potato | Preset::Low) => low_spec_budget(
            (spec.chunk_updates, spec.upload_budget_kib),
            recommended_chunk_budget(profile),
        ),
        _ => (spec.chunk_updates, spec.upload_budget_kib),
    };
    (g.chunk_updates, g.upload_budget_kib) = budget;
    g.sky = true;
    g.fog = true;
    g.weather = true;
    g.resolution_scale = spec.resolution_scale;
    g.mod_texture_cap = spec.mod_texture_cap;
    g.max_mipmap_levels = spec.max_mipmap_levels;
    g.texture_compression = spec.texture_compression;
    g.entity_lod = spec.entity_lod;
}

/// Snapshot of system hardware fed into preset recommendation (pure data, testable without GPU/OS).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HardwareProfile {
    pub cpu_name: Option<String>,
    pub cpu_cores: Option<u16>,
    pub cpu_threads: Option<usize>,
    pub ram_mb: Option<u64>,
    pub memory_kind: horizon_telemetry::system::MemoryKind,
    pub memory_channels: u8,
    pub single_channel: bool,
    pub gpu_name: String,
    pub gpu_vendor: String,
    pub device_type: wgpu::DeviceType,
    pub backend: wgpu::Backend,
    pub vram_mb: Option<u32>,
    pub driver: String,
    pub driver_info: String,
    pub bc_supported: bool,
}

impl HardwareProfile {
    pub fn from_adapter(info: &wgpu::AdapterInfo, bc_supported: bool) -> Self {
        let gpu_vendor = horizon_telemetry::system::gpu_vendor(&info.name, info.vendor).to_owned();
        let vram_mb = if info.device_type == wgpu::DeviceType::DiscreteGpu {
            horizon_telemetry::system::vram_mb(&gpu_vendor)
        } else {
            None
        };
        // The memory probe runs in the background (see `prefetch_memory_info`); until it is done
        // the profile carries no memory facts and the advice is refreshed when it finishes.
        let memory = horizon_telemetry::system::memory_info_if_ready().unwrap_or_default();
        Self {
            cpu_name: horizon_telemetry::system::cpu_name(),
            cpu_cores: horizon_telemetry::system::physical_cores(),
            cpu_threads: std::thread::available_parallelism().ok().map(|n| n.get()),
            ram_mb: horizon_telemetry::system::ram_mb(),
            memory_kind: memory.kind,
            memory_channels: memory.channel_count,
            single_channel: memory.single_channel,
            gpu_name: info.name.clone(),
            gpu_vendor,
            device_type: info.device_type,
            backend: info.backend,
            vram_mb,
            driver: info.driver.clone(),
            driver_info: info.driver_info.clone(),
            bc_supported,
        }
    }
}

/// Result of non-forced hardware auto-detection shown in the graphics UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HardwareAdvice {
    pub profile: HardwareProfile,
    pub preset: Preset,
    pub summary: String,
    pub reason: String,
    pub driver_warning: Option<String>,
    pub memory_tip: Option<String>,
}

/// Parses up to 3 numeric components from the first version-like token in `text`.
fn first_version_tuple(text: &str) -> Option<(u32, u32, u32)> {
    for token in text.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
        if token.contains('.') && token.bytes().any(|b| b.is_ascii_digit()) {
            let mut nums = token.split('.').filter_map(|p| p.parse::<u32>().ok());
            if let Some(major) = nums.next() {
                let minor = nums.next().unwrap_or(0);
                let patch = nums.next().unwrap_or(0);
                return Some((major, minor, patch));
            }
        }
    }
    None
}

pub fn driver_id(info: &wgpu::AdapterInfo) -> String {
    let vendor = horizon_telemetry::system::gpu_vendor(&info.name, info.vendor);
    let drv = format!("{} {}", info.driver, info.driver_info);
    format!("{vendor}:{}", drv.trim())
}

/// Checks `info` for legacy or problematic driver versions without performing any I/O.
pub fn check_legacy_driver(info: &wgpu::AdapterInfo, os: &str) -> Option<String> {
    let vendor = horizon_telemetry::system::gpu_vendor(&info.name, info.vendor);
    check_legacy_driver_parts(
        vendor,
        &info.name,
        &info.driver,
        &info.driver_info,
        info.backend,
        os,
    )
}

fn check_legacy_driver_parts(
    vendor: &str,
    gpu_name: &str,
    driver: &str,
    driver_info: &str,
    backend: wgpu::Backend,
    os: &str,
) -> Option<String> {
    let combined = format!("{driver} {driver_info}");
    let lower = combined.to_ascii_lowercase();
    let gpu_lower = gpu_name.to_ascii_lowercase();
    let ver = first_version_tuple(driver_info).or_else(|| first_version_tuple(driver));

    match (vendor, os) {
        ("amd", "windows") => {
            if backend == wgpu::Backend::Gl {
                return Some(
                    "AMD OpenGL fallback active: update Adrenalin drivers or switch Graphics API to DirectX 12.".into(),
                );
            }
            let legacy_vulkan = matches!(ver, Some((2, 0, p)) if p < 250);
            let old_adrenalin = matches!(ver, Some((y, _, _)) if (18..=22).contains(&y));
            let polaris = gpu_lower.contains("rx 4")
                || gpu_lower.contains("rx 5")
                || gpu_lower.contains("polaris");
            if legacy_vulkan || old_adrenalin {
                return Some(if polaris {
                    "Old AMD Polaris/RX 500 driver detected: update Adrenalin (24.9.1+) or switch Graphics API to DirectX 12 if Vulkan stutters.".into()
                } else {
                    "Old AMD Adrenalin driver detected: updating the GPU driver improves Vulkan stability and VRAM handling.".into()
                });
            }
            None
        }
        ("amd", "linux") => {
            if lower.contains("amdvlk") {
                return Some(
                    "AMDVLK Vulkan driver detected: Mesa RADV typically offers better frame pacing and fewer shader stalls.".into(),
                );
            }
            if lower.contains("mesa") && matches!(ver, Some((m, _, _)) if m < 23) {
                return Some(
                    "Old Mesa driver (< 23.0) detected: update Mesa for Vulkan fixes.".into(),
                );
            }
            None
        }
        ("nvidia", "windows" | "linux") => {
            if matches!(ver, Some((m, _, _)) if (300..515).contains(&m)) {
                return Some(
                    "Legacy NVIDIA driver (< 515) detected: updating the driver is recommended for Vulkan stability.".into(),
                );
            }
            None
        }
        ("intel", "windows") => {
            if matches!(ver, Some((30 | 27 | 26 | 25, _, _))) || backend == wgpu::Backend::Gl {
                return Some(
                    "Old Intel graphics driver detected: update Intel Arc/Iris drivers or try DirectX 12.".into(),
                );
            }
            None
        }
        (_, "linux") if lower.contains("mesa") => {
            if matches!(ver, Some((m, _, _)) if m < 22) {
                return Some(
                    "Old Mesa driver (< 22.0) detected: updating Mesa is recommended.".into(),
                );
            }
            None
        }
        _ => None,
    }
}

/// Broad capability tier inferred from the GPU model string so older or entry-level
/// adapters still get appropriate recommendations even when OS VRAM reporting is absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuTier {
    Entry,
    Budget,
    Standard,
}

pub fn classify_gpu_tier(gpu_name: &str, device_type: wgpu::DeviceType) -> GpuTier {
    if device_type == wgpu::DeviceType::Cpu {
        return GpuTier::Entry;
    }
    let lower = gpu_name.to_ascii_lowercase();
    let has_any = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));

    if has_any(&[
        "gt 6",
        "gt 7",
        "gt 1030",
        "geforce 210",
        "geforce 6",
        "geforce 7",
        "geforce 8",
        "geforce 9",
        "gtx 4",
        "gtx 5",
        "gtx 650",
        "gtx 745",
        "gtx 750",
        "radeon hd ",
        "radeon r5",
        "radeon r7 2",
        "radeon r7 3",
        "rx 540",
        "rx 550",
        "vega 3",
        "vega 6",
        "intel(r) hd graphics",
        "intel hd graphics",
        "uhd graphics 600",
        "uhd graphics 605",
        "uhd graphics 610",
        "uhd graphics 620",
        "uhd graphics 630",
        "llvmpipe",
        "microsoft basic render",
    ]) {
        return GpuTier::Entry;
    }

    if has_any(&[
        "gtx 660",
        "gtx 670",
        "gtx 680",
        "gtx 760",
        "gtx 770",
        "gtx 780",
        "gtx 950",
        "gtx 960",
        "gtx 1050",
        "gtx 1060 3gb",
        "gtx 1630",
        "gtx 1650",
        "mx1",
        "mx2",
        "mx3",
        "mx4",
        "mx5",
        "radeon r9",
        "rx 460",
        "rx 470",
        "rx 480",
        "rx 560",
        "rx 570",
        "rx 580",
        "rx 590",
        "vega 8",
        "vega 11",
        "uhd graphics",
        "iris",
        "arc a3",
    ]) {
        return GpuTier::Budget;
    }

    GpuTier::Standard
}

/// Recommends (never forces) a graphics preset from detected CPU, RAM, GPU, VRAM, and driver.
pub fn recommend_preset(profile: &HardwareProfile) -> HardwareAdvice {
    let threads = profile.cpu_threads.unwrap_or(8);
    let cores = profile.cpu_cores.unwrap_or((threads / 2).max(1) as u16);
    let ram = profile.ram_mb;
    let vram = profile.vram_mb;
    let apple_igpu = profile.device_type == wgpu::DeviceType::IntegratedGpu
        && profile.gpu_name.starts_with("Apple");
    let non_apple_igpu = profile.device_type == wgpu::DeviceType::IntegratedGpu
        && !profile.gpu_name.starts_with("Apple");
    let software = profile.device_type == wgpu::DeviceType::Cpu || profile.gpu_vendor == "software";
    let gpu_tier = classify_gpu_tier(&profile.gpu_name, profile.device_type);

    let mut reasons = Vec::new();
    let preset = if software {
        reasons.push("software/CPU renderer".to_owned());
        Preset::Potato
    } else if ram.is_some_and(|r| r <= 6144)
        || vram.is_some_and(|v| v <= 2048)
        || threads <= 2
        || gpu_tier == GpuTier::Entry
    {
        if let Some(r) = ram.filter(|&r| r <= 6144) {
            reasons.push(format!("{:.1} GiB RAM", r as f64 / 1024.0));
        }
        if let Some(v) = vram.filter(|&v| v <= 2048) {
            reasons.push(format!("{v} MiB VRAM"));
        }
        if threads <= 2 {
            reasons.push(format!("{threads} CPU threads"));
        }
        if gpu_tier == GpuTier::Entry {
            reasons.push(format!("entry-level GPU ({})", profile.gpu_name));
        }
        Preset::Potato
    } else if ram.is_some_and(|r| r <= 9216)
        || vram.is_some_and(|v| v <= 4096)
        || (non_apple_igpu && ram.is_none_or(|r| r < 14336))
        || threads <= 4
        || cores <= 2
        || gpu_tier == GpuTier::Budget
    {
        if let Some(r) = ram.filter(|&r| r <= 9216) {
            reasons.push(format!("{:.0} GiB system RAM", (r as f64 / 1024.0).round()));
        }
        if let Some(v) = vram.filter(|&v| v <= 4096) {
            reasons.push(format!("{v} MiB VRAM"));
        }
        if non_apple_igpu {
            reasons.push("integrated GPU sharing system memory".to_owned());
        }
        if threads <= 4 || cores <= 2 {
            reasons.push(format!("{cores}c/{threads}t CPU"));
        }
        if gpu_tier == GpuTier::Budget && vram.is_none_or(|v| v > 4096) {
            reasons.push(format!("budget/legacy GPU ({})", profile.gpu_name));
        }
        Preset::Low
    } else if non_apple_igpu
        || ram.is_some_and(|r| r < 14336)
        || vram.is_some_and(|v| v <= 6144)
        || cores <= 4
    {
        if non_apple_igpu {
            reasons.push("integrated GPU".to_owned());
        }
        if let Some(r) = ram.filter(|&r| r < 14336) {
            reasons.push(format!("{:.0} GiB RAM", (r as f64 / 1024.0).round()));
        }
        if let Some(v) = vram.filter(|&v| v <= 6144) {
            reasons.push(format!("{v} MiB VRAM"));
        }
        if cores <= 4 {
            reasons.push(format!("{cores} CPU cores"));
        }
        Preset::Medium
    } else if ram.is_some_and(|r| r >= 30720)
        && ((vram.is_some_and(|v| v >= 10240)
            && profile.device_type == wgpu::DeviceType::DiscreteGpu)
            || apple_igpu)
        && threads >= 12
    {
        reasons.push(format!(
            "{:.0} GiB RAM, {} CPU threads, high-capacity GPU",
            (ram.unwrap_or(32768) as f64 / 1024.0).round(),
            threads
        ));
        Preset::Ultra
    } else {
        let mut ok = Vec::new();
        if let Some(r) = ram {
            ok.push(format!("{:.0} GiB RAM", (r as f64 / 1024.0).round()));
        }
        if let Some(v) = vram {
            ok.push(format!("{v} MiB VRAM"));
        }
        ok.push(format!("{cores}c/{threads}t CPU"));
        reasons.push(ok.join(", "));
        Preset::High
    };

    let cap_preset = |current: Preset, max: Preset| match (current, max) {
        (Preset::Potato, _) | (_, Preset::Potato) => Preset::Potato,
        (Preset::Low, _) | (_, Preset::Low) => Preset::Low,
        (Preset::Medium, _) | (_, Preset::Medium) => Preset::Medium,
        (Preset::High, _) | (_, Preset::High) => Preset::High,
        _ => current,
    };
    let step_down_one = |current: Preset| match current {
        Preset::Ultra => Preset::High,
        Preset::High => Preset::Medium,
        Preset::Medium => Preset::Low,
        Preset::Low | Preset::Potato | Preset::Custom => current,
    };
    let preset = if profile.memory_kind.is_ddr3_or_slower() && profile.single_channel {
        reasons.push(format!(
            "{} single-channel RAM",
            profile.memory_kind.label()
        ));
        let cap = if threads <= 4 || cores <= 2 || non_apple_igpu || ram.is_some_and(|r| r <= 8192)
        {
            Preset::Potato
        } else {
            Preset::Low
        };
        cap_preset(preset, cap)
    } else if profile.memory_kind.is_ddr3_or_slower() {
        reasons.push(format!("{} RAM", profile.memory_kind.label()));
        let cap = if non_apple_igpu || ram.is_some_and(|r| r <= 10240) {
            Preset::Low
        } else {
            Preset::Medium
        };
        cap_preset(preset, cap)
    } else if profile.single_channel {
        reasons.push("single-channel RAM".to_owned());
        step_down_one(preset)
    } else {
        preset
    };

    let memory_tip = profile.single_channel.then(|| {
        "Tip: two matching RAM sticks (dual channel) boost chunk streaming & FPS.".to_owned()
    });
    let driver_warning = check_legacy_driver_parts(
        &profile.gpu_vendor,
        &profile.gpu_name,
        &profile.driver,
        &profile.driver_info,
        profile.backend,
        std::env::consts::OS,
    );
    let mem_type = (profile.memory_kind != horizon_telemetry::system::MemoryKind::Unknown)
        .then(|| profile.memory_kind.label());
    let mem_ch = if profile.single_channel {
        Some("1-ch")
    } else if profile.memory_channels >= 2 {
        Some("2-ch")
    } else {
        None
    };
    let ram_label = ram.map_or_else(
        || "? RAM".to_owned(),
        |r| {
            let gb = format!("{:.0} GB", (r as f64 / 1024.0).round());
            match (mem_type, mem_ch) {
                (Some(kind), Some(ch)) => format!("{gb} {kind} ({ch})"),
                (Some(kind), None) => format!("{gb} {kind}"),
                (None, Some(ch)) => format!("{gb} RAM ({ch})"),
                (None, None) => format!("{gb} RAM"),
            }
        },
    );
    let vram_label = match vram {
        Some(v) => format!(" ({:.1} GB VRAM)", f64::from(v) / 1024.0),
        None => String::new(),
    };
    let cpu_label = format!("{cores}c/{threads}t");
    let summary = format!(
        "{cpu_label} | {ram_label} | {}{vram_label}",
        profile.gpu_name
    );
    let mut reason = if reasons.is_empty() {
        format!(
            "Recommended {} for balanced quality and memory",
            preset.label()
        )
    } else {
        format!("Recommended {}: {}", preset.label(), reasons.join(", "))
    };
    if let Some(tip) = &memory_tip {
        reason.push_str(" — ");
        reason.push_str(tip);
    }
    HardwareAdvice {
        profile: profile.clone(),
        preset,
        summary,
        reason,
        driver_warning,
        memory_tip,
    }
}

static LAST_ADAPTER: std::sync::RwLock<Option<(wgpu::AdapterInfo, bool)>> =
    std::sync::RwLock::new(None);
static HARDWARE_ADVICE: std::sync::RwLock<Option<HardwareAdvice>> = std::sync::RwLock::new(None);

/// Detects hardware from `info` and caches the latest [`HardwareAdvice`] globally.
pub fn detect_hardware_from_adapter(
    info: &wgpu::AdapterInfo,
    bc_supported: bool,
) -> HardwareAdvice {
    if let Ok(mut guard) = LAST_ADAPTER.write() {
        *guard = Some((info.clone(), bc_supported));
    }
    let profile = HardwareProfile::from_adapter(info, bc_supported);
    let advice = recommend_preset(&profile);
    set_hardware_advice(Some(advice.clone()));
    advice
}

/// Re-runs hardware detection using the active adapter (for the UI "Re-detect" button).
pub fn redetect_hardware() -> Option<HardwareAdvice> {
    let adapter = LAST_ADAPTER.read().ok()?.clone()?;
    Some(detect_hardware_from_adapter(&adapter.0, adapter.1))
}

pub fn set_hardware_advice(advice: Option<HardwareAdvice>) {
    if let Ok(mut guard) = HARDWARE_ADVICE.write() {
        *guard = advice;
    }
}

pub fn hardware_advice() -> Option<HardwareAdvice> {
    HARDWARE_ADVICE.read().ok().and_then(|g| g.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_choice_matches_the_system() {
        let choices = GraphicsApi::choices();
        if cfg!(windows) {
            assert_eq!(choices, GraphicsApi::ALL);
        } else if cfg!(target_os = "macos") {
            assert_eq!(choices, [GraphicsApi::Auto]);
        } else {
            assert_eq!(
                choices,
                [GraphicsApi::Auto, GraphicsApi::Vulkan, GraphicsApi::OpenGl]
            );
        }
    }

    #[test]
    fn round_trips() {
        let g = Graphics {
            preset: Preset::Medium,
            lod_distance: 256,
            far_detail: FarDetail::Reduced,
            cave_culling: false,
            entity_culling: true,
            chunk_updates: 12,
            upload_budget_kib: 4096,
            sky: false,
            fog: false,
            weather: false,
            animated_textures: false,
            present_mode: PresentMode::Mailbox,
            pacing: Pacing::Precise,
            workers: 6,
            overlay: Overlay::Graph,
            resolution_scale: 75,
            fxaa: true,
            taa: true,
            upscaling: Upscaling::Balanced,
            hdr: true,
            hdr_paper_white: 240,
            hdr_peak: 1400,
            hdr_black_level: 12,
            hdr_ui_brightness: 60,
            battery: true,
            api: GraphicsApi::Dx12,
            shader_pack: "Golden Hour".into(),
            mod_texture_cap: 128,
            max_mipmap_levels: 2,
            texture_compression: true,
            entity_lod: EntityLod::Balanced,
            driver_warned: "amd:2.0.210".into(),
        };
        assert_eq!(Graphics::parse(&g.serialize()), g);
        assert_eq!(Graphics::parse(""), Graphics::default());
        assert_eq!(Graphics::parse("preset:performance\n").preset, Preset::Low);
        assert_eq!(Graphics::parse("preset:balanced\n").preset, Preset::Medium);
        assert_eq!(Graphics::parse("preset:quality\n").preset, Preset::High);
        // 0 is Auto; anything else is a peak the slider reaches.
        assert_eq!(Graphics::parse("hdrPeak:0\n").hdr_peak, 0);
        assert_eq!(Graphics::parse("hdrPeak:50\n").hdr_peak, 200);
        assert_eq!(Graphics::parse("hdrPaperWhite:0\n").hdr_paper_white, 0);
        assert_eq!(Graphics::parse("hdrPaperWhite:5\n").hdr_paper_white, 80);
        // The old menus' Simple / Advanced choice is ignored, and so is the
        // shader pack older versions saved.
        assert_eq!(Graphics::parse("level:advanced\n"), Graphics::default());
        assert_eq!(
            Graphics::parse("shaderPack:Pack.zip\n"),
            Graphics::default()
        );
        assert_eq!(
            Graphics::parse("workers:abc\nsky:maybe\ncullLeaves:true\nentityCulling:maybe\n"),
            Graphics::default()
        );
    }

    #[test]
    fn integrated_gpus_other_than_apple_cap_the_lod_distance() {
        let adapter = |name: &str, device_type| wgpu::AdapterInfo {
            name: name.into(),
            vendor: 0,
            device: 0,
            device_type,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Vulkan,
            subgroup_min_size: 0,
            subgroup_max_size: 0,
            transient_saves_memory: None,
            limit_bucket: None,
        };
        let hd620 = adapter(
            "Intel(R) HD Graphics 620 (KBL GT2)",
            wgpu::DeviceType::IntegratedGpu,
        );
        let m4 = adapter("Apple M4 Pro", wgpu::DeviceType::IntegratedGpu);
        let discrete = adapter("NVIDIA GeForce RTX 4070", wgpu::DeviceType::DiscreteGpu);
        for (distance, capped) in [(0, 0), (64, 64), (128, 128), (256, 128), (1024, 128)] {
            let g = Graphics {
                lod_distance: distance,
                ..Graphics::default()
            };
            assert_eq!(g.lod_distance_on(&hd620), capped);
            assert_eq!(g.lod_distance_on(&m4), distance);
            assert_eq!(g.lod_distance_on(&discrete), distance);
        }
        assert!(
            crate::graphics_ui::rows(crate::graphics_ui::Level::Simple)
                .iter()
                .any(|row| matches!(row, crate::graphics_ui::Row::Setting(s)
                    if s.text.contains(&format!("{INTEGRATED_LOD_DISTANCE} chunks")))),
            "the LOD Distance description names the cap"
        );
    }

    #[test]
    fn lod_distance_is_opt_in_and_bounded() {
        assert_eq!(Graphics::default().lod_distance, 0);
        for distance in [0, 64, 128, 256, 512, 1024] {
            let g = Graphics::parse(&format!("lodDistance:{distance}\n"));
            assert_eq!(g.lod_distance, distance);
            assert_eq!(Graphics::parse(&g.serialize()), g);
        }
        assert_eq!(Graphics::parse("lodDistance:10000\n").lod_distance, 1024);
        assert_eq!(Graphics::parse("lodDistance:-1\n").lod_distance, 0);
        assert_eq!(Graphics::parse("lodDistance:invalid\n").lod_distance, 0);
    }

    #[test]
    fn far_detail_is_full_unless_reduced_is_picked() {
        assert_eq!(Graphics::default().far_detail, FarDetail::Full);
        assert_eq!(
            Graphics::parse("farDetail:reduced\n").far_detail,
            FarDetail::Reduced
        );
        for text in ["farDetail:full\n", "farDetail:other\n", ""] {
            assert_eq!(Graphics::parse(text).far_detail, FarDetail::Full);
        }
        // No preset changes it.
        for preset in Preset::ALL {
            let mut s = crate::settings::Settings::default();
            apply_preset(preset, &mut s);
            assert_eq!(s.horizon.far_detail, FarDetail::Full, "{preset:?}");
        }
    }

    #[test]
    fn every_preset_keeps_the_animated_textures_on() {
        // The Performance preset used to turn them off, which froze the fire,
        // the water, the lava and the portals (#2064).
        assert!(Graphics::default().animated_textures);
        for preset in Preset::ALL {
            let mut s = crate::settings::Settings::default();
            apply_preset(preset, &mut s);
            assert!(
                s.horizon.animated_textures,
                "{preset:?} turned the animated textures off"
            );
        }
    }

    #[test]
    fn automatic_workers_leave_a_core_as_vanilla_does() {
        let g = Graphics::default();
        assert_eq!(g.workers, 0);
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        assert_eq!(g.worker_threads(), cores.saturating_sub(1).max(1));
        // A manual count is the player's own choice, kept as asked.
        let manual = Graphics { workers: 3, ..g };
        assert_eq!(manual.worker_threads(), 3);
    }

    #[test]
    fn slow_memory_lowers_automatic_workers_only_on_low_presets() {
        use horizon_telemetry::system::{MemoryInfo, MemoryKind};
        let single_ddr3 = || MemoryInfo {
            kind: MemoryKind::Ddr3,
            stick_count: 1,
            channel_count: 1,
            single_channel: true,
            speed_mhz: Some(1600),
        };
        for preset in [Preset::Custom, Preset::Medium, Preset::High, Preset::Ultra] {
            let g = Graphics { preset, ..Graphics::default() };
            assert_eq!(g.worker_threads_with(12, single_ddr3), 11, "{preset:?}");
        }
        for preset in [Preset::Potato, Preset::Low] {
            let g = Graphics { preset, ..Graphics::default() };
            assert_eq!(g.worker_threads_with(12, single_ddr3), 2, "{preset:?}");
        }
        let manual = Graphics { preset: Preset::Low, workers: 8, ..Graphics::default() };
        assert_eq!(manual.worker_threads_with(12, single_ddr3), 8);
    }

    #[test]
    fn battery_mode_only_lowers_the_frame_cap() {
        let mut g = Graphics::default();
        assert_eq!(g.battery_limit(260, true, true), 260);
        g.battery = true;
        assert_eq!(g.battery_limit(260, true, true), 60);
        assert_eq!(g.battery_limit(30, true, true), 30);
        assert_eq!(g.battery_limit(260, false, true), 30);
        assert_eq!(g.battery_limit(260, true, false), 30);
        assert_eq!(g.battery_limit(10, true, false), 10);
    }

    #[test]
    fn recommends_presets_from_sample_hardware() {
        let base = HardwareProfile {
            cpu_name: Some("Sample CPU".into()),
            cpu_cores: Some(6),
            cpu_threads: Some(12),
            ram_mb: Some(16384),
            memory_kind: horizon_telemetry::system::MemoryKind::Unknown,
            memory_channels: 0,
            single_channel: false,
            gpu_name: "NVIDIA GeForce RTX 3060".into(),
            gpu_vendor: "nvidia".into(),
            device_type: wgpu::DeviceType::DiscreteGpu,
            backend: wgpu::Backend::Vulkan,
            vram_mb: Some(12288),
            driver: "NVIDIA".into(),
            driver_info: "555.99".into(),
            bc_supported: true,
        };
        let high = recommend_preset(&base);
        assert_eq!(high.preset, Preset::High);
        assert!(high.reason.contains("High"));

        let rx580 = HardwareProfile {
            cpu_cores: Some(4),
            cpu_threads: Some(8),
            ram_mb: Some(8192),
            gpu_name: "AMD Radeon (TM) RX 580 Series".into(),
            gpu_vendor: "amd".into(),
            vram_mb: Some(4096),
            driver: "AMD proprietary".into(),
            driver_info: "2.0.210".into(),
            ..base.clone()
        };
        let low = recommend_preset(&rx580);
        assert_eq!(low.preset, Preset::Low);
        assert!(low.summary.contains("RX 580"));
        assert!(low.reason.contains("8 GiB") || low.reason.contains("4096 MiB"));

        let potato = recommend_preset(&HardwareProfile {
            ram_mb: Some(4096),
            vram_mb: Some(2048),
            ..base.clone()
        });
        assert_eq!(potato.preset, Preset::Potato);

        let medium = recommend_preset(&HardwareProfile {
            gpu_name: "NVIDIA GeForce GTX 1060 6GB".into(),
            vram_mb: Some(6144),
            ..base.clone()
        });
        assert_eq!(medium.preset, Preset::Medium);

        let ultra = recommend_preset(&HardwareProfile {
            cpu_cores: Some(8),
            cpu_threads: Some(16),
            ram_mb: Some(32768),
            gpu_name: "NVIDIA GeForce RTX 4080".into(),
            vram_mb: Some(16384),
            ..base
        });
        assert_eq!(ultra.preset, Preset::Ultra);
    }

    #[test]
    fn legacy_driver_check_flags_old_amd_mesa_and_nvidia() {
        let make = |name: &str, vendor: u32, driver: &str, info: &str, backend| wgpu::AdapterInfo {
            name: name.into(),
            vendor,
            device: 0,
            device_type: wgpu::DeviceType::DiscreteGpu,
            device_pci_bus_id: String::new(),
            driver: driver.into(),
            driver_info: info.into(),
            backend,
            subgroup_min_size: 0,
            subgroup_max_size: 0,
            transient_saves_memory: None,
            limit_bucket: None,
        };
        let old_rx580 = make(
            "Radeon RX 580 Series",
            0x1002,
            "AMD proprietary driver",
            "2.0.210",
            wgpu::Backend::Vulkan,
        );
        assert!(check_legacy_driver(&old_rx580, "windows").is_some());
        let new_amd = make(
            "AMD Radeon RX 7800 XT",
            0x1002,
            "AMD proprietary driver",
            "2.0.315",
            wgpu::Backend::Vulkan,
        );
        assert!(check_legacy_driver(&new_amd, "windows").is_none());

        let old_mesa = make(
            "AMD Radeon RX 580",
            0x1002,
            "radv",
            "Mesa 21.3.9",
            wgpu::Backend::Vulkan,
        );
        assert!(check_legacy_driver(&old_mesa, "linux").is_some());
        let old_nv = make(
            "GeForce GTX 1060",
            0x10de,
            "NVIDIA",
            "470.199.02",
            wgpu::Backend::Vulkan,
        );
        assert!(check_legacy_driver(&old_nv, "windows").is_some());
        let new_nv = make(
            "GeForce RTX 3060",
            0x10de,
            "NVIDIA",
            "555.99",
            wgpu::Backend::Vulkan,
        );
        assert!(check_legacy_driver(&new_nv, "windows").is_none());
    }

    #[test]
    fn ddr3_and_single_channel_lower_recommended_preset_render_distance_and_workers() {
        use horizon_telemetry::system::{MemoryInfo, MemoryKind};

        let dual_ddr4 = MemoryInfo {
            kind: MemoryKind::Ddr4,
            stick_count: 2,
            channel_count: 2,
            single_channel: false,
            speed_mhz: Some(3200),
        };
        let single_ddr3 = MemoryInfo {
            kind: MemoryKind::Ddr3,
            stick_count: 1,
            channel_count: 1,
            single_channel: true,
            speed_mhz: Some(1600),
        };
        let single_ddr4 = MemoryInfo {
            kind: MemoryKind::Ddr4,
            stick_count: 1,
            channel_count: 1,
            single_channel: true,
            speed_mhz: Some(2666),
        };

        assert_eq!(recommended_worker_threads(12, dual_ddr4), 11);
        assert_eq!(recommended_worker_threads(12, single_ddr3), 2);
        assert_eq!(recommended_worker_threads(12, single_ddr4), 3);

        let base = HardwareProfile {
            cpu_name: Some("Sample CPU".into()),
            cpu_cores: Some(6),
            cpu_threads: Some(12),
            ram_mb: Some(16384),
            memory_kind: MemoryKind::Ddr4,
            memory_channels: 2,
            single_channel: false,
            gpu_name: "NVIDIA GeForce RTX 3060".into(),
            gpu_vendor: "nvidia".into(),
            device_type: wgpu::DeviceType::DiscreteGpu,
            backend: wgpu::Backend::Vulkan,
            vram_mb: Some(12288),
            driver: "NVIDIA".into(),
            driver_info: "555.99".into(),
            bc_supported: true,
        };
        let dual_advice = recommend_preset(&base);
        assert_eq!(dual_advice.preset, Preset::High);
        assert!(dual_advice.memory_tip.is_none());

        let single_ddr4_advice = recommend_preset(&HardwareProfile {
            memory_channels: 1,
            single_channel: true,
            ..base.clone()
        });
        assert_eq!(single_ddr4_advice.preset, Preset::Medium);
        assert!(
            preset_spec(single_ddr4_advice.preset)
                .unwrap()
                .render_distance
                < preset_spec(dual_advice.preset).unwrap().render_distance
        );
        assert!(
            single_ddr4_advice
                .memory_tip
                .as_deref()
                .is_some_and(|t| t.contains("dual channel"))
        );

        let ddr3_single_advice = recommend_preset(&HardwareProfile {
            cpu_cores: Some(4),
            cpu_threads: Some(4),
            ram_mb: Some(8192),
            memory_kind: MemoryKind::Ddr3,
            memory_channels: 1,
            single_channel: true,
            ..base.clone()
        });
        assert_eq!(ddr3_single_advice.preset, Preset::Potato);
        assert!(
            ddr3_single_advice
                .memory_tip
                .as_deref()
                .is_some_and(|t| t.contains("dual channel"))
        );
        assert!(ddr3_single_advice.reason.contains("dual channel"));

        // Entry-level and budget GPUs are recognized even when VRAM reporting is unavailable (None).
        let gt710 = recommend_preset(&HardwareProfile {
            gpu_name: "NVIDIA GeForce GT 710".into(),
            vram_mb: None,
            ..base.clone()
        });
        assert_eq!(gt710.preset, Preset::Potato);
        assert!(gt710.reason.contains("entry-level GPU"));

        let rx580_no_vram = recommend_preset(&HardwareProfile {
            gpu_name: "AMD Radeon RX 580 2048SP".into(),
            gpu_vendor: "amd".into(),
            vram_mb: None,
            ..base
        });
        assert_eq!(rx580_no_vram.preset, Preset::Low);
        assert!(rx580_no_vram.reason.contains("budget/legacy GPU"));

        let mut s = crate::settings::Settings::default();
        apply_preset_for(Preset::Potato, &mut s, None);
        assert_eq!(s.horizon.chunk_updates, 4);
        assert_eq!(s.horizon.upload_budget_kib, 512);
        apply_preset_for(Preset::Low, &mut s, None);
        assert_eq!(s.horizon.chunk_updates, 8);
        assert_eq!(s.horizon.upload_budget_kib, 1024);
        apply_preset_for(Preset::High, &mut s, None);
        assert_eq!(s.horizon.chunk_updates, 0);
        assert_eq!(s.horizon.upload_budget_kib, 0);
    }

    #[test]
    fn ddr3_and_budget_hardware_recommend_capped_chunk_budget() {
        use horizon_telemetry::system::MemoryKind;

        let high_end = HardwareProfile {
            cpu_name: Some("Core i9".into()),
            cpu_cores: Some(8),
            cpu_threads: Some(16),
            ram_mb: Some(32768),
            memory_kind: MemoryKind::Ddr5,
            memory_channels: 2,
            single_channel: false,
            gpu_name: "NVIDIA GeForce RTX 4080".into(),
            gpu_vendor: "nvidia".into(),
            device_type: wgpu::DeviceType::DiscreteGpu,
            backend: wgpu::Backend::Vulkan,
            vram_mb: Some(16384),
            driver: "NVIDIA".into(),
            driver_info: "560.00".into(),
            bc_supported: true,
        };
        assert_eq!(recommended_chunk_budget(&high_end), (0, 0));

        let ddr3_dual = HardwareProfile {
            memory_kind: MemoryKind::Ddr3,
            single_channel: false,
            ..high_end.clone()
        };
        assert_eq!(recommended_chunk_budget(&ddr3_dual), (16, 2048));

        let ddr3_single = HardwareProfile {
            memory_kind: MemoryKind::Ddr3,
            single_channel: true,
            ..high_end.clone()
        };
        assert_eq!(recommended_chunk_budget(&ddr3_single), (8, 1024));

        let rx580_ddr4 = HardwareProfile {
            gpu_name: "Radeon RX 580".into(),
            gpu_vendor: "amd".into(),
            memory_kind: MemoryKind::Ddr4,
            single_channel: false,
            ..high_end.clone()
        };
        assert_eq!(recommended_chunk_budget(&rx580_ddr4), (16, 2048));

        let gt710 = HardwareProfile {
            gpu_name: "GeForce GT 710".into(),
            memory_kind: MemoryKind::Ddr4,
            single_channel: false,
            ..high_end
        };
        assert_eq!(recommended_chunk_budget(&gt710), (8, 1024));

        // Only the low presets take the recommendation, and only when it is tighter.
        let mut s = crate::settings::Settings::default();
        apply_preset_for(Preset::Low, &mut s, Some(&ddr3_single));
        assert_eq!(
            (s.horizon.chunk_updates, s.horizon.upload_budget_kib),
            (8, 1024)
        );
        apply_preset_for(Preset::Potato, &mut s, Some(&ddr3_single));
        assert_eq!(
            (s.horizon.chunk_updates, s.horizon.upload_budget_kib),
            (4, 512)
        );
        apply_preset_for(Preset::High, &mut s, Some(&ddr3_single));
        assert_eq!(
            (s.horizon.chunk_updates, s.horizon.upload_budget_kib),
            (0, 0)
        );
        assert_eq!(low_spec_budget((0, 0), (16, 2048)), (16, 2048));
        assert_eq!(low_spec_budget((8, 1024), (0, 0)), (8, 1024));
    }
}
