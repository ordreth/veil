//! Smoke test: the crate is usable as a library from `tests/` without a compositor.

use clap::Parser;
use std::path::Path;
use veil::config::{Cli, Command, DaemonArgs};
use veil::engine::media::{self, Content, MediaKind};
use veil::engine::WallpaperEngine;
use veil::error::VeilError;
use veil::ipc::protocol::Request;

#[test]
fn library_modules_are_reachable() {
    // What a file is, is read from its content; its name is only a hint.
    assert_eq!(
        media::sniff(&[0x1A, 0x45, 0xDF, 0xA3]),
        Content::Video("Matroska or WebM")
    );
    assert_eq!(
        media::extension_hint(Path::new("still.png")),
        Some(MediaKind::Image)
    );

    let json = serde_json::to_string(&Request::Ping {}).expect("Request::Ping serializes");
    assert_eq!(json, r#"{"cmd":"ping"}"#);

    // The engine trait is part of the public API.
    fn _accepts_engine(_: &dyn WallpaperEngine) {}
}

#[test]
fn run_without_command_is_rejected() {
    let cli = Cli {
        daemon: false,
        output: None,
        input: None,
        command: None,
    };
    assert!(matches!(veil::run(cli), Err(VeilError::MissingCommand)));
}

#[test]
fn daemon_needs_neither_output_nor_input() {
    // Only parsed and resolved: `veil::run` would connect to a real compositor.
    let cli = Cli::try_parse_from(["veil", "daemon"]).expect("`veil daemon` parses");
    assert_eq!(
        cli.resolve().expect("resolves"),
        Command::Daemon(DaemonArgs::default())
    );
}
