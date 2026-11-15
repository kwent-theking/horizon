//! Console entry point for the optional friends profile; no GPUI window or self-update.
use clap::Parser;
use horizon_launcher::{account, backend, friends};
use std::io::IsTerminal;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Update, prepare and play the owner's signed friends profile")]
struct Args {
    /// Trusted local configuration (default: friends-launcher.json beside this executable)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Keep the friends snapshots and game home here
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Download and prepare without starting the game
    #[arg(long)]
    update_only: bool,
}

fn sign_in() -> Result<(), String> {
    let client_id = account::client_id().ok_or(account::NOT_CONFIGURED)?;
    let ep = account::Endpoints::live();
    let code = account::device_code(&ep, &client_id)?;
    eprintln!(
        "To sign in, open {} in a browser and enter {}",
        code.verification_uri, code.user_code
    );
    let ms = account::wait_for_sign_in(&ep, &client_id, &code, &|| false)?;
    let (signed_in, secrets) = account::finish_sign_in(&ep, ms)?;
    let mut accounts = account::Accounts::load();
    accounts.add(account::store(), signed_in, &secrets)?;
    accounts.save()
}

fn run(args: Args) -> Result<(), String> {
    let beside = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("Cannot locate friends launcher")?
        .to_path_buf();
    let config = friends::Config::read(
        &args
            .config
            .unwrap_or_else(|| beside.join("friends-launcher.json")),
    )?;
    let root = args
        .data_dir
        .unwrap_or_else(|| backend::data_dir().join("friends"));
    horizon_launcher::proxy::announce(backend::Settings::load().proxy);
    let ready = friends::update(&config, &root, &|line| eprintln!("{line}"))?;
    println!(
        "Friends sequence {} is ready for {}.",
        ready.manifest.sequence, ready.manifest.server
    );
    if !args.update_only {
        let mut cmd = match ready.authenticated_command() {
            Ok(cmd) => cmd,
            Err(why) => {
                eprintln!("{why}");
                sign_in()?;
                ready.authenticated_command()?
            }
        };
        let status = cmd.status().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("Horizon exited with {status}"));
        }
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run(Args::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("Friends launcher: {why}");
            // A double-clicked Windows console stays open so setup errors can be read.
            if cfg!(windows) && std::io::stdin().is_terminal() {
                eprintln!("Press Enter to close.");
                let _ = std::io::stdin().read_line(&mut String::new());
            }
            std::process::ExitCode::FAILURE
        }
    }
}
