//! starahd: Starah's privilege broker.
//!
//! Agents run as their own system users (`starah-<name>`), sandboxed. When an agent
//! asks the system for something privileged, the system asks polkit, and polkit's
//! Starah rule asks starahd (`Check`) through `starah-polkit-check`. starahd answers
//! from the agent's level, files a request for the human when the level says "ask",
//! and keeps the grants the human approves.
//!
//! usage: starahd [--config /etc/starah/config.json] [--state /var/lib/starah]

mod polkit;
mod rules_file;
mod service;
mod session;
mod systemd;

use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let mut config = PathBuf::from("/etc/starah/config.json");
    let mut state = PathBuf::from("/var/lib/starah");
    // Interactive sessions' root steps, run by starahd as transient units.
    let all: Vec<String> = std::env::args().skip(1).collect();
    match all.first().map(String::as_str) {
        Some("--prepare-session") => std::process::exit(session::prepare_main(&all[1..])),
        Some("--end-session") => std::process::exit(session::end_main(&all[1..])),
        _ => {}
    }
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config = args.next().map(PathBuf::from).unwrap_or(config),
            "--state" => state = args.next().map(PathBuf::from).unwrap_or(state),
            other => {
                eprintln!("starahd: unknown argument {other:?}");
                std::process::exit(2);
            }
        }
    }
    if let Err(e) = service::run(&config, &state).await {
        eprintln!("starahd: {e}");
        std::process::exit(1);
    }
}
