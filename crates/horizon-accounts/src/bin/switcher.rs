//! EN: `horizon-switcher` — CLI front-end for `horizon-accounts`.
//! Ported from `horizon-account-switcher.py`'s `main`.
//!
//! Usage:
//!   horizon-switcher add [--client-id ID]           sign in a new account (device code)
//!   horizon-switcher list                           list stored accounts
//!   horizon-switcher switch NAME_OR_UUID [--server HOST[:PORT]] [--game PATH] [--resources PATH]
//!   horizon-switcher session NAME_OR_UUID           only write the session file, print path
//!   horizon-switcher remove NAME_OR_UUID            remove an account
//!
//! The MSA client id comes from `--client-id` or the `HORIZON_MSA_CLIENT_ID`
//! env var; Horizon upstream keeps its own id private, so you must bring
//! your own Azure app registration approved by Mojang.
//!
//! RU: CLI-обёртка над библиотекой. client_id берётся из `--client-id` или
//! переменной окружения `HORIZON_MSA_CLIENT_ID` (своего id нет — апстрим
//! держит приватным; нужна регистрация Azure-приложения, одобренная Mojang).

use horizon_accounts::msa;
use horizon_accounts::profile;
use horizon_accounts::session_file;
use horizon_accounts::store::{Account, AccountStore};
use horizon_accounts::xbox;
use horizon_accounts::{default_http, AcctError};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_GAME: &str = r"C:\Users\kwent\horizon-release\horizon-client.exe";
const DEFAULT_RESOURCES: &str = r"C:\Users\kwent\horizon-rust\.local\horizon-client\26.3";

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn local_appdata() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // fall back to the user profile, like the Python original
            let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
            PathBuf::from(home)
        })
}

fn store_path() -> PathBuf {
    local_appdata().join("horizon-switcher").join("accounts.json")
}

fn game_home() -> PathBuf {
    local_appdata().join("horizon")
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cmd = argv.first().cloned().unwrap_or_else(|| "list".into());
    let args = &argv[1.min(argv.len())..];

    let mut client_id = std::env::var("HORIZON_MSA_CLIENT_ID").unwrap_or_default();
    let mut server: Option<String> = None;
    let mut game: Option<String> = None;
    let mut resources: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--client-id" | "--server" | "--game" | "--resources" => {
                let val = args.get(i + 1).cloned().unwrap_or_default();
                match args[i].as_str() {
                    "--client-id" => client_id = val,
                    "--server" => server = Some(val),
                    "--game" => game = Some(val),
                    _ => resources = Some(val),
                }
                i += 2;
            }
            a if a.starts_with("--") => {
                i += 1; // unknown flag: ignore (forward-compat, as in the Python CLI)
            }
            a => {
                positional.push(a.to_string());
                i += 1;
            }
        }
    }

    let result = match cmd.as_str() {
        "add" => cmd_add(client_id),
        "list" => cmd_list(),
        "switch" => {
            let Some(target) = positional.first().cloned() else {
                exit_err("switch NAME_OR_UUID [--server host:port] [--game PATH] [--resources PATH]");
            };
            let game = game.unwrap_or_else(|| DEFAULT_GAME.into());
            let res = resources.unwrap_or_else(|| DEFAULT_RESOURCES.into());
            cmd_switch(target, server, game, res)
        }
        "session" => {
            let Some(target) = positional.first().cloned() else {
                exit_err("session NAME_OR_UUID");
            };
            cmd_session(target)
        }
        "remove" => {
            let Some(target) = positional.first().cloned() else {
                exit_err("remove NAME_OR_UUID");
            };
            cmd_remove(target)
        }
        _ => {
            print_usage();
            Ok(())
        }
    };

    if let Err(e) = result {
        exit_err(&e.to_string());
    }
}

fn exit_err(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

fn print_usage() {
    println!(
        "horizon-switcher — account switcher for the Horizon client

usage:
  add [--client-id ID]     sign in a new account (MSA device code)
  list                     list stored accounts
  switch NAME_OR_UUID [--server HOST[:PORT]] [--game PATH] [--resources PATH]
                           refresh token, write session file, launch the game
  session NAME_OR_UUID     only write the session file and print its path
  remove NAME_OR_UUID      remove an account

env:
  HORIZON_MSA_CLIENT_ID    default MSA client id (Azure app approved by Mojang)

defaults:
  game:      {DEFAULT_GAME}
  resources: {DEFAULT_RESOURCES}
  store:     %LOCALAPPDATA%\\horizon-switcher\\accounts.json
  sessions:  %LOCALAPPDATA%\\horizon\\sessions"
    );
}

/// EN: load the store from the default path. RU: загрузить хранилище.
fn open_store() -> Result<AccountStore, AcctError> {
    AccountStore::load(&store_path())
}

/// RU: как `ensure_token` из Python-версии — вернуть живой MC-токен,
/// при необходимости обновив его через refresh-цепочку.
fn ensure_token(a: &mut Account) -> Result<(), AcctError> {
    if a.mc_token_fresh(now()) {
        return Ok(());
    }
    let http = default_http();
    let msa = msa::refresh(http.as_ref(), &a.client_id, &a.refresh_token)?;
    a.refresh_token = msa.refresh;
    let (mc, xuid) = xbox::minecraft_token(http.as_ref(), &msa.access)?;
    a.mc_token = mc;
    a.xuid = xuid;
    a.mc_expiry = now() + 86400;
    Ok(())
}

fn cmd_add(client_id: String) -> Result<(), AcctError> {
    if client_id.is_empty() {
        return Err(AcctError::BadField(
            "client-id required: --client-id or HORIZON_MSA_CLIENT_ID (Azure app approved by Mojang)".into(),
        ));
    }
    let http = default_http();
    // CLI may print freely: the notify callback just goes to stdout.
    let tokens = msa::device_code(http.as_ref(), &client_id, &|m| println!("{m}"), &|| false)?;
    let (mc, xuid) = xbox::minecraft_token(http.as_ref(), &tokens.access)?;
    let prof = profile::fetch(http.as_ref(), &mc)?;
    let mut store = open_store()?;
    store.add(Account {
        uuid: prof.uuid,
        name: prof.name,
        skin: prof.skin,
        refresh_token: tokens.refresh,
        mc_token: mc,
        mc_expiry: now() + 86400,
        xuid,
        client_id,
    });
    store.save()?;
    let name = store.accounts.last().map(|a| a.name.as_str()).unwrap_or("?");
    let uuid = store.selected.as_deref().unwrap_or("?");
    println!("Added and selected: {name} ({uuid})");
    Ok(())
}

fn cmd_list() -> Result<(), AcctError> {
    let store = open_store()?;
    if store.accounts.is_empty() {
        println!("No accounts. Run: add --client-id ID");
        return Ok(());
    }
    for a in &store.accounts {
        let mark = if store.selected.as_deref() == Some(a.uuid.as_str()) { "*" } else { " " };
        println!("{mark} {:<20} {}", a.name, a.uuid);
    }
    Ok(())
}

fn cmd_session(target: String) -> Result<(), AcctError> {
    let mut store = open_store()?;
    let mut account = store
        .find(&target)
        .cloned()
        .ok_or_else(|| AcctError::NotFound(target.clone()))?;
    ensure_token(&mut account)?;
    // replace the stored copy with the refreshed one
    if let Some(slot) = store.find_mut(&target) {
        *slot = account.clone();
    }
    store.select(&target)?;
    store.save()?;
    let path = session_file::write(&game_home(), &account, &account.mc_token)?;
    println!("{}", path.display());
    Ok(())
}

fn cmd_switch(target: String, server: Option<String>, game: String, resources: String) -> Result<(), AcctError> {
    let mut store = open_store()?;
    let mut account = store
        .find(&target)
        .cloned()
        .ok_or_else(|| AcctError::NotFound(target.clone()))?;
    ensure_token(&mut account)?;
    if let Some(slot) = store.find_mut(&target) {
        *slot = account.clone();
    }
    store.select(&target)?;
    store.save()?;
    let session = session_file::write(&game_home(), &account, &account.mc_token)?;

    let mut cmd = std::process::Command::new(&game);
    cmd.arg("--session-file").arg(&session);
    if !resources.is_empty() {
        cmd.arg("--resources").arg(&resources);
    }
    if let Some(server) = &server {
        cmd.arg("--server").arg(server);
    }
    if let Some(dir) = Path::new(&game).parent() {
        cmd.current_dir(dir);
    }
    let server_arg = server.as_deref().map(|s| format!(" --server {s}")).unwrap_or_default();
    let resources_arg =
        if resources.is_empty() { String::new() } else { format!(" --resources {resources}") };
    let argline =
        format!("{} --session-file {}{}{}", game, session.display(), server_arg, resources_arg);
    println!("Launching: {argline}");
    cmd.spawn()?;
    Ok(())
}

fn cmd_remove(target: String) -> Result<(), AcctError> {
    let mut store = open_store()?;
    let removed = store.remove(&target)?;
    store.save()?;
    println!("Removed {}", removed.name);
    Ok(())
}
