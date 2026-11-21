//! An opt-in friends distribution, separate from the launcher's normal versions and updates.
//! `docs/friends-launcher.md` describes the signed channel and its local trust configuration.
use crate::{account, backend};
use horizon_release_sign::{self as signing, PublicKey};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::read::GzDecoder;
use tar::{Archive as TarArchive, EntryType};

const MANIFEST: &str = "friends.json";
const METADATA_LIMIT: u64 = 1 << 20;
const PAYLOAD_LIMIT: u64 = 4 << 30;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub repository: String,
    #[serde(default)]
    pub channel: String,
    /// Direct base URL for the owner's VPS pack host (e.g. `https://packs.example.invalid/friends`).
    /// With a token it must be https: the token is never sent over plain http.
    #[serde(default)]
    pub url: Option<String>,
    /// Shared password/token sent in `Authorization: Bearer` and `X-Horizon-Token` headers.
    #[serde(default)]
    pub token: Option<String>,
    /// Base64 Ed25519 public keys, received with the launcher, never from the update host.
    pub public_keys: String,
    /// Override automatic discovery for a nonstandard Minecraft installation.
    #[serde(default)]
    pub minecraft: Option<PathBuf>,
}

impl Config {
    pub fn read(path: &Path) -> Result<Self, String> {
        let mut config: Self =
            serde_json::from_slice(&fs::read(path).map_err(err)?).map_err(err)?;
        config.keys()?;
        if let Some(minecraft) = config.minecraft.as_mut().filter(|p| p.is_relative()) {
            *minecraft = std::path::absolute(path)
                .map_err(err)?
                .parent()
                .ok_or("Config has no parent folder")?
                .join(&*minecraft);
        }
        Ok(config)
    }

    fn has_token(&self) -> bool {
        self.token.as_deref().is_some_and(|t| !t.trim().is_empty())
    }

    fn base_url(&self) -> Option<&str> {
        self.url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(|u| u.trim_end_matches('/'))
    }

    fn keys(&self) -> Result<Vec<PublicKey>, String> {
        if let Some(base) = self.base_url() {
            let rest = base
                .strip_prefix("https://")
                .or_else(|| base.strip_prefix("http://"))
                .ok_or("Friends URL must start with http:// or https://")?;
            if rest.is_empty()
                || rest.starts_with('/')
                || base
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '?' | '#'))
            {
                return Err("Friends URL must be a valid HTTP(S) base URL".into());
            }
            if self.has_token() && base.starts_with("http://") {
                return Err(
                    "Friends URL must be https:// when a token is set; the token is never sent over plain http"
                        .into(),
                );
            }
        } else {
            let parts: Vec<_> = self.repository.split('/').collect();
            if parts.len() != 2 || !parts.iter().all(|p| safe_name(p)) || !safe_name(&self.channel)
            {
                return Err(
                    "Friends needs a VPS `url` or an owner/repository and release channel tag"
                        .into(),
                );
            }
        }
        let keys = signing::parse_keys(&self.public_keys)?;
        if keys.is_empty() {
            return Err("Friends needs the owner's public signing key in its local config".into());
        }
        Ok(keys)
    }

    fn scope(&self) -> String {
        match self.base_url() {
            Some(base) => format!("friends:{base}"),
            None => format!("friends:{}:{}", self.repository, self.channel),
        }
    }

    fn url(&self, name: &str) -> String {
        match self.base_url() {
            Some(base) => format!("{base}/{name}"),
            None => format!(
                "https://github.com/{}/releases/download/{}/{name}",
                self.repository, self.channel
            ),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "WireManifest")]
pub struct Manifest {
    pub schema: u32,
    /// Increases for every change, including a rollback to older client code.
    pub sequence: u64,
    pub minecraft: String,
    pub server: String,
    /// Optional single archive containing `client/`, `mods/`, `tables/`, and optional `mod-cache/`.
    pub bundle: Option<Artifact>,
    pub client: Option<Artifact>,
    pub mods: Vec<Artifact>,
    pub states: Option<Artifact>,
    pub registries: Option<Artifact>,
    pub codecs: Option<Artifact>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WireManifest {
    Bundle(WireBundleManifest),
    Multi(WireMultiManifest),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireBundleManifest {
    schema: u32,
    sequence: u64,
    minecraft: String,
    server: String,
    bundle: Artifact,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireMultiManifest {
    schema: u32,
    sequence: u64,
    minecraft: String,
    server: String,
    client: Artifact,
    mods: Vec<Artifact>,
    states: Artifact,
    registries: Artifact,
    codecs: Artifact,
}

impl TryFrom<WireManifest> for Manifest {
    type Error = &'static str;

    fn try_from(wire: WireManifest) -> Result<Self, Self::Error> {
        Ok(match wire {
            WireManifest::Bundle(b) => Self {
                schema: b.schema,
                sequence: b.sequence,
                minecraft: b.minecraft,
                server: b.server,
                bundle: Some(b.bundle),
                client: None,
                mods: Vec::new(),
                states: None,
                registries: None,
                codecs: None,
            },
            WireManifest::Multi(m) => Self {
                schema: m.schema,
                sequence: m.sequence,
                minecraft: m.minecraft,
                server: m.server,
                bundle: None,
                client: Some(m.client),
                mods: m.mods,
                states: Some(m.states),
                registries: Some(m.registries),
                codecs: Some(m.codecs),
            },
        })
    }
}

impl Manifest {
    fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.sequence == 0 || !safe_name(&self.minecraft) {
            return Err("Unsupported friends schema, sequence or Minecraft version".into());
        }
        if self.server.is_empty()
            || self.server.len() > 255
            || self.server.starts_with('-')
            || self
                .server
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("Friends manifest needs a server address".into());
        }
        if let Some(bundle) = &self.bundle {
            if self.client.is_some()
                || !self.mods.is_empty()
                || self.states.is_some()
                || self.registries.is_some()
                || self.codecs.is_some()
                || !(bundle.name.ends_with(".zip")
                    || bundle.name.ends_with(".tar.gz")
                    || bundle.name.ends_with(".tgz"))
            {
                return Err(
                    "Friends bundle manifest needs one .zip/.tar.gz archive without separate artifacts"
                        .into(),
                );
            }
        } else {
            let (Some(client), Some(states), Some(registries), Some(codecs)) =
                (&self.client, &self.states, &self.registries, &self.codecs)
            else {
                return Err(
                    "Friends needs a Windows x64 client, content mods and three TSV tables".into(),
                );
            };
            if client.name != "horizon-windows-x64.zip"
                || self.mods.len() > 512
                || self.mods.iter().any(|m| !m.name.ends_with(".hmod"))
                || !states.name.ends_with(".tsv")
                || !registries.name.ends_with(".registries.tsv")
                || !codecs.name.ends_with(".codecs.tsv")
            {
                return Err(
                    "Friends needs a Windows x64 client, content mods and three TSV tables".into(),
                );
            }
        }
        let mut names = BTreeSet::new();
        let mut total = 0u64;
        for a in self.artifacts() {
            if !safe_name(&a.name)
                || !names.insert(a.name.to_ascii_lowercase())
                || a.size == 0
                || a.sha256.len() != 64
                || !a.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(format!("Invalid or duplicate friends artifact: {}", a.name));
            }
            total = total.saturating_add(a.size);
        }
        if total > PAYLOAD_LIMIT {
            return Err("Friends downloads exceed 4 GiB".into());
        }
        Ok(())
    }

    fn artifacts(&self) -> Vec<&Artifact> {
        if let Some(bundle) = &self.bundle {
            return vec![bundle];
        }
        self.client
            .iter()
            .chain(&self.states)
            .chain(&self.registries)
            .chain(&self.codecs)
            .chain(&self.mods)
            .collect()
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// One portable filename, including Windows device names and trailing dots.
fn safe_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !name.is_empty()
        && name.len() <= 128
        && (name.as_bytes()[0].is_ascii_alphanumeric() || name == ".horizon-derived")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        && !name.ends_with('.')
        && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

fn authenticated_manifest(
    config: &Config,
    bytes: &[u8],
    sums: &[u8],
    signature: &[u8],
) -> Result<Manifest, String> {
    if [bytes.len(), sums.len(), signature.len()]
        .into_iter()
        .any(|n| n as u64 > METADATA_LIMIT)
    {
        return Err("Friends metadata exceeds 1 MiB".into());
    }
    // verify() accepts a domain as its tag. No upstream keys, legacy pins or nightly fallback.
    let sums = signing::verify(
        &config.scope(),
        sums,
        std::str::from_utf8(signature).map_err(err)?,
        &config.keys()?,
    )?;
    let digests: Vec<_> = sums.iter().filter(|(name, _)| name == MANIFEST).collect();
    if digests.len() != 1 {
        return Err("Signed checksums must list friends.json exactly once".into());
    }
    signing::check(MANIFEST, &digests[0].1, &signing::sha256_hex(bytes))?;
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(err)?;
    manifest.validate()?;
    Ok(manifest)
}

struct Lock(PathBuf);

impl Lock {
    fn take(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(err)?;
        let path = root.join("friends.lock");
        if lock_is_stale(&path) {
            // Two launchers may race here; create_new below lets only one of them win.
            let _ = fs::remove_file(&path);
        }
        let mut file = OpenOptions::new().write(true).create_new(true).open(&path)
            .map_err(|e| format!("{}: {e}. Close the other friends launcher; after a crash, remove this lock only when it is stopped.", path.display()))?;
        let lock = Self(path);
        writeln!(file, "{}", std::process::id()).map_err(err)?;
        Ok(lock)
    }
}

/// A lock whose recorded process is gone (a crash or power loss left it behind).
fn lock_is_stale(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .is_some_and(|pid| !process_alive(pid))
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    // SAFETY: both are plain kernel32 calls; the handle is checked for null and closed once.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // Only "no such process" proves it is gone; access denied means it exists.
            return std::io::Error::last_os_error().raw_os_error() != Some(ERROR_INVALID_PARAMETER);
        }
        CloseHandle(handle);
    }
    true
}

#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(any(windows, target_os = "linux")))]
fn process_alive(_pid: u32) -> bool {
    true
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// An accepted snapshot. Keep it alive until the game exits: its lock also prevents concurrent Play.
pub struct Ready {
    pub manifest: Manifest,
    pub snapshot: PathBuf,
    pub home: PathBuf,
    _lock: Lock,
}

impl Ready {
    fn mod_paths(&self) -> Vec<PathBuf> {
        if !self.manifest.mods.is_empty() {
            return self
                .manifest
                .mods
                .iter()
                .map(|m| self.snapshot.join("mods").join(&m.name))
                .collect();
        }
        sorted_hmods(&self.snapshot.join("mods"))
    }

    /// Builds the command without starting a process. Mods are explicit, so removed mods stay out.
    pub fn command(&self) -> Command {
        build_client_command(
            &self.snapshot,
            &self.home,
            &self.manifest.server,
            &self.mod_paths(),
        )
    }

    pub fn authenticated_command(&self) -> Result<Command, String> {
        let (session, offline) = account::session_for_game(&self.home, false).map_err(err)?;
        let mut cmd = self.command();
        if let Some(file) = session {
            cmd.arg("--session-file").arg(file);
        }
        if let Some(why) = offline {
            eprintln!("Playing offline: {why}");
        }
        let accounts = account::Accounts::load();
        if let Some(name) = accounts.name_arg() {
            cmd.arg("--name").arg(name);
        }
        cmd.envs(accounts.look_env());
        Ok(cmd)
    }
}

fn build_client_command(snapshot: &Path, home: &Path, server: &str, mods: &[PathBuf]) -> Command {
    let mut cmd = Command::new(snapshot.join("client/horizon-client.exe"));
    cmd.current_dir(home)
        .env("HORIZON_HOME", home)
        .env(
            "HORIZON_SERVER_STATES",
            snapshot.join("tables/server-states.tsv"),
        )
        .arg("--resources")
        .arg(backend::resources_at(&snapshot.join("client")))
        .arg("--server")
        .arg(server);
    for path in mods {
        cmd.arg("--dev-mod").arg(path);
    }
    cmd
}

fn sorted_hmods(mods_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(entries) = fs::read_dir(mods_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "hmod") && path.is_file() {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

/// Check the opted-in channel or VPS pack, download/verify its snapshot, then prepare from this player's install
/// (falling back to Mojang for a signed-in owner) and prewarm caches before Play.
/// The default launcher never calls this and its upstream source and self-update policy stay intact.
pub fn update(config: &Config, root: &Path, log: &dyn Fn(String)) -> Result<Ready, String> {
    if !cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        return Err("The friends MVP supports Windows x64 only".into());
    }
    let minecraft = backend::minecraft_dir(config.minecraft.clone())?;
    // The token belongs to the VPS pack host only; GitHub downloads redirect to other hosts.
    let token = config.base_url().and(config.token.as_deref());
    let ready = update_with(
        config,
        root,
        &|url| metadata_with_token(url, token),
        &|url, a, to| {
            log(format!("Downloading {}", a.name));
            // Read at most the signed size plus one byte, then check_file hashes the staged file.
            // A host serving a larger body cannot fill the disk before the size check rejects it.
            let mut body = backend::get_with_token(url, token)?
                .into_reader()
                .take(a.size + 1);
            let mut file = File::create(to).map_err(err)?;
            std::io::copy(&mut body, &mut file).map_err(err)?;
            Ok(())
        },
        &|dir| prepare(dir, &minecraft, log),
    )?;
    prewarm_client_caches(&ready, log);
    Ok(ready)
}

fn metadata_with_token(url: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    backend::get_with_token(url, token)?
        .into_reader()
        .take(METADATA_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() as u64 > METADATA_LIMIT {
        return Err("Friends metadata exceeds 1 MiB".into());
    }
    Ok(bytes)
}

fn prepare(dir: &Path, minecraft: &Path, log: &dyn Fn(String)) -> Result<(), String> {
    let (preparer, minecraft) =
        backend::preparer_at(dir, minecraft, backend::MinecraftFrom::Install, log)?;
    match preparer {
        backend::Preparer::InProcess(options) => backend::prepare_in_process(&options, log)?,
        backend::Preparer::Process(mut cmd) => {
            if !cmd.status().map_err(err)?.success() {
                return Err(
                    "Resource preparation failed; the previous snapshot is still accepted".into(),
                );
            }
        }
    }
    // Optional sounds still come only from the player's asset store.
    if let Err(why) = horizon_prepare::sounds(dir, Some(&minecraft), &|line| log(line.into())) {
        log(format!("No sound: {why}"));
    }
    Ok(())
}

/// Whether `error` says no Minecraft install was found (the only failure a sign-in can fix).
pub fn is_minecraft_not_found(error: &str) -> bool {
    crate::sources::is_not_found(error)
}

/// Written last, once the derived caches in `mod-content` are whole. A bundle cannot carry it
/// (dot-prefixed names are refused), so a half-copied or half-built cache never counts.
const CACHE_COMPLETE: &str = ".friends-cache-complete";

fn mod_content(home: &Path) -> PathBuf {
    home.join(".horizon-cache/mod-content")
}

pub fn has_prebuilt_derived_caches(home: &Path) -> bool {
    if !mod_content(home).join(CACHE_COMPLETE).is_file() {
        return false;
    }
    let derived = mod_content(home).join(".horizon-derived");
    const KEY_CACHES: [&str; 4] = [
        "baked-render-v2",
        "block-facts-v3",
        "items-table-v1",
        "prepared-textures-v1",
    ];
    KEY_CACHES.iter().all(|name| {
        let p = derived.join(name);
        fs::metadata(&p)
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false)
    })
}

/// The client's stage lines worth showing while it prebuilds: one per finished stage, not
/// every log line.
fn is_prewarm_progress(line: &str) -> bool {
    ["stage=", "mod content root built", "resources loaded in"]
        .iter()
        .any(|mark| line.contains(mark))
}

fn prewarm_client_caches(ready: &Ready, log: &dyn Fn(String)) {
    let exe = ready.snapshot.join("client/horizon-client.exe");
    if !exe.is_file() || !ready.snapshot.join("tables/server-states.tsv").is_file() {
        return;
    }
    if has_prebuilt_derived_caches(&ready.home) {
        log("Using prebuilt derived game caches".into());
        return;
    }
    log("Prebuilding derived game caches (the first time takes several minutes)".into());
    let mut cmd = ready.command();
    cmd.arg("--prepare-caches")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    // Streamed, not collected: a silent multi-minute wait looks like a hang.
    let run = cmd.spawn().and_then(|mut child| {
        let mut last = String::new();
        if let Some(stderr) = child.stderr.take() {
            // Bytes, not `lines()`: one non-UTF-8 line must not stop draining the pipe.
            let mut reader = std::io::BufReader::new(stderr);
            let mut buf = Vec::new();
            while std::io::BufRead::read_until(&mut reader, b'\n', &mut buf).is_ok_and(|n| n > 0) {
                let line = String::from_utf8_lossy(&buf).trim_end().to_owned();
                buf.clear();
                if is_prewarm_progress(&line) {
                    log(line.clone());
                }
                last = line;
            }
        }
        child.wait().map(|status| (status, last))
    });
    match run {
        Ok((status, _)) if status.success() => {
            let cache = mod_content(&ready.home);
            match fs::create_dir_all(&cache)
                .and_then(|_| fs::write(cache.join(CACHE_COMPLETE), b""))
            {
                Ok(()) => log("Derived game caches are ready".into()),
                Err(why) => log(format!("Derived game caches are ready but unmarked: {why}")),
            }
        }
        Ok((_, last)) => {
            let detail = if last.is_empty() { "non-zero exit" } else { last.as_str() };
            log(format!(
                "Derived cache prebuild skipped (will build on launch): {detail}"
            ));
        }
        Err(why) => {
            log(format!(
                "Derived cache prebuild skipped (will build on launch): {why}"
            ));
        }
    }
}

fn current(root: &Path) -> Result<Option<(u64, String)>, String> {
    let accepted = root.join("accepted");
    fs::create_dir_all(&accepted).map_err(err)?;
    let mut newest = None;
    for entry in fs::read_dir(accepted).map_err(err)? {
        let path = entry.map_err(err)?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let bytes = fs::read(&path).map_err(err)?;
        let m: Manifest = serde_json::from_slice(&bytes).map_err(err)?;
        let hash = signing::sha256_hex(&bytes);
        let expected = record_name(m.sequence, &hash);
        if path.file_name().and_then(|s| s.to_str()) != Some(expected.as_str()) {
            return Err("Invalid friends update history".into());
        }
        let candidate = (m.sequence, hash);
        if newest.as_ref().is_none_or(|old| &candidate > old) {
            newest = Some(candidate);
        }
    }
    Ok(newest)
}

fn record_name(sequence: u64, hash: &str) -> String {
    format!("{sequence:020}-{hash}.json")
}

fn update_with(
    config: &Config,
    root: &Path,
    fetch: &impl Fn(&str) -> Result<Vec<u8>, String>,
    download: &impl Fn(&str, &Artifact, &Path) -> Result<(), String>,
    prepare: &impl Fn(&Path) -> Result<(), String>,
) -> Result<Ready, String> {
    config.keys()?;
    // A repository/channel switch cannot overwrite another channel's files or update history.
    let root = std::path::absolute(root)
        .map_err(err)?
        .join(signing::sha256_hex(config.scope().as_bytes()));
    let lock = Lock::take(&root)?;
    let bytes = fetch(&config.url(MANIFEST))?;
    let m = authenticated_manifest(
        config,
        &bytes,
        &fetch(&config.url(signing::SUMS))?,
        &fetch(&config.url(signing::SIGNATURE))?,
    )?;
    let hash = signing::sha256_hex(&bytes);
    let previous = current(&root)?;
    if previous.as_ref().is_some_and(|(sequence, digest)| {
        m.sequence < *sequence || (m.sequence == *sequence && hash != *digest)
    }) {
        return Err(
            "Refusing an older or changed friends sequence; the owner must increment it".into(),
        );
    }
    let snapshot = root.join("snapshots").join(&hash);
    let home = root.join("home");
    fs::create_dir_all(&home).map_err(err)?;
    let prepared = backend::resources_at(&snapshot.join("client")).join("horizon-resources.json");
    if previous.as_ref() != Some(&(m.sequence, hash.clone())) || !prepared.is_file() {
        // Only an unaccepted/interrupted snapshot, or a damaged current snapshot, is replaced.
        // The lock is held through preparation and Play; no process from this profile can use it.
        if snapshot.exists() {
            fs::remove_dir_all(&snapshot).map_err(err)?;
        }
        for dir in ["mods", "tables", "downloads"] {
            fs::create_dir_all(snapshot.join(dir)).map_err(err)?;
        }
        if let Some(bundle) = &m.bundle {
            let to = snapshot.join("downloads").join(&bundle.name);
            download(&config.url(&bundle.name), bundle, &to)?;
            check_file(bundle, &to)?;
            unpack_bundle(&to, &snapshot)?;
            let _ = fs::remove_dir_all(snapshot.join("downloads"));
            let pins = horizon_prepare::artifacts::Lock::read(&snapshot.join("client"))?;
            if pins.version != m.minecraft {
                return Err("Friends manifest and client pin different Minecraft versions".into());
            }
            if let Some((sequence, old_hash)) = previous.as_ref().filter(|(_, h)| h != &hash) {
                let old: Manifest = serde_json::from_slice(
                    &fs::read(root.join("accepted").join(record_name(*sequence, old_hash)))
                        .map_err(err)?,
                )
                .map_err(err)?;
                let old_client = root.join("snapshots").join(old_hash).join("client");
                if old.minecraft == m.minecraft
                    && same_client_bin(&old_client, &snapshot.join("client"))
                    && backend::resources_at(&old_client)
                        .join("horizon-resources.json")
                        .is_file()
                {
                    backend::copy_dir(
                        &backend::resources_at(&old_client),
                        &backend::resources_at(&snapshot.join("client")),
                    )
                    .map_err(err)?;
                }
            }
            if !prepared.is_file() {
                prepare(&snapshot.join("client"))?;
            }
        } else {
            let client_art = m
                .client
                .as_ref()
                .ok_or("Friends manifest is missing client artifact")?;
            let states = m
                .states
                .as_ref()
                .ok_or("Friends manifest is missing states table")?;
            let registries = m
                .registries
                .as_ref()
                .ok_or("Friends manifest is missing registries table")?;
            let codecs = m
                .codecs
                .as_ref()
                .ok_or("Friends manifest is missing codecs table")?;
            // Exact same client archive: reuse only resources prepared on this player's computer.
            // Mods and server tables can update without repeating the Java exporters.
            let reusable = previous
                .as_ref()
                .filter(|(_, old_hash)| old_hash != &hash)
                .map(|(sequence, old_hash)| -> Result<Option<PathBuf>, String> {
                    let old: Manifest = serde_json::from_slice(
                        &fs::read(root.join("accepted").join(record_name(*sequence, old_hash)))
                            .map_err(err)?,
                    )
                    .map_err(err)?;
                    let dir = root.join("snapshots").join(old_hash).join("client");
                    Ok((old.client.as_ref().map(|c| (&c.sha256, c.size))
                        == Some((&client_art.sha256, client_art.size))
                        && old.minecraft == m.minecraft
                        && backend::resources_at(&dir)
                            .join("horizon-resources.json")
                            .is_file())
                    .then_some(dir))
                })
                .transpose()?
                .flatten();
            for a in m.artifacts() {
                if a.name == client_art.name && reusable.is_some() {
                    continue;
                }
                let to = snapshot.join("downloads").join(&a.name);
                download(&config.url(&a.name), a, &to)?;
                check_file(a, &to)?;
            }
            match &reusable {
                Some(from) => backend::copy_dir(from, &snapshot.join("client")).map_err(err)?,
                None => unpack_client(
                    &snapshot.join("downloads").join(&client_art.name),
                    &snapshot.join("client"),
                )?,
            }
            let pins = horizon_prepare::artifacts::Lock::read(&snapshot.join("client"))?;
            if pins.version != m.minecraft {
                return Err("Friends manifest and client pin different Minecraft versions".into());
            }
            let mut staged_mods = Vec::with_capacity(m.mods.len());
            for a in &m.mods {
                staged_mods.push(snapshot.join("downloads").join(&a.name));
            }
            validate_mods(&staged_mods)?;
            for (a, from) in m.mods.iter().zip(staged_mods) {
                fs::rename(from, snapshot.join("mods").join(&a.name)).map_err(err)?;
            }
            for (a, name) in [
                (states, "server-states.tsv"),
                (registries, "server-states.registries.tsv"),
                (codecs, "server-states.codecs.tsv"),
            ] {
                fs::rename(
                    snapshot.join("downloads").join(&a.name),
                    snapshot.join("tables").join(name),
                )
                .map_err(err)?;
            }
            if reusable.is_none() {
                prepare(&snapshot.join("client"))?;
            }
        }
        if !prepared.is_file() {
            return Err("Preparation did not produce horizon-resources.json".into());
        }
        // Immutable journal entries avoid replacing a live pointer (including on Windows).
        // A crash before the rename leaves the preceding accepted sequence in effect.
        if previous.as_ref() != Some(&(m.sequence, hash.clone())) {
            let pending = root.join("accepted/pending");
            let mut file = File::create(&pending).map_err(err)?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(err)?;
            drop(file);
            fs::rename(
                pending,
                root.join("accepted").join(record_name(m.sequence, &hash)),
            )
            .map_err(err)?;
        }
        // The previous prewarm was keyed to the old exe and files: without this, an update that
        // ships no mod-cache keeps the old marker, skips the prewarm and rebuilds on first Play.
        let _ = fs::remove_file(mod_content(&home).join(CACHE_COMPLETE));
        if m.bundle.is_some() {
            // Only an accepted snapshot may replace the player's cache. It is optional: the game
            // rebuilds it, so a failed seeding must not undo the acceptance.
            let _ = seed_home_cache(&snapshot, &home);
        }
        let mut keep = vec![hash.as_str()];
        keep.extend(previous.iter().map(|(_, old)| old.as_str()));
        prune_snapshots(&root, &keep);
    }
    Ok(Ready {
        manifest: m,
        snapshot,
        home,
        _lock: lock,
    })
}

/// Deletes every snapshot but the `keep` ones (the current and the previous, which a later
/// update reuses). Best effort: a snapshot that cannot be removed now goes next time.
fn prune_snapshots(root: &Path, keep: &[&str]) {
    let Ok(entries) = fs::read_dir(root.join("snapshots")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !keep.iter().any(|k| *k == &*name) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Copies the bundle's `mod-cache` into the game home beside it first and swaps it in whole, so
/// the old cache is removed only once the new one is complete.
fn seed_home_cache(snapshot: &Path, home: &Path) -> Result<(), String> {
    let source = snapshot.join("mod-cache");
    if !source.is_dir() {
        return Ok(());
    }
    let target = mod_content(home);
    let staging = target.with_file_name("mod-content.new");
    let old = target.with_file_name("mod-content.old");
    for leftover in [&staging, &old] {
        if leftover.exists() {
            fs::remove_dir_all(leftover).map_err(err)?;
        }
    }
    let staged = backend::copy_dir(&source, &staging)
        .and_then(|_| fs::write(staging.join(CACHE_COMPLETE), b""));
    if let Err(why) = staged {
        let _ = fs::remove_dir_all(&staging);
        return Err(why.to_string());
    }
    let had_old = target.exists();
    if had_old {
        fs::rename(&target, &old).map_err(err)?;
    }
    if let Err(why) = fs::rename(&staging, &target) {
        if had_old {
            let _ = fs::rename(&old, &target);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(why.to_string());
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

fn check_file(a: &Artifact, path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(err)?;
    if file.metadata().map_err(err)?.len() != a.size {
        return Err(format!("{} has the wrong size", a.name));
    }
    let mut hash = signing::Sha256::default();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(err)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    signing::check(&a.name, &a.sha256, &hash.hex())
}

/// A standard public Windows release zip, flat or with one enclosing folder. Never prepared data.
fn unpack_client(path: &Path, target: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(File::open(path).map_err(err)?).map_err(err)?;
    if zip.len() > 100_000 {
        return Err("Client archive has too many entries".into());
    }
    let roots: Vec<_> = zip
        .file_names()
        .filter_map(|name| {
            name.strip_suffix("research-lock.json")
                .filter(|prefix| {
                    prefix.is_empty() || (prefix.ends_with('/') && prefix.matches('/').count() == 1)
                })
                .map(str::to_owned)
        })
        .collect();
    let [prefix] = roots.as_slice() else {
        return Err("Client archive needs one research-lock.json at its root".into());
    };
    if !prefix.is_empty() && !safe_name(prefix.trim_end_matches('/')) {
        return Err("Unsafe client archive root".into());
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(err)?;
        let name = entry
            .name()
            .strip_prefix(prefix.as_str())
            .ok_or("Client archive has multiple roots")?
            .trim_end_matches('/')
            .to_owned();
        if name.is_empty() && entry.is_dir() {
            continue;
        }
        let parts: Vec<_> = name.split('/').collect();
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        // Java's nested exporter classes use '$', which is safe in a Windows filename.
        if !parts.iter().all(|p| safe_name(&p.replace('$', "_")))
            || parts
                .iter()
                .any(|p| p.eq_ignore_ascii_case("assets") || p.eq_ignore_ascii_case("minecraft"))
            || name.to_ascii_lowercase().ends_with(".jar")
            || !matches!(mode, 0 | 0o100000 | 0o040000)
            || !seen.insert(name.to_ascii_lowercase())
        {
            return Err(format!("Unsafe or non-public client archive entry: {name}"));
        }
        total = total.saturating_add(entry.size());
        if total > PAYLOAD_LIMIT || entry.size() > (1 << 30) {
            return Err("Client archive exceeds extraction limits".into());
        }
        let to = target.join(&name);
        if entry.is_dir() {
            fs::create_dir_all(to).map_err(err)?;
        } else {
            fs::create_dir_all(to.parent().ok_or("Invalid archive path")?).map_err(err)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(to)
                .map_err(err)?;
            let expected = entry.size();
            let mut body = entry.take(expected + 1);
            if std::io::copy(&mut body, &mut file).map_err(err)? != expected {
                return Err(format!("Client archive entry has the wrong size: {name}"));
            }
        }
    }
    if !target.join("horizon-client.exe").is_file() || !target.join("horizon-prepare.exe").is_file()
    {
        return Err("Client archive needs horizon-client.exe and horizon-prepare.exe".into());
    }
    Ok(())
}

fn validate_mods(paths: &[PathBuf]) -> Result<(), String> {
    if paths.len() > 512 {
        return Err("Friends supports at most 512 content mods".into());
    }
    let mut namespaces = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for from in paths {
        let package = horizon_mods::Package::read(from)?;
        let manifest = &package.manifest;
        if package.wasm.is_some() || !ids.insert(manifest.id.clone()) {
            return Err(
                "Friends accepts only distinct content mods, without executable code".into(),
            );
        }
        let content = horizon_mods::Content::read(from, manifest)?;
        if content
            .namespaces
            .iter()
            .any(|n| !namespaces.insert(n.clone()))
        {
            return Err("Friends content mods share a namespace".into());
        }
    }
    Ok(())
}

fn file_digest(path: &Path) -> Result<Option<String>, String> {
    let Ok(mut file) = File::open(path) else {
        return Ok(None);
    };
    let mut hash = signing::Sha256::default();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(err)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(Some(hash.hex()))
}

fn same_client_bin(old_client: &Path, new_client: &Path) -> bool {
    for rel in [
        "research-lock.json",
        "horizon-client.exe",
        "horizon-prepare.exe",
    ] {
        let Ok(Some(old_hex)) = file_digest(&old_client.join(rel)) else {
            return false;
        };
        let Ok(Some(new_hex)) = file_digest(&new_client.join(rel)) else {
            return false;
        };
        if old_hex != new_hex {
            return false;
        }
    }
    true
}

fn strip_bundle_prefix(names: &[String]) -> Result<String, String> {
    let mut roots = Vec::new();
    for raw in names {
        let cleaned = raw.strip_prefix("./").unwrap_or(raw);
        if let Some(prefix) = cleaned.strip_suffix("client/research-lock.json") {
            if prefix.is_empty() || (prefix.ends_with('/') && prefix.matches('/').count() == 1) {
                roots.push(prefix.to_owned());
            }
        }
    }
    let [prefix] = roots.as_slice() else {
        return Err("Bundle archive needs one client/research-lock.json at its root".into());
    };
    if !prefix.is_empty() && !safe_name(prefix.trim_end_matches('/')) {
        return Err("Unsafe bundle archive root".into());
    }
    Ok(prefix.clone())
}

fn write_bundle_entry(
    raw_name: &str,
    is_dir: bool,
    regular_or_dir: bool,
    size: u64,
    reader: &mut dyn Read,
    prefix: &str,
    snapshot: &Path,
    seen: &mut BTreeSet<String>,
    total: &mut u64,
) -> Result<(), String> {
    if is_dir && matches!(raw_name, "." | "./" | "") {
        return Ok(());
    }
    let cleaned = raw_name.strip_prefix("./").unwrap_or(raw_name);
    if cleaned.is_empty()
        || (prefix == cleaned)
        || (!prefix.is_empty() && cleaned == prefix.trim_end_matches('/'))
    {
        return Ok(());
    }
    let name = cleaned
        .strip_prefix(prefix)
        .ok_or("Bundle archive has multiple roots")?
        .trim_end_matches('/');
    if name.is_empty() && is_dir {
        return Ok(());
    }
    let parts: Vec<&str> = name.split('/').collect();
    if !parts.iter().all(|p| safe_name(&p.replace('$', "_")))
        || !regular_or_dir
        || !seen.insert(name.to_ascii_lowercase())
    {
        return Err(format!("Unsafe or duplicate bundle entry: {name}"));
    }
    let lower = name.to_ascii_lowercase();
    let allowed = match parts.first().copied() {
        Some("client") => {
            !parts[1..]
                .iter()
                .any(|p| p.eq_ignore_ascii_case("assets") || p.eq_ignore_ascii_case("minecraft"))
                && !lower.ends_with(".jar")
        }
        Some("mods") => {
            (is_dir && parts.len() == 1)
                || (!is_dir && parts.len() == 2 && parts[1].ends_with(".hmod"))
        }
        Some("tables") => {
            (is_dir && parts.len() == 1)
                || (!is_dir
                    && parts.len() == 2
                    && matches!(
                        parts[1],
                        "server-states.tsv"
                            | "server-states.registries.tsv"
                            | "server-states.codecs.tsv"
                    ))
        }
        Some("mod-cache") => {
            // Textures derived from Minecraft's assets are built on the friend's own PC.
            // So is vanilla's own data and the HUD atlas (Minecraft's GUI pixels).
            !parts[1..].windows(2).any(|w| {
                w[0].eq_ignore_ascii_case(".horizon-derived")
                    && (w[1].to_ascii_lowercase().starts_with("prepared-textures")
                        || w[1].to_ascii_lowercase().starts_with("baked-render")
                        || w[1].to_ascii_lowercase().starts_with("hud-atlas"))
            }) && !lower.ends_with(".jar")
                && !parts[1..].last().is_some_and(|p| {
                    p.eq_ignore_ascii_case("horizon-resources.json")
                        || (parts.len() == 2 && p.eq_ignore_ascii_case("pack.png"))
                })
                && !parts[1..].windows(2).any(|w| {
                    (w[0].eq_ignore_ascii_case("assets") || w[0].eq_ignore_ascii_case("data"))
                        && w[1].eq_ignore_ascii_case("minecraft")
                })
        }
        _ => false,
    };
    if !allowed {
        return Err(format!("Unsafe or unexpected bundle entry: {name}"));
    }
    *total = total.saturating_add(size);
    if *total > PAYLOAD_LIMIT || size > (1 << 30) {
        return Err("Bundle archive exceeds extraction limits".into());
    }
    let to = snapshot.join(name);
    if is_dir {
        fs::create_dir_all(to).map_err(err)?;
    } else {
        fs::create_dir_all(to.parent().ok_or("Invalid bundle path")?).map_err(err)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)
            .map_err(err)?;
        let mut body = reader.take(size + 1);
        if std::io::copy(&mut body, &mut file).map_err(err)? != size {
            return Err(format!("Bundle entry has the wrong size: {name}"));
        }
    }
    Ok(())
}

fn unpack_bundle(path: &Path, snapshot: &Path) -> Result<(), String> {
    let name_str = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    if name_str.ends_with(".tar.gz") || name_str.ends_with(".tgz") {
        let mut names = Vec::new();
        {
            let mut archive = TarArchive::new(GzDecoder::new(File::open(path).map_err(err)?));
            for entry in archive.entries().map_err(err)? {
                let entry = entry.map_err(err)?;
                let entry_path = entry
                    .path()
                    .map_err(err)?
                    .to_str()
                    .ok_or("Bundle path is not UTF-8")?
                    .to_owned();
                names.push(entry_path);
                if names.len() > 100_000 {
                    return Err("Bundle archive has too many entries".into());
                }
            }
        }
        let prefix = strip_bundle_prefix(&names)?;
        let mut archive = TarArchive::new(GzDecoder::new(File::open(path).map_err(err)?));
        for entry in archive.entries().map_err(err)? {
            let mut entry = entry.map_err(err)?;
            let raw_name = entry
                .path()
                .map_err(err)?
                .to_str()
                .ok_or("Bundle path is not UTF-8")?
                .to_owned();
            let entry_type = entry.header().entry_type();
            let is_dir = entry_type.is_dir();
            let regular_or_dir = is_dir || entry_type == EntryType::Regular;
            let size = if is_dir {
                0
            } else {
                entry.header().size().map_err(err)?
            };
            write_bundle_entry(
                &raw_name,
                is_dir,
                regular_or_dir,
                size,
                &mut entry,
                &prefix,
                snapshot,
                &mut seen,
                &mut total,
            )?;
        }
    } else {
        let mut zip = zip::ZipArchive::new(File::open(path).map_err(err)?).map_err(err)?;
        if zip.len() > 100_000 {
            return Err("Bundle archive has too many entries".into());
        }
        let names: Vec<String> = zip.file_names().map(str::to_owned).collect();
        let prefix = strip_bundle_prefix(&names)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(err)?;
            let raw_name = entry.name().to_owned();
            let is_dir = entry.is_dir();
            let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
            let regular_or_dir = matches!(mode, 0 | 0o100000 | 0o040000);
            let size = entry.size();
            write_bundle_entry(
                &raw_name,
                is_dir,
                regular_or_dir,
                size,
                &mut entry,
                &prefix,
                snapshot,
                &mut seen,
                &mut total,
            )?;
        }
    }
    let client = snapshot.join("client");
    if !client.join("horizon-client.exe").is_file() || !client.join("horizon-prepare.exe").is_file()
    {
        return Err(
            "Bundle archive needs client/horizon-client.exe and client/horizon-prepare.exe".into(),
        );
    }
    for table in [
        "server-states.tsv",
        "server-states.registries.tsv",
        "server-states.codecs.tsv",
    ] {
        if !snapshot.join("tables").join(table).is_file() {
            return Err(format!("Bundle archive is missing tables/{table}"));
        }
    }
    let mods = sorted_hmods(&snapshot.join("mods"));
    validate_mods(&mods)?;
    Ok(())
}

#[cfg(test)]
mod tests;
