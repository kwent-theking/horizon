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

const MANIFEST: &str = "friends.json";
const METADATA_LIMIT: u64 = 1 << 20;
const PAYLOAD_LIMIT: u64 = 4 << 30;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub repository: String,
    pub channel: String,
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

    fn keys(&self) -> Result<Vec<PublicKey>, String> {
        let parts: Vec<_> = self.repository.split('/').collect();
        if parts.len() != 2 || !parts.iter().all(|p| safe_name(p)) || !safe_name(&self.channel) {
            return Err(
                "Friends needs an owner/repository and a simple release channel tag".into(),
            );
        }
        let keys = signing::parse_keys(&self.public_keys)?;
        if keys.is_empty() {
            return Err("Friends needs the owner's public signing key in its local config".into());
        }
        Ok(keys)
    }

    fn scope(&self) -> String {
        format!("friends:{}:{}", self.repository, self.channel)
    }

    fn url(&self, name: &str) -> String {
        format!(
            "https://github.com/{}/releases/download/{}/{name}",
            self.repository, self.channel
        )
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
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    /// Increases for every change, including a rollback to older client code.
    pub sequence: u64,
    pub minecraft: String,
    pub server: String,
    pub client: Artifact,
    pub mods: Vec<Artifact>,
    pub states: Artifact,
    pub registries: Artifact,
    pub codecs: Artifact,
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
        if self.client.name != "horizon-windows-x64.zip"
            || self.mods.len() > 512
            || self.mods.iter().any(|m| !m.name.ends_with(".hmod"))
            || !self.states.name.ends_with(".tsv")
            || !self.registries.name.ends_with(".registries.tsv")
            || !self.codecs.name.ends_with(".codecs.tsv")
        {
            return Err(
                "Friends needs a Windows x64 client, content mods and three TSV tables".into(),
            );
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

    fn artifacts(&self) -> impl Iterator<Item = &Artifact> {
        [&self.client, &self.states, &self.registries, &self.codecs]
            .into_iter()
            .chain(&self.mods)
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
        && name.as_bytes()[0].is_ascii_alphanumeric()
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
        let mut file = OpenOptions::new().write(true).create_new(true).open(&path)
            .map_err(|e| format!("{}: {e}. Close the other friends launcher; after a crash, remove this lock only when it is stopped.", path.display()))?;
        let lock = Self(path);
        writeln!(file, "{}", std::process::id()).map_err(err)?;
        Ok(lock)
    }
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
    /// Builds the command without starting a process. Mods are explicit, so removed mods stay out.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(self.snapshot.join("client/horizon-client.exe"));
        cmd.current_dir(&self.home)
            .env("HORIZON_HOME", &self.home)
            .env(
                "HORIZON_SERVER_STATES",
                self.snapshot.join("tables/server-states.tsv"),
            )
            .arg("--resources")
            .arg(backend::resources_at(&self.snapshot.join("client")))
            .arg("--server")
            .arg(&self.manifest.server);
        for m in &self.manifest.mods {
            cmd.arg("--dev-mod")
                .arg(self.snapshot.join("mods").join(&m.name));
        }
        cmd
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

/// Check the opted-in channel, download/verify its snapshot, then prepare from this player's install.
/// The default launcher never calls this and its upstream source and self-update policy stay intact.
pub fn update(config: &Config, root: &Path, log: &dyn Fn(String)) -> Result<Ready, String> {
    if !cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        return Err("The friends MVP supports Windows x64 only".into());
    }
    let minecraft = backend::minecraft_dir(config.minecraft.clone())?;
    update_with(
        config,
        root,
        &metadata,
        &|url, a, to| {
            log(format!("Downloading {}", a.name));
            // Read at most the signed size plus one byte, then check_file hashes the staged file.
            // A host serving a larger body cannot fill the disk before the size check rejects it.
            let mut body = backend::get(url)?.into_reader().take(a.size + 1);
            let mut file = File::create(to).map_err(err)?;
            std::io::copy(&mut body, &mut file).map_err(err)?;
            Ok(())
        },
        &|dir| prepare(dir, &minecraft, log),
    )
}

fn metadata(url: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    backend::get(url)?
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
        backend::preparer_at(dir, minecraft, backend::MinecraftFrom::LocalInstall, log)?;
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
                Ok((old.client.sha256 == m.client.sha256
                    && old.client.size == m.client.size
                    && old.minecraft == m.minecraft
                    && backend::resources_at(&dir)
                        .join("horizon-resources.json")
                        .is_file())
                .then_some(dir))
            })
            .transpose()?
            .flatten();
        for a in m.artifacts() {
            if a.name == m.client.name && reusable.is_some() {
                continue;
            }
            let to = snapshot.join("downloads").join(&a.name);
            download(&config.url(&a.name), a, &to)?;
            check_file(a, &to)?;
        }
        match &reusable {
            Some(from) => backend::copy_dir(from, &snapshot.join("client")).map_err(err)?,
            None => unpack_client(
                &snapshot.join("downloads").join(&m.client.name),
                &snapshot.join("client"),
            )?,
        }
        let pins = horizon_prepare::artifacts::Lock::read(&snapshot.join("client"))?;
        if pins.version != m.minecraft {
            return Err("Friends manifest and client pin different Minecraft versions".into());
        }
        let mut namespaces = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for a in &m.mods {
            let from = snapshot.join("downloads").join(&a.name);
            let package = horizon_mods::Package::read(&from)?;
            let manifest = &package.manifest;
            if package.wasm.is_some() || !ids.insert(manifest.id.clone()) {
                return Err(
                    "Friends accepts only distinct content mods, without executable code".into(),
                );
            }
            let content = horizon_mods::Content::read(&from, manifest)?;
            if content
                .namespaces
                .iter()
                .any(|n| !namespaces.insert(n.clone()))
            {
                return Err("Friends content mods share a namespace".into());
            }
            fs::rename(from, snapshot.join("mods").join(&a.name)).map_err(err)?;
        }
        for (a, name) in [
            (&m.states, "server-states.tsv"),
            (&m.registries, "server-states.registries.tsv"),
            (&m.codecs, "server-states.codecs.tsv"),
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
    }
    Ok(Ready {
        manifest: m,
        snapshot,
        home,
        _lock: lock,
    })
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

#[cfg(test)]
mod tests;
