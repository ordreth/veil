//! Veil: dynamic (video) and static wallpaper daemon for Wayland compositors.
//!
//! The binary in `src/main.rs` only sets up logging and calls [`run`]; everything
//! else lives in this library so it can be exercised from `tests/` without a
//! running compositor.

pub mod compositor;
pub mod config;
pub mod config_file;
pub mod engine;
pub mod error;
pub mod generate;
pub mod ipc;
pub mod notify;
pub mod persist;
pub mod policy;
pub mod scaling;
pub mod service;
pub mod wayland;

use config::{Cli, Command, ConfigCommand, DaemonArgs};
use error::VeilError;

/// Dispatches a parsed command line: starts the daemon, checks the
/// configuration file, prints the completions or writes the man pages, or sends a command to a running daemon and prints its
/// answer on stdout.
pub fn run(cli: Cli) -> Result<(), VeilError> {
    match cli.resolve()? {
        Command::Daemon(args) => run_daemon(args),
        Command::Config(config) => match config.command {
            ConfigCommand::Check(args) => {
                print!("{}", config_file::check(args.config.as_deref())?);
                Ok(())
            }
        },
        Command::Completions(args) => {
            print!("{}", generate::completions(args.shell)?);
            Ok(())
        }
        Command::ManPages(args) => generate::write_man_pages(&args.dir),
        command => {
            if let Some(text) = ipc::client::run(command)? {
                print!("{text}");
            }
            Ok(())
        }
    }
}

fn run_daemon(args: DaemonArgs) -> Result<(), VeilError> {
    tracing::info!(
        "veil {} (GStreamer {})",
        env!("CARGO_PKG_VERSION"),
        engine::gstreamer_version()
    );
    let file = config_file::find(args.config.as_deref())?;
    match &file {
        Some(file) => tracing::info!("configuration file {}", file.path.display()),
        None => tracing::debug!("no configuration file"),
    }
    let settings = config_file::settings(&args, file.as_ref());
    if settings.input.is_some() && file.as_ref().is_some_and(|file| !file.outputs.is_empty()) {
        tracing::info!("--input is given; the wallpapers of the configuration file are not used");
    }
    wayland::run(settings)
}
