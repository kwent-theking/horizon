#!/usr/bin/env python3
"""RustTurbo - cargo build orchestrator written in Python (faster to develop than Rust).

EN: Speeds up iteration on Rust projects by:
    - fingerprinting sources (mtime + size + content SHA256) and reusing artifacts,
    - dispatching cargo with -j N and RUSTC_WRAPPER=sccache (if found in PATH),
    - validating produced binaries (exists, > 1 MB, optional --frames 1 smoke run),
    - profiling every stage and printing a timing summary.

RU: Uskoryaet iteracii po Rust-proektu:
    - fingerprint ishodnikov (mtime + razmer + SHA256) i pereispolzovanie artefaktov,
    - zapusk cargo s -j N i RUSTC_WRAPPER=sccache (esli najden v PATH),
    - validaciya binarnika (sushchestvuet, > 1 MB, opcionalnyj smoke --frames 1),
    - profilirovanie stadij i itogovaya svodka tajmingov.

Stdlib only / tolko standartnaya biblioteka. Windows compatible (.exe suffix on Windows).
"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import time
from contextlib import contextmanager
from pathlib import Path

CACHE_FILE = ".rustturbo-cache.json"        # cache location: project root / kesh v korne proekta
MIN_EXE_BYTES = 1024 * 1024                 # 1 MB minimal artifact size / minimalnyj razmer artefakta
SMOKE_TIMEOUT = 60                          # seconds for the smoke run / sekund na smoke-zapusk

# Profile Analyzer: profile name -> cargo flags + target subdirectory.
# Analizator profilej: imya profilya -> flagi cargo + podkatalog target.
PROFILES = {
    "fast":    {"cargo_flags": [],            "dir": "debug"},
    "dev":     {"cargo_flags": [],            "dir": "debug"},
    "release": {"cargo_flags": ["--release"], "dir": "release"},
}


def exe_suffix():
    """Windows compatibility: .exe on Windows, empty elsewhere.
    Windows-sovmestimost: .exe pod Windows, pustaya stroka inache."""
    return ".exe" if os.name == "nt" else ""


# ---------------------------------------------------------------------------
# Build Profiler / Profilirovshchik sborki
# ---------------------------------------------------------------------------
class Profiler:
    """Build Profiler: records per-stage wall time, prints a summary.
    Profilirovshchik sborki: tajmingi stadij i itogovaya svodka."""

    def __init__(self):
        self.stages = []  # (name, seconds, note)

    @contextmanager
    def stage(self, name):
        """Context manager measuring one stage's duration.
        Kontekstnyj menedzher, izmeryayushchij dlitelnost odnoj stadii."""
        t0 = time.perf_counter()
        note = ["ok"]
        try:
            yield note
        finally:
            sec = time.perf_counter() - t0
            self.stages.append((name, sec, note[0]))
            print(f"[rustturbo] {name:<26} {sec:7.2f}s  {note[0]}", flush=True)

    def summary(self):
        """Final timing table for the whole run / itogovaya tablica tajmingov."""
        total = sum(sec for _, sec, _ in self.stages)
        print("\n[rustturbo] ===== Build Profiler =====")
        for name, sec, note in self.stages:
            print(f"  {name:<28} {sec:8.2f}s  {note}")
        print(f"  {'TOTAL':<28} {total:8.2f}s")


# ---------------------------------------------------------------------------
# Smart Scheduler / Uмный planirovshchik
# ---------------------------------------------------------------------------
def find_sccache():
    """Locate sccache in PATH, None if absent.
    Ischem sccache v PATH, None esli net."""
    return shutil.which("sccache")


def run_cargo(root, cargo_args, jobs):
    """Run cargo with -j N and RUSTC_WRAPPER=sccache when available.
    Zapusk cargo s -j N i RUSTC_WRAPPER=sccache pri nalichii."""
    env = os.environ.copy()
    wrapper = find_sccache()
    if wrapper:
        env["RUSTC_WRAPPER"] = "sccache"
    cmd = ["cargo", *cargo_args, "-j", str(jobs)]
    print(f"[rustturbo] $ {' '.join(cmd)} (sccache={'on' if wrapper else 'off'})", flush=True)
    proc = subprocess.run(cmd, cwd=str(root), env=env)
    if proc.returncode != 0:
        raise SystemExit(f"[rustturbo] cargo failed (exit {proc.returncode}): {' '.join(cargo_args)}")


# ---------------------------------------------------------------------------
# Project & Dependency Analyzer / Analizator proekta i zavisimostej
# ---------------------------------------------------------------------------
def analyze_project(root):
    """Parse `cargo metadata --format-version 1` -> workspace packages,
    per-package dependencies, primary bin name.
    Parsim `cargo metadata --format-version 1` -> pakety worskpace-a,
    zavisimosti kazhdogo paketa, imya glavnogo binarnika."""
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=str(root), capture_output=True, text=True)
    if proc.returncode != 0:
        raise SystemExit(f"[rustturbo] cargo metadata failed:\n{proc.stderr.strip()}")
    meta = json.loads(proc.stdout)

    member_ids = set(meta.get("workspace_members", []))
    packages = [p for p in meta.get("packages", []) if p["id"] in member_ids]
    # Root package first so its bin target wins / kornevoj paket pervym - ego bin glavnyj
    ws_root = Path(meta["workspace_root"])
    packages.sort(key=lambda p: 0 if Path(p["manifest_path"]).parent == ws_root else 1)

    deps = {p["name"]: sorted({d["name"] for d in p.get("dependencies", [])})
            for p in packages}

    bin_name = None
    for p in packages:
        for t in p.get("targets", []):
            if "bin" in t.get("kind", []):
                bin_name = t["name"]
                break
        if bin_name:
            break
    if not bin_name:
        raise SystemExit("[rustturbo] no bin target found in workspace / bin-tarhet ne najden")
    return {"packages": packages, "deps": deps, "bin": bin_name}


# ---------------------------------------------------------------------------
# Fingerprint Engine / Dvizhok fingerprint-ov
# ---------------------------------------------------------------------------
def file_digest(path):
    """SHA256 of file content / SHA256 soderzhimogo fajla."""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def compute_fingerprint(root, info):
    """SHA256 over package sources (mtime + size + content hash of each .rs),
    Cargo.toml / Cargo.lock and the dependency graph. Any change -> new hash.
    SHA256 po ishodnikam paketov (mtime + razmer + hesh kazhdogo .rs),
    Cargo.toml / Cargo.lock i grafu zavisimostej. Lyuboe izmenenie -> novyj hesh."""
    h = hashlib.sha256()
    h.update(json.dumps(info["deps"], sort_keys=True).encode("utf-8"))
    lock = root / "Cargo.lock"
    if lock.exists():
        h.update(file_digest(lock).encode("utf-8"))
    for pkg in info["packages"]:
        pkg_dir = Path(pkg["manifest_path"]).parent
        h.update(pkg["id"].encode("utf-8"))
        manifest = pkg_dir / "Cargo.toml"
        if manifest.exists():
            h.update(file_digest(manifest).encode("utf-8"))
        for src in sorted(pkg_dir.rglob("*.rs")):
            st = src.stat()
            h.update(str(src).encode("utf-8"))
            h.update(f"{st.st_mtime}:{st.st_size}".encode("utf-8"))
            h.update(file_digest(src).encode("utf-8"))
    return h.hexdigest()


def load_cache(root):
    """Load .rustturbo-cache.json, tolerate corruption / chitaem kesh, lyubim povrezhdeniya."""
    p = root / CACHE_FILE
    if p.exists():
        try:
            return json.loads(p.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            pass
    return {}


def save_cache(root, cache):
    """Persist cache (indent=2, UTF-8, CRLF-safe content) / sohranyaem kesh."""
    (root / CACHE_FILE).write_text(json.dumps(cache, indent=2), encoding="utf-8")


# ---------------------------------------------------------------------------
# Artifact Validator / Validator artefaktov
# ---------------------------------------------------------------------------
def validate_artifact(exe, smoke, prof):
    """Artifact exists + size > 1 MB + optional smoke run (--frames 1, 60s timeout).
    Artefakt sushchestvuet + razmer > 1 MB + opcionalnyj smoke-zapusk (--frames 1, 60s)."""
    with prof.stage("validate") as note:
        if not exe.exists():
            raise SystemExit(f"[rustturbo] artifact missing / artefakt ne najden: {exe}")
        size = exe.stat().st_size
        if size <= MIN_EXE_BYTES:
            raise SystemExit(f"[rustturbo] artifact too small / artefakt slishkom malen': {size} bytes")
        note[0] = f"{size / 1048576:.1f} MB"
    if smoke:
        with prof.stage("smoke") as note:
            try:
                proc = subprocess.run([str(exe), "--frames", "1"],
                                      cwd=str(exe.parent), timeout=SMOKE_TIMEOUT)
            except subprocess.TimeoutExpired:
                raise SystemExit(f"[rustturbo] smoke timeout ({SMOKE_TIMEOUT}s) / smoke prevyshen tajmaut")
            if proc.returncode != 0:
                raise SystemExit(f"[rustturbo] smoke failed (exit {proc.returncode}) / smoke ne proshel")
            note[0] = "--frames 1 ok"


# ---------------------------------------------------------------------------
# Build Controller / Kontroler sborki
# ---------------------------------------------------------------------------
def do_build(root, profile, jobs, smoke, prof):
    """Full build pipeline: analyze -> fingerprint -> reuse-or-cargo -> validate.
    Polnyj cikl sborki: analiz -> fingerprint -> kesh-ili-cargo -> validaciya."""
    cfg = PROFILES[profile]

    with prof.stage("analyze") as note:
        info = analyze_project(root)
        note[0] = f"{len(info['packages'])} pkg(s), bin={info['bin']}"

    with prof.stage("fingerprint") as note:
        fp = compute_fingerprint(root, info)
        note[0] = f"sha256:{fp[:12]}"

    exe = root / "target" / cfg["dir"] / (info["bin"] + exe_suffix())
    key = f"{profile}:{info['bin']}"
    entry = load_cache(root).get(key, {})

    # Reuse: artifact exists AND fingerprint matches AND exe hash valid -> skip build.
    # Pereispolzovanie: artefakt est I fingerprint sovpal I hesh exe validen -> propusk sborki.
    reuse = False
    if exe.exists() and entry.get("fingerprint") == fp:
        try:
            reuse = entry.get("exe_hash") == file_digest(exe)
        except OSError:
            reuse = False

    if reuse:
        with prof.stage("build") as note:
            note[0] = "CACHE HIT - build skipped / sborka propushchena"
    else:
        with prof.stage("build") as note:
            run_cargo(root, ["build", *cfg["cargo_flags"]], jobs)
            note[0] = f"cargo build ({profile})"

    validate_artifact(exe, smoke, prof)

    # Refresh cache entry after successful build + validation.
    # Obnovlyaem zapis kesha posle uspeshnoj sborki i validacii.
    cache = load_cache(root)
    cache[key] = {
        "fingerprint": fp,
        "artifact": str(exe.relative_to(root)),
        "exe_hash": file_digest(exe),
    }
    save_cache(root, cache)
    return exe


def do_ship(root, dest, jobs, prof):
    """ship: check -> build fast -> smoke -> copy artifact to destination.
    ship: check -> bystraya sborka -> smoke -> kopirovanie artefakta v papku."""
    with prof.stage("ship:check") as note:
        run_cargo(root, ["check"], jobs)
        note[0] = "cargo check passed / check proshel"

    exe = do_build(root, "fast", jobs, smoke=True, prof=prof)

    with prof.stage("ship:copy") as note:
        dest_dir = Path(dest)
        dest_dir.mkdir(parents=True, exist_ok=True)
        target = dest_dir / exe.name
        shutil.copy2(exe, target)
        note[0] = str(target)
    return target


# ---------------------------------------------------------------------------
# CLI / Interfejs komandnoj stroki
# ---------------------------------------------------------------------------
def build_parser():
    p = argparse.ArgumentParser(
        prog="rustturbo",
        description="RustTurbo: fast cargo orchestrator / bystryj orkestrator sborki cargo")
    p.add_argument("-j", "--jobs", type=int, default=4,
                   help="cargo parallel jobs (default 4) / parallel'nye zadachi cargo")
    p.add_argument("--smoke", action="store_true",
                   help="smoke-run exe with --frames 1 (60s timeout) / smoke-test binarnika")
    p.add_argument("--root", default=None,
                   help="project root (default: parent of tools/) / koren' proekta")

    sub = p.add_subparsers(dest="command", required=True)
    b = sub.add_parser("build", help="build with --profile / sborka s profilem")
    b.add_argument("--profile", choices=sorted(PROFILES), default="fast")
    sub.add_parser("fast", help="shortcut for build --profile fast / sborka fast")
    sub.add_parser("release", help="shortcut for build --profile release / sborka release")
    sub.add_parser("check", help="cargo check")
    sub.add_parser("test", help="cargo test")
    sh = sub.add_parser("ship", help="check -> fast build -> smoke -> copy / postavka")
    sh.add_argument("--dest", required=True,
                    help="destination folder / celovaya papka")
    return p


def main():
    args = build_parser().parse_args()

    if args.root:
        root = Path(args.root).resolve()
    else:
        # tools/rustturbo.py -> project root one level above / koren' na uroven' vyshe
        root = Path(__file__).resolve().parent.parent
    if not (root / "Cargo.toml").exists():
        raise SystemExit(f"[rustturbo] no Cargo.toml in {root} / Cargo.toml ne najden")

    prof = Profiler()
    if args.command == "build":
        do_build(root, args.profile, args.jobs, args.smoke, prof)
    elif args.command == "fast":
        do_build(root, "fast", args.jobs, args.smoke, prof)
    elif args.command == "release":
        do_build(root, "release", args.jobs, args.smoke, prof)
    elif args.command == "check":
        with prof.stage("cargo check"):
            run_cargo(root, ["check"], args.jobs)
    elif args.command == "test":
        with prof.stage("cargo test") as note:
            run_cargo(root, ["test"], args.jobs)
            note[0] = "tests passed / testy proshli"
    elif args.command == "ship":
        target = do_ship(root, args.dest, args.jobs, prof)
        print(f"[rustturbo] shipped / otgruzheno: {target}")
    prof.summary()


if __name__ == "__main__":
    main()
