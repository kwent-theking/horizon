#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Port a mod list (Modrinth, MC 1.21.1 + NeoForge) to Horizon.

Pipeline per mod: search on Modrinth v2 -> download the primary
1.21.1/neoforge jar (sha1-checked) into <out>/work/<slug>.jar ->
horizon-port-pack `check <jar> <out>/check/<slug>` for the verdict
(Works / Partly / Doesn't run) and the carry report.

Run it yourself (network is slow, the orchestrator decides the scale):
    python tools/port_modlist.py --dry-run --limit 3   # search only
    python tools/port_modlist.py --limit 5             # search + download + check
    python tools/port_modlist.py --mods create,pipez   # only these
    python tools/port_modlist.py --list my-mods.json --out C:\\out

Output: <out>/port-report.json, <out>/PORT-REPORT.md and per-mod
<out>/work/<slug>.jar + <out>/check/<slug>/check.json.
Default <out> is C:\\Users\\kwent\\horizon-release.

Stdlib only (urllib). 0.5s pause between Modrinth requests, retries
on 429/5xx.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

API = "https://api.modrinth.com/v2"
UA = "kwent/horizon-port/1.0 (port_modlist.py)"
GAME_VERSIONS = ["1.21.1"]
LOADERS = ["neoforge"]
PAUSE = 0.5          # s, Modrinth asks for a pause between requests
RETRIES = 4
DEFAULT_OUT = r"C:\Users\kwent\horizon-release"
EXE = Path(__file__).resolve().parent.parent / "target" / "release" / "horizon-port-pack.exe"

# Default mod list: (search name, Modrinth slug or "auto-search").
# "auto-search" = I am not sure of the slug, the API search decides
# (first hit with 1.21.1 + neoforge).
DEFAULT_MODS: list[dict[str, str]] = [
    {"name": "Create", "slug": "create"},
    {"name": "Create Big Cannons", "slug": "create-big-cannons"},
    {"name": "Create Food", "slug": "create-food"},
    {"name": "Ars Nouveau", "slug": "ars-nouveau"},
    {"name": "Occultism", "slug": "occultism"},
    {"name": "Apotheosis", "slug": "apotheosis"},
    {"name": "Artifacts", "slug": "artifacts"},
    {"name": "Relics", "slug": "relics"},
    {"name": "Immersive Armors", "slug": "immersive-armors"},
    {"name": "L_Ender's Cataclysm", "slug": "cataclysm"},
    {"name": "Mowzie's Mobs", "slug": "mowzies-mobs"},
    {"name": "Born in Chaos", "slug": "born-in-chaos"},
    {"name": "Naturalist", "slug": "naturalist"},
    {"name": "Aquamirae", "slug": "aquamirae"},
    {"name": "The Graveyard", "slug": "the-graveyard"},
    {"name": "When Dungeons Arise", "slug": "when-dungeons-arise"},
    {"name": "YUNG's Better Dungeons", "slug": "yungs-better-dungeons"},
    {"name": "Biomes O' Plenty", "slug": "biomes-o-plenty"},
    {"name": "Regions Unexplored", "slug": "regions-unexplored"},
    {"name": "Incendium", "slug": "incendium"},
    {"name": "The Aether", "slug": "aether"},
    {"name": "Farmer's Delight", "slug": "farmers-delight"},
    {"name": "Supplementaries", "slug": "supplementaries"},
    {"name": "Chipped", "slug": "chipped"},
    {"name": "Handcrafted", "slug": "handcrafted"},
    {"name": "Every Compat", "slug": "every-compat"},
    {"name": "Better Combat", "slug": "better-compat"},
    {"name": "Applied Energistics 2", "slug": "applied-energistics-2"},
    {"name": "CC: Tweaked", "slug": "cc-tweaked"},
    {"name": "Industrial Foregoing", "slug": "industrial-foregoing"},
    {"name": "Pipez", "slug": "pipez"},
    {"name": "Sophisticated Backpacks", "slug": "sophisticated-backpacks"},
    {"name": "JEI", "slug": "jei"},
    {"name": "Jade", "slug": "jade"},
    {"name": "WTHIT", "slug": "wthit"},
    {"name": "Waystones", "slug": "waystones"},
    {"name": "Mouse Tweaks", "slug": "mouse-tweaks"},
    {"name": "Polymorph", "slug": "polymorph"},
    {"name": "Corail Tombstone", "slug": "corail-tombstone"},
    {"name": "Serene Seasons", "slug": "serene-seasons"},
    {"name": "Create Aeronautics", "slug": "create-aeronautics"},
    {"name": "Create Crafts & Additions", "slug": "create-crafts-additions"},
    {"name": "Create New Age", "slug": "create-new-age"},
    {"name": "Create Ore Excavation", "slug": "create-ore-excavation"},
    {"name": "Create Enchantment Industry", "slug": "create-enchantment-industry"},
    {"name": "Create Mechanical Extruder", "slug": "create-mechanical-extruder"},
    {"name": "Create Dragons Plus", "slug": "auto-search"},
    {"name": "Create Design n Decor", "slug": "auto-search"},
    {"name": "Create Deco", "slug": "create-deco"},
    {"name": "Copycats+", "slug": "copycats"},
    {"name": "Create Goggles", "slug": "auto-search"},
    {"name": "Sable", "slug": "auto-search"},
    {"name": "Veil", "slug": "auto-search"},
    {"name": "Mechanicals Lib", "slug": "auto-search"},
    {"name": "Ritchie's Projectile Lib", "slug": "auto-search"},
    {"name": "Ars Additions", "slug": "auto-search"},
    {"name": "Iron's Spells 'n Spellbooks", "slug": "irons-spells-n-spellbooks"},
    {"name": "Mahou Tsukai", "slug": "mahou-tsukai"},
    {"name": "Modonomicon", "slug": "modonomicon"},
    {"name": "Apothic Attributes", "slug": "auto-search"},
    {"name": "Apothic Enchanting", "slug": "auto-search"},
    {"name": "Apothic Spawners", "slug": "auto-search"},
    {"name": "Artifacts Crafting", "slug": "auto-search"},
    {"name": "L2 Artifacts", "slug": "auto-search"},
    {"name": "Sol's Relic System", "slug": "auto-search"},
    {"name": "Reliquified Artifacts", "slug": "auto-search"},
    {"name": "Reliquified Twilight Forest", "slug": "auto-search"},
    {"name": "Fragmentum", "slug": "auto-search"},
    {"name": "Bosses of Mass Destruction", "slug": "bosses-of-mass-destruction"},
    {"name": "Gateways to Eternity", "slug": "auto-search"},
    {"name": "YUNG's Better Strongholds", "slug": "yungs-better-strongholds"},
    {"name": "YUNG's Better Mineshafts", "slug": "yungs-better-mineshafts"},
    {"name": "YUNG's Better Desert Temples", "slug": "yungs-better-desert-temples"},
    {"name": "YUNG's Better Nether Fortresses", "slug": "yungs-better-nether-fortresses"},
    {"name": "YUNG's Better Ocean Monuments", "slug": "yungs-better-ocean-monuments"},
    {"name": "YUNG's Better Witch Huts", "slug": "yungs-better-witch-huts"},
    {"name": "VillagersTradingPlus", "slug": "auto-search"},
    {"name": "Oh The Biomes We've Gone", "slug": "oh-the-biomes-weve-gone"},
    {"name": "YUNG's Cave Biomes", "slug": "yungs-cave-biomes"},
    {"name": "Gardens of the Dead", "slug": "gardens-of-the-dead"},
    {"name": "Nullscape", "slug": "nullscape"},
    {"name": "Enderscape", "slug": "enderscape"},
    {"name": "Deep Aether", "slug": "deep-aether"},
    {"name": "TerraBlender", "slug": "terrablender"},
    {"name": "Supplementaries Squared", "slug": "supplementaries-squared"},
    {"name": "Player Animator", "slug": "player-animator"},
    {"name": "Tom's Simple Storage", "slug": "toms-simple-storage"},
    {"name": "KubeJS", "slug": "kubejs"},
    {"name": "Sodium", "slug": "sodium"},
    {"name": "Lithium", "slug": "lithium"},
    {"name": "ModernFix", "slug": "modernfix"},
    {"name": "architectury-api", "slug": "architectury-api"},
    {"name": "GeckoLib", "slug": "geckolib"},
    {"name": "Curios", "slug": "curios"},
    {"name": "Moonlight", "slug": "moonlight"},
    {"name": "SmartBrainLib", "slug": "auto-search"},
    {"name": "owo-lib", "slug": "auto-search"},
    {"name": "GuideME", "slug": "auto-search"},
    {"name": "Patchouli", "slug": "patchouli"},
    {"name": "Placebo", "slug": "auto-search"},
    {"name": "Lionfish-API", "slug": "auto-search"},
    {"name": "CERBON's API", "slug": "auto-search"},
    {"name": "Titanium", "slug": "auto-search"},
    {"name": "OctoLib", "slug": "auto-search"},
    {"name": "GlitchCore", "slug": "auto-search"},
    {"name": "CorgiLib", "slug": "auto-search"},
    {"name": "OTTYG", "slug": "auto-search"},
    {"name": "Lithostitched", "slug": "auto-search"},
    {"name": "Trimmed", "slug": "auto-search"},
    {"name": "YACL", "slug": "yet-another-config-lib"},
    {"name": "l2library", "slug": "auto-search"},
    {"name": "Rhino", "slug": "auto-search"},
    {"name": "Sophisticated Core", "slug": "auto-search"},
    {"name": "Better Advanced Tooltips", "slug": "auto-search"},
    {"name": "YUNG's API", "slug": "yungs-api"},
]


# ---------------------------------------------------------------- net

def get(url: str, accept: str = "application/json") -> bytes:
    """GET with retries on 429/5xx. Returns body bytes."""
    delay = 1.0
    for attempt in range(RETRIES):
        req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": accept})
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code in (429, 500, 502, 503, 504) and attempt < RETRIES - 1:
                wait = float(e.headers.get("Retry-After", delay))
                print(f"    {e.code}, retry in {wait:.1f}s", file=sys.stderr)
                time.sleep(wait)
                delay = min(delay * 2, 30.0)
                continue
            raise
        except urllib.error.URLError as e:
            if attempt < RETRIES - 1:
                print(f"    network: {e.reason}, retry in {delay:.1f}s", file=sys.stderr)
                time.sleep(delay)
                delay = min(delay * 2, 30.0)
                continue
            raise
    raise RuntimeError(f"unreachable: {url}")


def get_json(url: str) -> dict:
    return json.loads(get(url).decode("utf-8"))


# ---------------------------------------------------------------- Modrinth

def search(name: str) -> dict | None:
    """Search a project by name. First hit wins (facets do the filtering)."""
    facets = json.dumps([
        ["versions:" + v for v in GAME_VERSIONS],
        ["categories:" + l for l in LOADERS],
        ["project_type:mod"],
    ])
    url = f"{API}/search?query={urllib.parse.quote(name)}&facets={urllib.parse.quote(facets)}"
    hits = get_json(url).get("hits", [])
    exact = [h for h in hits if h.get("slug", "").lower() == name.lower()]
    return (exact or hits or [None])[0]


def project_versions(project_id: str) -> list[dict]:
    url = (
        f"{API}/project/{urllib.parse.quote(project_id)}/version"
        f"?game_versions={urllib.parse.quote(json.dumps(GAME_VERSIONS))}"
        f"&loaders={urllib.parse.quote(json.dumps(LOADERS))}"
    )
    return get_json(url)


def pick_file(versions: list[dict]) -> dict | None:
    """The first stable (else any) version's primary (else any) file."""
    stable = [v for v in versions if v.get("version_type") == "release"] or versions
    for v in stable:
        files = v.get("files", [])
        primary = [f for f in files if f.get("primary")] or files
        if primary:
            f = primary[0]
            if f.get("url"):
                return f
    return None


def download(url: str, path: Path, sha1: str | None) -> tuple[bool, str]:
    """Download to path; verify sha1 when given. Returns (ok, error)."""
    try:
        data = get(url, accept="*/*")
    except Exception as e:  # noqa: BLE001 - reported in the JSON
        return False, f"download: {e}"
    got = hashlib.sha1(data).hexdigest()
    if sha1 and got != sha1:
        return False, f"sha1 mismatch: expected {sha1}, got {got}"
    path.write_bytes(data)
    return True, ""


def resolve(mod: dict, cache: dict) -> dict:
    """Slug -> project + file. `cache` maps project_id -> resolved dict."""
    slug, name = mod["slug"], mod["name"]
    if slug != "auto-search":
        hit = search(slug)  # search by slug is exact enough
    else:
        hit = search(name)
    if not hit and slug != "auto-search":
        time.sleep(PAUSE)
        hit = search(name)
    if not hit:
        return {"status": "not-found", "error": f"no Modrinth hit for {name!r}"}
    pid = hit["project_id"]
    if pid not in cache:
        time.sleep(PAUSE)
        try:
            cache[pid] = pick_file(project_versions(pid)) or {}
        except Exception as e:  # noqa: BLE001
            cache[pid] = {"error": str(e)}
    cached = cache[pid]
    if "error" in cached:
        return {"status": "api-error", "error": cached["error"]}
    if not cached:
        return {
            "status": "no-file",
            "error": f"no {GAME_VERSIONS[0]}/{','.join(LOADERS)} file",
        }
    return {
        "status": "found",
        "project_id": pid,
        "slug": hit["slug"],
        "title": hit.get("title", name),
        "filename": cached.get("filename", ""),
        "url": cached["url"],
        "sha1": cached.get("hashes", {}).get("sha1"),
    }


# ---------------------------------------------------------------- port-pack

def run_check(jar: Path, out_dir: Path) -> tuple[dict | None, str]:
    """horizon-port-pack check <jar> <out> -> (check.json, stderr-ish error)."""
    if not EXE.is_file():
        return None, f"missing {EXE}"
    out_dir.mkdir(parents=True, exist_ok=True)
    try:
        p = subprocess.run(
            [str(EXE), "check", str(jar), str(out_dir)],
            capture_output=True, text=True, timeout=300,
        )
    except subprocess.TimeoutExpired:
        return None, "horizon-port-pack check timed out"
    check = out_dir / "check.json"
    if p.returncode == 0 and check.is_file():
        return json.loads(check.read_text(encoding="utf-8")), ""
    return None, (p.stderr or p.stdout or f"exit code {p.returncode}").strip()


# ---------------------------------------------------------------- report

def md_table(rows: list[dict]) -> str:
    cols = [
        ("mod", lambda m: m["name"]),
        ("slug", lambda m: m.get("slug", "")),
        ("jar", lambda m: "downloaded" if m.get("jar") else "-"),
        ("verdict", lambda m: m.get("verdict", m.get("status", ""))),
        ("ports", lambda m: "; ".join(m.get("ports", []))),
        ("skipped", lambda m: "; ".join(m.get("skipped", []))),
        ("note", lambda m: m.get("error", m.get("summary", ""))),
    ]
    out = ["| " + " | ".join(c[0] for c in cols) + " |",
           "|" + "---|" * len(cols)]
    for m in rows:
        cells = [fn(m).replace("|", "\\|").replace("\n", " ") for _, fn in cols]
        out.append("| " + " | ".join(cells) + " |")
    return "\n".join(out)


def write_report(out: Path, mods: list[dict], dry_run: bool) -> None:
    out.mkdir(parents=True, exist_ok=True)
    data = {
        "generated": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "game_versions": GAME_VERSIONS,
        "loaders": LOADERS,
        "dry_run": dry_run,
        "count": len(mods),
        "summary": {
            s: sum(1 for m in mods if m.get("verdict", m.get("status")) == s)
            for s in ["found", "not-found", "no-file", "api-error",
                      "downloaded", "Works", "Partly", "Doesn't run"]
        },
        "mods": mods,
    }
    (out / "port-report.json").write_text(
        json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    title = "Horizon port report" + (" (dry run: search only)" if dry_run else "")
    md = [
        f"# {title}",
        "",
        f"- Game: {GAME_VERSIONS[0]}, loader: {LOADERS[0]}",
        f"- Mods: {len(mods)}",
        f"- Found on Modrinth: {data['summary']['found']}",
        f"- Jars downloaded: {data['summary']['downloaded']}",
        "- Works / Partly / Doesn't run: {} / {} / {}".format(
            data["summary"]["Works"],
            data["summary"]["Partly"],
            data["summary"]["Doesn't run"],
        ),
        "",
        md_table(mods),
        "",
    ]
    (out / "PORT-REPORT.md").write_text("\n".join(md), encoding="utf-8", newline="\n")


# ---------------------------------------------------------------- main

def load_list(path: str | None) -> list[dict]:
    """--list JSON: [{name, slug}] or {"mods": [...]}. Falls back to default."""
    if path:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
        mods = data["mods"] if isinstance(data, dict) else data
        return [{"name": m["name"], "slug": m.get("slug", "auto-search")} for m in mods]
    return DEFAULT_MODS


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Port a Modrinth mod list (MC 1.21.1 / NeoForge) to Horizon "
                    "via horizon-port-pack."
    )
    p.add_argument("--list", metavar="FILE",
                   help="JSON list of mods: [{name, slug}]; default is built in")
    p.add_argument("--dry-run", action="store_true",
                   help="search and report only, no download, no check")
    p.add_argument("--limit", type=int, metavar="N",
                   help="only the first N mods of the list")
    p.add_argument("--mods", metavar="m1,m2",
                   help="only these mods (by name or slug, comma-separated)")
    p.add_argument("--out", metavar="DIR", default=DEFAULT_OUT,
                   help=f"output dir (default {DEFAULT_OUT})")
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    mods = load_list(args.list)
    if args.mods:
        wanted = {s.strip().lower() for s in args.mods.split(",") if s.strip()}
        mods = [m for m in mods
                if m["name"].lower() in wanted or m["slug"].lower() in wanted]
    if args.limit:
        mods = mods[:args.limit]
    out = Path(args.out)
    work = out / "work"
    checks = out / "check"
    if not args.dry_run:
        work.mkdir(parents=True, exist_ok=True)

    print(f"{len(mods)} mods, game {GAME_VERSIONS[0]}, "
          f"loader {LOADERS[0]}, dry-run={args.dry_run}")
    cache: dict[str, dict] = {}
    results: list[dict] = []
    for i, mod in enumerate(mods, 1):
        print(f"[{i}/{len(mods)}] {mod['name']} (slug: {mod['slug']})")
        row: dict = {"name": mod["name"]}
        try:
            r = resolve(mod, cache)
        except Exception as e:  # noqa: BLE001 - keep going, report it
            r = {"status": "api-error", "error": str(e)}
        row.update(r)
        print(f"    -> {row.get('status')}"
              + (f" ({row['error']})" if row.get("error") else ""))

        if args.dry_run or row["status"] != "found":
            results.append(row)
            continue

        jar = work / f"{row['slug']}.jar"
        ok, err = download(row["url"], jar, row.get("sha1"))
        if not ok:
            row["status"] = "download-error"
            row["error"] = err
            print(f"    !! {err}")
            results.append(row)
            continue
        row["jar"] = str(jar)
        row["status"] = "downloaded"
        print(f"    downloaded {row['filename']}")

        check, err = run_check(jar, checks / row["slug"])
        if check is None:
            row["verdict"] = "check-error"
            row["error"] = err
            print(f"    !! check: {err}")
        else:
            row["verdict"] = check.get("verdict", "?")
            row["summary"] = check.get("summary", "")
            row["ports"] = check.get("ports", [])
            row["skipped"] = check.get("skipped", [])
            print(f"    {check.get('verdict')}: {check.get('summary')}")
        results.append(row)
        time.sleep(PAUSE)

    write_report(out, results, args.dry_run)
    print(f"report: {out / 'PORT-REPORT.md'}")
    print(f"json:   {out / 'port-report.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
