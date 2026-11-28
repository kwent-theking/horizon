use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
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
            url: None,
            token: None,
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
        let name = url.rsplit('/').next().unwrap();
        assert_eq!(url, self.config.url(name));
        let (bytes, sums, sig) = self.metadata();
        match name {
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
    assert!(
        update_with(
            &release.config,
            &scratch.0,
            &|_| panic!("no network without a local trust root"),
            &|_, _, _| panic!("no payload"),
            &prepared
        )
        .is_err()
    );
    release.config.public_keys = STANDARD.encode(release.key.public());
    let (_, sums, sig) = release.metadata();
    assert!(
        authenticated_manifest(
            &release.config,
            &vec![0; METADATA_LIMIT as usize + 1],
            &sums,
            &sig
        )
        .is_err()
    );
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
        assert!(
            serde_json::from_value::<Manifest>(value)
                .unwrap()
                .validate()
                .is_err()
        );
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
    assert!(
        serde_json::from_value::<Manifest>(value)
            .unwrap()
            .validate()
            .is_err()
    );
    let mut value = release.manifest.clone();
    value["client"]["size"] = json!(PAYLOAD_LIMIT + 1);
    assert!(
        serde_json::from_value::<Manifest>(value)
            .unwrap()
            .validate()
            .is_err()
    );
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
    assert!(
        release
            .install(&scratch.0)
            .err()
            .unwrap()
            .contains("changed")
    );
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
    assert!(
        update_with(
            &release.config,
            &scratch.0,
            &|u| release.fetch(u),
            &|_, _, _| Err("connection interrupted".into()),
            &prepared
        )
        .is_err()
    );
    assert_eq!(current(&root).unwrap(), old);
    assert!(
        update_with(
            &release.config,
            &scratch.0,
            &|u| release.fetch(u),
            &|u, a, to| release.download(u, a, to),
            &|_| Err("Java missing".into())
        )
        .is_err()
    );
    assert_eq!(current(&root).unwrap(), old);
    assert!(old_snapshot.join("client/horizon-client.exe").is_file());
    drop(release.install(&scratch.0).unwrap());
    assert_eq!(current(&root).unwrap().unwrap().0, 2);
}

#[test]
fn corrupted_download_is_never_extracted_or_prepared() {
    let scratch = Scratch::new();
    let release = Release::new();
    assert!(
        update_with(
            &release.config,
            &scratch.0,
            &|u| release.fetch(u),
            &|_, a, to| fs::write(to, vec![b'x'; a.size as usize]).map_err(err),
            &|_| panic!("corrupted payload must not be prepared")
        )
        .is_err()
    );
}

#[test]
fn incompatible_client_and_code_mods_cannot_become_accepted() {
    let scratch = Scratch::new();
    let mut release = Release::new();
    release.manifest["minecraft"] = json!("1.21.1");
    assert!(
        release
            .install(&scratch.0)
            .err()
            .unwrap()
            .contains("different Minecraft")
    );
    release.manifest["minecraft"] = json!("26.3");
    let code = horizon_mods::Package::write(
        "id = 'create'\nname = 'Create'\nversion = '1.0'\napi = '0.1'\n",
        b"code",
    )
    .unwrap();
    release.manifest["mods"][0]["size"] = json!(code.len());
    release.manifest["mods"][0]["sha256"] = json!(signing::sha256_hex(&code));
    release.assets.insert("create.hmod".into(), code);
    assert!(
        release
            .install(&scratch.0)
            .err()
            .unwrap()
            .contains("without executable code")
    );
}

#[test]
fn preparation_must_finish_before_acceptance_and_repair_is_retryable() {
    let scratch = Scratch::new();
    let release = Release::new();
    assert!(
        update_with(
            &release.config,
            &scratch.0,
            &|u| release.fetch(u),
            &|u, a, to| release.download(u, a, to),
            &|_| Ok(())
        )
        .is_err()
    );
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
    assert!(
        target
            .join("exporters/client/dev/horizonlab/parity/PrepareWorker$Handler.class")
            .is_file()
    );
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

fn tar_gz_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, *name, &bytes[..]).unwrap();
    }
    builder.finish().unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn vps_single_bundle_zip_and_tar_gz_install_seed_mod_cache_and_reuse_unchanged_client() {
    let scratch = Scratch::new();
    let mut release = Release::new();
    release.config.repository.clear();
    release.config.channel.clear();
    release.config.url = Some("https://packs.example.invalid/pack/".into());
    release.config.token = Some("vps-secret".into());

    let hmod = release.assets["create.hmod"].clone();
    let states = release.assets["server-states.tsv"].clone();
    let regs = release.assets["server-states.registries.tsv"].clone();
    let codecs = release.assets["server-states.codecs.tsv"].clone();

    let bundle_zip = archive(&[
        ("horizon-pack/client/research-lock.json", PINS),
        ("horizon-pack/client/horizon-client.exe", b"client-v1"),
        ("horizon-pack/client/horizon-prepare.exe", b"prep-v1"),
        ("horizon-pack/mods/create.hmod", &hmod),
        ("horizon-pack/tables/server-states.tsv", &states),
        ("horizon-pack/tables/server-states.registries.tsv", &regs),
        ("horizon-pack/tables/server-states.codecs.tsv", &codecs),
        (
            "horizon-pack/mod-cache/horizon-mod-content.stamp",
            b"stamp-v1",
        ),
        ("horizon-pack/mod-cache/assets/create/icon.png", b"icon"),
    ]);
    let bundle_name = "horizon-pack-v1.zip";
    release.assets.clear();
    release
        .assets
        .insert(bundle_name.into(), bundle_zip.clone());
    release.manifest = json!({
        "schema": 1,
        "sequence": 1,
        "minecraft": "26.3",
        "server": "play.example.invalid:25565",
        "bundle": {
            "name": bundle_name,
            "size": bundle_zip.len(),
            "sha256": signing::sha256_hex(&bundle_zip),
        }
    });

    assert_eq!(
        release.config.url("friends.json"),
        "https://packs.example.invalid/pack/friends.json"
    );
    let ready = release.install(&scratch.0).unwrap();
    assert!(ready.snapshot.join("client/horizon-client.exe").is_file());
    assert!(ready.snapshot.join("mods/create.hmod").is_file());
    assert!(ready.snapshot.join("tables/server-states.tsv").is_file());
    assert_eq!(
        fs::read(
            ready
                .home
                .join(".horizon-cache/mod-content/assets/create/icon.png")
        )
        .unwrap(),
        b"icon"
    );
    assert!(ready.command().get_args().any(|a| a == "--dev-mod"));
    drop(ready);

    // Sequence 2 as .tar.gz with the same client binaries reuses prepared resources cleanly.
    let bundle_tgz = tar_gz_archive(&[
        ("client/research-lock.json", PINS),
        ("client/horizon-client.exe", b"client-v1"),
        ("client/horizon-prepare.exe", b"prep-v1"),
        ("mods/create.hmod", &hmod),
        ("tables/server-states.tsv", b"0\tminecraft:air\tUPDATED\n"),
        ("tables/server-states.registries.tsv", &regs),
        ("tables/server-states.codecs.tsv", &codecs),
    ]);
    let tgz_name = "horizon-pack-v2.tar.gz";
    release.assets.insert(tgz_name.into(), bundle_tgz.clone());
    release.manifest["sequence"] = json!(2);
    release.manifest["bundle"] = json!({
        "name": tgz_name,
        "size": bundle_tgz.len(),
        "sha256": signing::sha256_hex(&bundle_tgz),
    });

    let ready2 = update_with(
        &release.config,
        &scratch.0,
        &|u| release.fetch(u),
        &|u, a, to| release.download(u, a, to),
        &|_| panic!("identical client binaries inside bundle should reuse prepared resources"),
    )
    .unwrap();
    assert_eq!(
        fs::read(ready2.snapshot.join("tables/server-states.tsv")).unwrap(),
        b"0\tminecraft:air\tUPDATED\n"
    );
}

#[test]
fn vps_bundle_rejects_mojang_files_symlinks_traversal_and_wasm() {
    let scratch = Scratch::new();
    let release = Release::new();
    let hmod = release.assets["create.hmod"].clone();
    let states = release.assets["server-states.tsv"].clone();
    let regs = release.assets["server-states.registries.tsv"].clone();
    let codecs = release.assets["server-states.codecs.tsv"].clone();

    for (extra_name, extra_bytes) in [
        ("client/client.jar", b"mojang".as_slice()),
        (
            "client/assets/minecraft/textures/a.png",
            b"mojang".as_slice(),
        ),
        ("mod-cache/assets/minecraft/a.png", b"mojang".as_slice()),
        (
            "mod-cache/data/minecraft/recipe/a.json",
            b"mojang".as_slice(),
        ),
        ("mod-cache/pack.png", b"mojang".as_slice()),
        ("mod-cache/horizon-resources.json", b"{}".as_slice()),
        ("unexpected/file.txt", b"bad".as_slice()),
        ("client/../escape.txt", b"bad".as_slice()),
    ] {
        let mut entries = vec![
            ("client/research-lock.json", PINS),
            ("client/horizon-client.exe", b"client".as_slice()),
            ("client/horizon-prepare.exe", b"prep".as_slice()),
            ("mods/create.hmod", hmod.as_slice()),
            ("tables/server-states.tsv", states.as_slice()),
            ("tables/server-states.registries.tsv", regs.as_slice()),
            ("tables/server-states.codecs.tsv", codecs.as_slice()),
        ];
        entries.push((extra_name, extra_bytes));
        let path = scratch.0.join("bad-bundle.zip");
        fs::write(&path, archive(&entries)).unwrap();
        let snap = scratch.0.join("snap");
        fs::create_dir_all(&snap).unwrap();
        assert!(
            unpack_bundle(&path, &snap).is_err(),
            "expected bundle rejection for {extra_name}"
        );
        let _ = fs::remove_dir_all(&snap);
        let _ = fs::remove_file(&path);
    }

    // Backslash inside tar entry path (written as raw bytes without OS path normalization) must be rejected cleanly.
    let mut tar_bytes = tar_gz_raw_entries(&[
        (b"client/research-lock.json", PINS, false),
        (b"client/horizon-client.exe", b"client", false),
        (b"client/horizon-prepare.exe", b"prep", false),
        (b"mods\\evil.hmod", &hmod, false),
        (b"tables/server-states.tsv", &states, false),
        (b"tables/server-states.registries.tsv", &regs, false),
        (b"tables/server-states.codecs.tsv", &codecs, false),
    ]);
    let tar_path = scratch.0.join("bad.tar.gz");
    fs::write(&tar_path, &tar_bytes).unwrap();
    let snap = scratch.0.join("snap-tar");
    fs::create_dir_all(&snap).unwrap();
    assert!(unpack_bundle(&tar_path, &snap).is_err());

    // Symlink inside tar bundle must also be rejected cleanly.
    tar_bytes = tar_gz_raw_entries(&[
        (b"client/research-lock.json", PINS, false),
        (b"client/horizon-client.exe", b"client", false),
        (b"client/horizon-prepare.exe", b"prep", false),
        (b"client/link", b"../outside", true),
        (b"tables/server-states.tsv", &states, false),
        (b"tables/server-states.registries.tsv", &regs, false),
        (b"tables/server-states.codecs.tsv", &codecs, false),
    ]);
    let symlink_tar = scratch.0.join("symlink.tar.gz");
    fs::write(&symlink_tar, tar_bytes).unwrap();
    let snap2 = scratch.0.join("snap-symlink");
    fs::create_dir_all(&snap2).unwrap();
    assert!(unpack_bundle(&symlink_tar, &snap2).is_err());
}

fn tar_gz_raw_entries(entries: &[(&[u8], &[u8], bool)]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    for &(name, data, is_symlink) in entries {
        let mut header = tar::Header::new_gnu();
        let slot = &mut header.as_gnu_mut().unwrap().name;
        slot[..name.len()].copy_from_slice(name);
        if is_symlink {
            header.set_entry_type(tar::EntryType::Symlink);
            let link = &mut header.as_gnu_mut().unwrap().linkname;
            link[..data.len()].copy_from_slice(data);
            header.set_size(0);
        } else {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(data.len() as u64);
        }
        header.set_mode(0o644);
        header.set_cksum();
        encoder.write_all(header.as_bytes()).unwrap();
        if !is_symlink {
            encoder.write_all(data).unwrap();
            let rem = (512 - (data.len() % 512)) % 512;
            if rem > 0 {
                encoder.write_all(&vec![0u8; rem]).unwrap();
            }
        }
    }
    encoder.write_all(&[0u8; 1024]).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn prebuilt_derived_caches_check_and_prewarm_skip() {
    let scratch = Scratch::new();
    let home = scratch.0.join("home");
    let derived = home.join(".horizon-cache/mod-content/.horizon-derived");

    // 1. Initially false (directory does not exist)
    assert!(!has_prebuilt_derived_caches(&home));

    // 2. Directory exists, but files missing -> false
    fs::create_dir_all(&derived).unwrap();
    assert!(!has_prebuilt_derived_caches(&home));

    // 3. Three of four files exist -> false
    fs::write(derived.join("baked-render-v2"), b"render").unwrap();
    fs::write(derived.join("block-facts-v3"), b"facts").unwrap();
    fs::write(derived.join("items-table-v1"), b"items").unwrap();
    assert!(!has_prebuilt_derived_caches(&home));

    // 4. Fourth file is 0 bytes -> false
    fs::write(derived.join("prepared-textures-v1"), b"").unwrap();
    assert!(!has_prebuilt_derived_caches(&home));

    // 5. All four files non-empty but no completion marker -> false (could be a partial copy)
    fs::write(derived.join("prepared-textures-v1"), b"textures").unwrap();
    assert!(!has_prebuilt_derived_caches(&home));

    // 5b. The marker, written last, completes it
    fs::write(mod_content(&home).join(CACHE_COMPLETE), b"").unwrap();
    assert!(has_prebuilt_derived_caches(&home));

    // 6. prewarm_client_caches logs "Using prebuilt derived game caches" and skips command execution
    let snap = scratch.0.join("snap");
    let client_dir = snap.join("client");
    let tables_dir = snap.join("tables");
    fs::create_dir_all(&client_dir).unwrap();
    fs::create_dir_all(&tables_dir).unwrap();
    fs::write(client_dir.join("horizon-client.exe"), b"exe").unwrap();
    fs::write(tables_dir.join("server-states.tsv"), b"states").unwrap();

    let manifest: Manifest = serde_json::from_value(json!({
        "schema": 1,
        "sequence": 1,
        "minecraft": "26.3",
        "server": "127.0.0.1:25565",
        "client": {"name": "c.zip", "size": 0, "sha256": ""},
        "mods": [],
        "states": {"name": "s.tsv", "size": 0, "sha256": ""},
        "registries": {"name": "r.tsv", "size": 0, "sha256": ""},
        "codecs": {"name": "c.tsv", "size": 0, "sha256": ""},
    }))
    .unwrap();

    let ready = Ready {
        manifest,
        snapshot: snap,
        home: home.clone(),
        _lock: Lock::take(&scratch.0.join("lock-dir")).unwrap(),
    };

    let logs = std::cell::RefCell::new(Vec::new());
    prewarm_client_caches(&ready, &|msg| logs.borrow_mut().push(msg));
    assert_eq!(*logs.borrow(), vec!["Using prebuilt derived game caches"]);
}

fn bundle_entries<'a>(
    release: &Release,
    client_marker: &'a [u8],
    extra: &[(&'a str, &'a [u8])],
) -> Vec<(&'a str, Vec<u8>)> {
    let mut entries: Vec<(&str, Vec<u8>)> = vec![
        ("client/research-lock.json", PINS.to_vec()),
        ("client/horizon-client.exe", client_marker.to_vec()),
        ("client/horizon-prepare.exe", b"prep".to_vec()),
        ("mods/create.hmod", release.assets["create.hmod"].clone()),
        (
            "tables/server-states.tsv",
            release.assets["server-states.tsv"].clone(),
        ),
        (
            "tables/server-states.registries.tsv",
            release.assets["server-states.registries.tsv"].clone(),
        ),
        (
            "tables/server-states.codecs.tsv",
            release.assets["server-states.codecs.tsv"].clone(),
        ),
    ];
    entries.extend(extra.iter().map(|(n, b)| (*n, b.to_vec())));
    entries
}

fn zip_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let borrowed: Vec<(&str, &[u8])> = entries.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    archive(&borrowed)
}

/// A VPS-mode release whose manifest is one bundle with the given client marker and mod-cache.
fn vps_bundle(sequence: u64, client_marker: &[u8], cache: &[(&str, &[u8])]) -> Release {
    let mut release = Release::new();
    release.config.repository.clear();
    release.config.channel.clear();
    release.config.url = Some("https://packs.example.invalid/pack".into());
    release.config.token = Some("vps-secret".into());
    set_bundle(&mut release, sequence, client_marker, cache);
    release
}

fn set_bundle(
    release: &mut Release,
    sequence: u64,
    client_marker: &[u8],
    cache: &[(&str, &[u8])],
) {
    let bundle = zip_of(&bundle_entries(release, client_marker, cache));
    let name = format!("horizon-pack-v{sequence}.zip");
    release.manifest = json!({
        "schema": 1,
        "sequence": sequence,
        "minecraft": "26.3",
        "server": "play.example.invalid:25565",
        "bundle": {"name": name, "size": bundle.len(), "sha256": signing::sha256_hex(&bundle)},
    });
    release.assets.insert(name, bundle);
}

fn derived_file(ready_home: &Path, name: &str) -> PathBuf {
    mod_content(ready_home).join(".horizon-derived").join(name)
}

#[test]
fn bundle_refuses_prebuilt_textures_but_accepts_other_derived_caches() {
    let scratch = Scratch::new();
    let release = Release::new();
    let accepted = [
        ("mod-cache/.horizon-derived/block-facts-v3", b"facts".as_slice()),
        ("mod-cache/.horizon-derived/items-table-v1", b"items".as_slice()),
    ];
    let path = scratch.0.join("ok.zip");
    fs::write(&path, zip_of(&bundle_entries(&release, b"client", &accepted))).unwrap();
    let snap = scratch.0.join("snap");
    fs::create_dir_all(&snap).unwrap();
    unpack_bundle(&path, &snap).unwrap();
    // Even a complete seeding can never satisfy the prebuilt check: the textures are built locally.
    let home = scratch.0.join("home");
    seed_home_cache(&snap, &home).unwrap();
    assert!(derived_file(&home, "block-facts-v3").is_file());
    assert!(mod_content(&home).join(CACHE_COMPLETE).is_file());
    assert!(!has_prebuilt_derived_caches(&home));

    for (n, forbidden) in [
        ("prepared-textures-v1", "mod-cache/.horizon-derived/prepared-textures-v1"),
        ("baked-render-v2", "mod-cache/.horizon-derived/baked-render-v2"),
        ("prepared-textures-v9", "mod-cache/.horizon-derived/Prepared-Textures-v9"),
        ("hud-atlas-v1", "mod-cache/.horizon-derived/hud-atlas-v1"),
        ("language-v1", "mod-cache/.horizon-derived/language-v1"),
        ("en-names-v1", "mod-cache/.horizon-derived/En-Names-v1"),
        ("loot-tables-v1", "mod-cache/.horizon-derived/loot-tables-v1/minecraft.tsv"),
    ] {
        let extra = [(forbidden, b"textures".as_slice())];
        fs::write(&path, zip_of(&bundle_entries(&release, b"client", &extra))).unwrap();
        let snap = scratch.0.join(format!("snap-{n}"));
        fs::create_dir_all(&snap).unwrap();
        assert!(unpack_bundle(&path, &snap).is_err(), "{forbidden} must be refused");
    }
}

#[test]
fn bundle_with_wrong_hash_is_never_unpacked() {
    let scratch = Scratch::new();
    let release = vps_bundle(1, b"client-v1", &[]);
    let root = scratch
        .0
        .join(signing::sha256_hex(release.config.scope().as_bytes()));
    let snapshot = root
        .join("snapshots")
        .join(signing::sha256_hex(&release.metadata().0));
    let result = update_with(
        &release.config,
        &scratch.0,
        &|u| release.fetch(u),
        &|_, a, to| fs::write(to, vec![0u8; a.size as usize]).map_err(err),
        &|_| panic!("a bundle that failed its hash must not be prepared"),
    );
    assert!(result.is_err());
    assert!(!snapshot.join("client").exists());
    assert!(current(&root).unwrap().is_none());
}

#[test]
fn vps_metadata_with_a_bad_signature_is_rejected() {
    let scratch = Scratch::new();
    let release = vps_bundle(1, b"client-v1", &[]);
    let (bytes, sums, _) = release.metadata();
    let (other, _) = signing::SigningKey::generate().unwrap();
    let forged = other.sign(&release.config.scope(), &sums);
    assert!(authenticated_manifest(&release.config, &bytes, &sums, forged.as_bytes()).is_err());
    // A genuine signature for another scope (the GitHub channel) is not valid for the VPS.
    let wrong_scope = release.key.sign("friends:owner/repo:channel", &sums);
    assert!(authenticated_manifest(&release.config, &bytes, &sums, wrong_scope.as_bytes()).is_err());

    let result = update_with(
        &release.config,
        &scratch.0,
        &|u| match u.rsplit('/').next().unwrap() {
            signing::SIGNATURE => Ok(forged.clone().into_bytes()),
            _ => release.fetch(u),
        },
        &|_, _, _| panic!("nothing is downloaded under a bad signature"),
        &|_| panic!("nothing is prepared under a bad signature"),
    );
    assert!(result.is_err());
}

#[test]
fn a_token_is_refused_over_plain_http() {
    let scratch = Scratch::new();
    let mut config = Release::new().config;
    config.repository.clear();
    config.channel.clear();
    config.url = Some("http://packs.example.invalid/pack".into());
    config.token = Some("vps-secret".into());
    assert!(config.keys().unwrap_err().contains("https"));
    let result = update_with(
        &config,
        &scratch.0,
        &|_| panic!("nothing is fetched"),
        &|_, _, _| panic!("nothing is downloaded"),
        &|_| panic!("nothing is prepared"),
    );
    assert!(result.is_err());
    assert!(
        backend::get_with_token("http://packs.example.invalid/pack/friends.json", Some("t"))
            .unwrap_err()
            .contains("not https")
    );
    config.token = Some("  ".into());
    assert!(config.keys().is_ok());
    config.token = Some("vps-secret".into());
    config.url = Some("https://packs.example.invalid/pack".into());
    assert!(config.keys().is_ok());
}

#[test]
fn zip_symlink_in_a_bundle_is_rejected() {
    let scratch = Scratch::new();
    let release = Release::new();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in bundle_entries(&release, b"client", &[]) {
        zip.start_file(name, options).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.add_symlink("client/link", "../outside", options).unwrap();
    let path = scratch.0.join("symlink.zip");
    fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
    let snap = scratch.0.join("snap");
    fs::create_dir_all(&snap).unwrap();
    assert!(unpack_bundle(&path, &snap).is_err());
    assert!(fs::symlink_metadata(snap.join("client/link")).is_err());
}

#[test]
fn accepting_a_new_snapshot_invalidates_the_previous_prewarm() {
    let scratch = Scratch::new();
    let mut release = vps_bundle(
        1,
        b"client-v1",
        &[("mod-cache/.horizon-derived/block-facts-v3", b"v1")],
    );
    let ready = release.install(&scratch.0).unwrap();
    let home = ready.home.clone();
    drop(ready);
    // A finished prewarm of v1: every key cache plus the marker.
    for name in [
        "baked-render-v2",
        "block-facts-v3",
        "items-table-v1",
        "prepared-textures-v1",
    ] {
        fs::write(derived_file(&home, name), b"v1").unwrap();
    }
    fs::write(mod_content(&home).join(CACHE_COMPLETE), b"").unwrap();
    assert!(has_prebuilt_derived_caches(&home));

    // v2 ships a new exe but no mod-cache: the v1 caches no longer match it.
    set_bundle(&mut release, 2, b"client-v2", &[]);
    drop(release.install(&scratch.0).unwrap());
    assert!(!has_prebuilt_derived_caches(&home));
}

#[test]
fn prewarm_progress_shows_stage_lines_only() {
    assert!(is_prewarm_progress(
        "Horizon: stage=baked_render cache=miss elapsed_ms=125528"
    ));
    assert!(is_prewarm_progress("Horizon: mod content root built in 935.4s"));
    assert!(is_prewarm_progress(
        "Horizon: resources loaded in 14.7s; 445432 block states"
    ));
    assert!(!is_prewarm_progress(
        "Horizon: using the missing texture for create:block/seat/top_tan"
    ));
    assert!(!is_prewarm_progress(""));
}

#[test]
fn failed_preparation_leaves_the_home_cache_untouched() {
    let scratch = Scratch::new();
    let mut release = vps_bundle(
        1,
        b"client-v1",
        &[("mod-cache/.horizon-derived/block-facts-v3", b"old")],
    );
    let ready = release.install(&scratch.0).unwrap();
    let home = ready.home.clone();
    drop(ready);
    assert_eq!(fs::read(derived_file(&home, "block-facts-v3")).unwrap(), b"old");

    set_bundle(
        &mut release,
        2,
        b"client-v2",
        &[("mod-cache/.horizon-derived/block-facts-v3", b"new")],
    );
    let failed = update_with(
        &release.config,
        &scratch.0,
        &|u| release.fetch(u),
        &|u, a, to| release.download(u, a, to),
        &|_| Err("Java missing".into()),
    );
    assert!(failed.is_err());
    assert_eq!(fs::read(derived_file(&home, "block-facts-v3")).unwrap(), b"old");
    assert!(mod_content(&home).join(CACHE_COMPLETE).is_file());

    drop(release.install(&scratch.0).unwrap());
    assert_eq!(fs::read(derived_file(&home, "block-facts-v3")).unwrap(), b"new");
    let cache = home.join(".horizon-cache");
    assert!(!cache.join("mod-content.new").exists());
    assert!(!cache.join("mod-content.old").exists());
}

#[test]
fn snapshot_cleanup_keeps_the_current_and_previous_only() {
    let scratch = Scratch::new();
    let mut release = vps_bundle(1, b"client-v1", &[]);
    let ready = release.install(&scratch.0).unwrap();
    let snapshots = ready.home.parent().unwrap().join("snapshots");
    drop(ready);
    let mut newest = None;
    for sequence in 2..=4u64 {
        set_bundle(&mut release, sequence, format!("client-v{sequence}").as_bytes(), &[]);
        newest = Some(release.install(&scratch.0).unwrap().snapshot);
        let count = fs::read_dir(&snapshots).unwrap().count();
        assert_eq!(count, 2, "after sequence {sequence}");
    }
    assert!(newest.unwrap().join("client/horizon-client.exe").is_file());
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn lock_left_by_a_dead_process_is_taken_over_but_a_live_one_is_not() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("friends.lock"), "4294967294\n").unwrap();
    let lock = Lock::take(&scratch.0).unwrap();
    assert!(Lock::take(&scratch.0).is_err());
    drop(lock);
    assert!(Lock::take(&scratch.0).is_ok());
}
