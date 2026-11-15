use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "horizon-friends-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

const PINS: &[u8] = br#"{"minecraft":{"version":"26.3","java_major":25}}"#;

fn client(marker: &[u8]) -> Vec<u8> {
    archive(&[
        ("horizon/research-lock.json", PINS),
        ("horizon/horizon-client.exe", marker),
        ("horizon/horizon-prepare.exe", b"prepare"),
    ])
}

fn prepared(dir: &Path) -> Result<(), String> {
    let resources = backend::resources_at(dir);
    fs::create_dir_all(&resources).map_err(err)?;
    fs::write(resources.join("horizon-resources.json"), b"{}").map_err(err)
}

struct Release {
    config: Config,
    key: signing::SigningKey,
    manifest: Value,
    assets: BTreeMap<String, Vec<u8>>,
}

impl Release {
    fn new() -> Self {
        let (key, _) = signing::SigningKey::generate().unwrap();
        let config = Config {
            repository: "kwent-theking/horizon".into(),
            channel: "friends".into(),
            public_keys: STANDARD.encode(key.public()),
            minecraft: Some(PathBuf::from("unused-own-install")),
        };
        let content = horizon_mods::Package::write_files(
            "id = 'create'\nname = 'Create'\nversion = '1.0'\napi = '0.2'\ncapabilities = ['registries']\n",
            None, &[],
        ).unwrap();
        let assets: BTreeMap<String, Vec<u8>> = BTreeMap::from([
            ("horizon-windows-x64.zip".into(), client(b"first")),
            ("create.hmod".into(), content),
            ("server-states.tsv".into(), b"0\tminecraft:air\t\n".to_vec()),
            (
                "server-states.registries.tsv".into(),
                b"minecraft:item\t0\tminecraft:air\n".to_vec(),
            ),
            (
                "server-states.codecs.tsv".into(),
                b"minecraft:particle_type\t0\tminecraft:test\t\"unit\"\n".to_vec(),
            ),
        ]);
        let artifact = |name: &str| {
            json!({"name": name, "size": assets[name].len(),
            "sha256": signing::sha256_hex(&assets[name])})
        };
        let manifest = json!({"schema":1, "sequence":1, "minecraft":"26.3", "server":"play.example:25565",
            "client":artifact("horizon-windows-x64.zip"), "mods":[artifact("create.hmod")],
            "states":artifact("server-states.tsv"), "registries":artifact("server-states.registries.tsv"),
            "codecs":artifact("server-states.codecs.tsv")});
        Self {
            config,
            key,
            manifest,
            assets,
        }
    }

    fn metadata(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let bytes = serde_json::to_vec(&self.manifest).unwrap();
        let sums = signing::sums_file(&[(MANIFEST.into(), signing::sha256_hex(&bytes))]);
        let signature = self.key.sign(&self.config.scope(), sums.as_bytes());
        (bytes, sums.into_bytes(), signature.into_bytes())
    }

    fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        assert!(
            url.starts_with("https://github.com/kwent-theking/horizon/releases/download/friends/")
        );
        let (bytes, sums, sig) = self.metadata();
        match url.rsplit('/').next().unwrap() {
            MANIFEST => Ok(bytes),
            signing::SUMS => Ok(sums),
            signing::SIGNATURE => Ok(sig),
            _ => Err("Unexpected URL".into()),
        }
    }

    fn download(&self, url: &str, a: &Artifact, to: &Path) -> Result<(), String> {
        assert_eq!(url, self.config.url(&a.name));
        fs::write(to, &self.assets[&a.name]).map_err(err)
    }

    fn install(&self, root: &Path) -> Result<Ready, String> {
        update_with(
            &self.config,
            root,
            &|u| self.fetch(u),
            &|u, a, to| self.download(u, a, to),
            &prepared,
        )
    }

    fn new_client(&mut self) {
        let bytes = client(b"second");
        self.manifest["client"]["sha256"] = json!(signing::sha256_hex(&bytes));
        self.manifest["client"]["size"] = json!(bytes.len());
        self.assets.insert("horizon-windows-x64.zip".into(), bytes);
    }
}

#[test]
fn signature_binds_owner_repository_channel_and_manifest_bytes() {
    let release = Release::new();
    let (mut bytes, sums, signature) = release.metadata();
    assert!(authenticated_manifest(&release.config, &bytes, &sums, &signature).is_ok());
    let mut config = release.config;
    config.repository = "other/horizon".into();
    assert!(authenticated_manifest(&config, &bytes, &sums, &signature).is_err());
    config.repository = "kwent-theking/horizon".into();
    config.channel = "other".into();
    assert!(authenticated_manifest(&config, &bytes, &sums, &signature).is_err());
    config.channel = "friends".into();
    let (other, _) = signing::SigningKey::generate().unwrap();
    config.public_keys = STANDARD.encode(other.public());
    assert!(authenticated_manifest(&config, &bytes, &sums, &signature).is_err());
    config.public_keys = STANDARD.encode(release.key.public());
    bytes.push(b' ');
    assert!(authenticated_manifest(&config, &bytes, &sums, &signature).is_err());
    assert!(authenticated_manifest(&config, &bytes, &sums, b"").is_err());
}

#[test]
fn duplicate_checksum_entries_are_refused_even_when_signed() {
    let release = Release::new();
    let (bytes, sums, _) = release.metadata();
    let sums = [sums.clone(), sums].concat();
    let signature = release.key.sign(&release.config.scope(), &sums);
    assert!(authenticated_manifest(&release.config, &bytes, &sums, signature.as_bytes()).is_err());
}

#[test]
fn missing_local_trust_and_oversize_metadata_fail_before_payload_downloads() {
    let scratch = Scratch::new();
    let mut release = Release::new();
    release.config.public_keys.clear();
    assert!(update_with(
        &release.config,
        &scratch.0,
        &|_| panic!("no network without a local trust root"),
        &|_, _, _| panic!("no payload"),
        &prepared
    )
    .is_err());
    release.config.public_keys = STANDARD.encode(release.key.public());
    let (_, sums, sig) = release.metadata();
    assert!(authenticated_manifest(
        &release.config,
        &vec![0; METADATA_LIMIT as usize + 1],
        &sums,
        &sig
    )
    .is_err());
}

#[test]
fn config_discovers_minecraft_by_default_and_resolves_an_override_beside_itself() {
    let scratch = Scratch::new();
    let release = Release::new();
    let mut config = json!({"repository":"kwent-theking/horizon", "channel":"friends",
        "public_keys":release.config.public_keys});
    let path = scratch.0.join("friends-launcher.json");
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert!(Config::read(&path).unwrap().minecraft.is_none());
    config["minecraft"] = json!("own-minecraft");
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert_eq!(
        Config::read(&path).unwrap().minecraft,
        Some(scratch.0.join("own-minecraft"))
    );
}

#[test]
fn manifest_rejects_unknown_schemas_unsafe_names_and_incomplete_sets() {
    let release = Release::new();
    for (field, bad) in [
        ("schema", json!(2)),
        ("sequence", json!(0)),
        ("server", json!("--world")),
        ("minecraft", json!("../26.3")),
    ] {
        let mut value = release.manifest.clone();
        value[field] = bad;
        assert!(serde_json::from_value::<Manifest>(value)
            .unwrap()
            .validate()
            .is_err());
    }
    for name in [
        "../evil.hmod",
        "C:evil.hmod",
        "evil\\a.hmod",
        "CON.hmod",
        "NUL.hmod",
        "thing.jar",
    ] {
        let mut value = release.manifest.clone();
        value["mods"][0]["name"] = json!(name);
        assert!(
            serde_json::from_value::<Manifest>(value)
                .unwrap()
                .validate()
                .is_err(),
            "{name}"
        );
    }
    let mut value = release.manifest.clone();
    value["mods"]
        .as_array_mut()
        .unwrap()
        .push(release.manifest["mods"][0].clone());
    assert!(serde_json::from_value::<Manifest>(value)
        .unwrap()
        .validate()
        .is_err());
    let mut value = release.manifest.clone();
    value["client"]["size"] = json!(PAYLOAD_LIMIT + 1);
    assert!(serde_json::from_value::<Manifest>(value)
        .unwrap()
        .validate()
        .is_err());
    let mut value = release.manifest.clone();
    value.as_object_mut().unwrap().remove("codecs");
    assert!(serde_json::from_value::<Manifest>(value).is_err());
}

#[test]
fn snapshot_command_has_its_own_home_and_all_three_sibling_tables() {
    let scratch = Scratch::new();
    let release = Release::new();
    let ready = release.install(&scratch.0).unwrap();
    let cmd = ready.command(); // Inspects a command only; never starts a game or a server.
    assert_eq!(cmd.get_current_dir(), Some(ready.home.as_path()));
    assert_eq!(
        cmd.get_program(),
        ready.snapshot.join("client/horizon-client.exe").as_os_str()
    );
    let args: Vec<_> = cmd
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[2..4], ["--server", "play.example:25565"]);
    assert_eq!(args[4], "--dev-mod");
    assert!(args[5].ends_with("create.hmod"));
    let env: BTreeMap<_, _> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.unwrap().to_string_lossy().into_owned(),
            )
        })
        .collect();
    assert_eq!(env["HORIZON_HOME"], ready.home.to_string_lossy());
    let states = PathBuf::from(&env["HORIZON_SERVER_STATES"]);
    for path in [
        states.clone(),
        states.with_extension("registries.tsv"),
        states.with_extension("codecs.tsv"),
    ] {
        assert!(path.is_file());
    }
}

#[test]
fn same_manifest_is_a_noop_and_play_holds_an_exclusive_lock() {
    let scratch = Scratch::new();
    let release = Release::new();
    let ready = release.install(&scratch.0).unwrap();
    assert!(release.install(&scratch.0).is_err());
    let snapshot = ready.snapshot.clone();
    drop(ready);
    let again = update_with(
        &release.config,
        &scratch.0,
        &|u| release.fetch(u),
        &|_, _, _| panic!("no download needed"),
        &|_| panic!("no preparation needed"),
    )
    .unwrap();
    assert_eq!(snapshot, again.snapshot);
}

#[test]
fn updates_preserve_player_data_reuse_own_resources_and_remove_old_mods_from_command() {
    let scratch = Scratch::new();
    let mut release = Release::new();
    let ready = release.install(&scratch.0).unwrap();
    fs::write(ready.home.join("options.txt"), b"player choices").unwrap();
    fs::create_dir(ready.home.join("saves")).unwrap();
    fs::write(ready.home.join("saves/world"), b"player world").unwrap();
    let old_snapshot = ready.snapshot.clone();
    drop(ready);
    release.manifest["sequence"] = json!(2);
    release.manifest["mods"] = json!([]);
    let ready = update_with(
        &release.config,
        &scratch.0,
        &|u| release.fetch(u),
        &|u, a, to| {
            assert_ne!(a.name, "horizon-windows-x64.zip");
            release.download(u, a, to)
        },
        &|_| panic!("same client reuses resources prepared locally"),
    )
    .unwrap();
    assert_eq!(
        fs::read(ready.home.join("options.txt")).unwrap(),
        b"player choices"
    );
    assert_eq!(
        fs::read(ready.home.join("saves/world")).unwrap(),
        b"player world"
    );
    assert!(old_snapshot.join("mods/create.hmod").exists());
    assert!(!ready.command().get_args().any(|a| a == "--dev-mod"));
}

#[test]
fn rollback_and_same_sequence_equivocation_are_refused() {
    let scratch = Scratch::new();
    let mut release = Release::new();
    release.manifest["sequence"] = json!(2);
    drop(release.install(&scratch.0).unwrap());
    release.manifest["sequence"] = json!(1);
    assert!(release.install(&scratch.0).err().unwrap().contains("older"));
    release.manifest["sequence"] = json!(2);
    release.manifest["server"] = json!("changed.example");
    assert!(release
        .install(&scratch.0)
        .err()
        .unwrap()
        .contains("changed"));
}

#[test]
fn download_or_prepare_failure_keeps_previous_accepted_snapshot_and_can_retry() {
    let scratch = Scratch::new();
    let mut release = Release::new();
    let ready = release.install(&scratch.0).unwrap();
    let root = ready.home.parent().unwrap().to_path_buf();
    let old = current(&root).unwrap();
    let old_snapshot = ready.snapshot.clone();
    drop(ready);
    release.manifest["sequence"] = json!(2);
    release.new_client();
    assert!(update_with(
        &release.config,
        &scratch.0,
        &|u| release.fetch(u),
        &|_, _, _| Err("connection interrupted".into()),
        &prepared
    )
    .is_err());
    assert_eq!(current(&root).unwrap(), old);
    assert!(update_with(
        &release.config,
        &scratch.0,
        &|u| release.fetch(u),
        &|u, a, to| release.download(u, a, to),
        &|_| Err("Java missing".into())
    )
    .is_err());
    assert_eq!(current(&root).unwrap(), old);
    assert!(old_snapshot.join("client/horizon-client.exe").is_file());
    drop(release.install(&scratch.0).unwrap());
    assert_eq!(current(&root).unwrap().unwrap().0, 2);
}

#[test]
fn corrupted_download_is_never_extracted_or_prepared() {
    let scratch = Scratch::new();
    let release = Release::new();
    assert!(update_with(
        &release.config,
        &scratch.0,
        &|u| release.fetch(u),
        &|_, a, to| fs::write(to, vec![b'x'; a.size as usize]).map_err(err),
        &|_| panic!("corrupted payload must not be prepared")
    )
    .is_err());
}

#[test]
fn incompatible_client_and_code_mods_cannot_become_accepted() {
    let scratch = Scratch::new();
    let mut release = Release::new();
    release.manifest["minecraft"] = json!("1.21.1");
    assert!(release
        .install(&scratch.0)
        .err()
        .unwrap()
        .contains("different Minecraft"));
    release.manifest["minecraft"] = json!("26.3");
    let code = horizon_mods::Package::write(
        "id = 'create'\nname = 'Create'\nversion = '1.0'\napi = '0.1'\n",
        b"code",
    )
    .unwrap();
    release.manifest["mods"][0]["size"] = json!(code.len());
    release.manifest["mods"][0]["sha256"] = json!(signing::sha256_hex(&code));
    release.assets.insert("create.hmod".into(), code);
    assert!(release
        .install(&scratch.0)
        .err()
        .unwrap()
        .contains("without executable code"));
}

#[test]
fn preparation_must_finish_before_acceptance_and_repair_is_retryable() {
    let scratch = Scratch::new();
    let release = Release::new();
    assert!(update_with(
        &release.config,
        &scratch.0,
        &|u| release.fetch(u),
        &|u, a, to| release.download(u, a, to),
        &|_| Ok(())
    )
    .is_err());
    let ready = release.install(&scratch.0).unwrap();
    let marker =
        backend::resources_at(&ready.snapshot.join("client")).join("horizon-resources.json");
    let snapshot = ready.snapshot.clone();
    drop(ready);
    fs::remove_file(marker).unwrap();
    let repaired = release.install(&scratch.0).unwrap();
    assert_eq!(repaired.snapshot, snapshot);
}

#[test]
fn public_client_archive_allows_original_nested_java_classes() {
    let scratch = Scratch::new();
    let path = scratch.0.join("public.zip");
    fs::write(
        &path,
        archive(&[
            ("research-lock.json", PINS),
            ("horizon-client.exe", b"client"),
            ("horizon-prepare.exe", b"prepare"),
            (
                "exporters/client/dev/horizonlab/parity/PrepareWorker$Handler.class",
                b"own class",
            ),
        ]),
    )
    .unwrap();
    let target = scratch.0.join("unpacked");
    unpack_client(&path, &target).unwrap();
    assert!(target
        .join("exporters/client/dev/horizonlab/parity/PrepareWorker$Handler.class")
        .is_file());
}

#[test]
fn archive_rejects_traversal_links_duplicates_and_redistributed_minecraft_data() {
    for name in [
        "../escape",
        "C:/escape",
        "a\\b",
        ".local/secret",
        "assets/minecraft/a.png",
        "client.jar",
        "CON.txt",
        "HORIZON-CLIENT.EXE",
    ] {
        let scratch = Scratch::new();
        let zip = archive(&[
            ("research-lock.json", PINS),
            ("horizon-client.exe", b"client"),
            ("horizon-prepare.exe", b"prepare"),
            (name, b"bad"),
        ]);
        let path = scratch.0.join("client.zip");
        fs::write(&path, zip).unwrap();
        assert!(
            unpack_client(&path, &scratch.0.join("unpacked")).is_err(),
            "{name}"
        );
    }
    let scratch = Scratch::new();
    let path = scratch.0.join("link.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    zip.start_file(
        "research-lock.json",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(PINS).unwrap();
    zip.add_symlink(
        "escape",
        "../outside",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.finish().unwrap();
    assert!(unpack_client(&path, &scratch.0.join("unpacked")).is_err());
}
