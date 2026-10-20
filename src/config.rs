use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum, ValueHint};

use crate::error::VeilError;
use crate::policy::PauseOn;
use crate::scaling::{Color, ScaleMode};

/// Minimalist dynamic and static wallpaper daemon for Wayland/wlroots (Hyprland)
///
/// `veil daemon` shows a picture or a video as the wallpaper of each output (monitor) of a Wayland compositor with the layer-shell protocol, such as Hyprland or sway. Each output can have a wallpaper of its own, and what each output shows is saved and shown again after a restart.
///
/// The other commands control a running daemon through its control socket: `veil set` changes the wallpaper, `veil clear` shows only a colour, `veil pause` and `veil resume` stop and play videos, `veil status` and `veil outputs` report. `veil config check` and `veil completions` need no daemon.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "veil",
    version,
    about,
    // Hides the bare `veil` form, which only exists for the deprecated `--daemon` flag.
    override_usage = "veil <COMMAND>",
    args_conflicts_with_subcommands = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Deprecated alias for `veil daemon`, kept for one release.
    #[arg(long, hide = true)]
    pub daemon: bool,

    /// Deprecated: `--output` of the `--daemon` alias.
    #[arg(short, long, hide = true, requires = "daemon")]
    pub output: Option<String>,

    /// Deprecated: `--input` of the `--daemon` alias.
    #[arg(short, long, hide = true, requires = "daemon")]
    pub input: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonArgs {
    /// Output (monitor) to draw on, e.g. HDMI-A-1. Repeat it for several outputs. Without it the daemon draws on the outputs of the config file's `outputs`, else on every output, also on those that are connected later.
    #[arg(short, long, value_name = "OUTPUT", value_hint = ValueHint::Other)]
    pub output: Vec<String>,

    /// Wallpaper of every output, over the config file's: a video or an image, told apart by the content of the file. It is saved like a `veil set`. Without it each output shows its wallpaper from the config file, else the one it saved before (see `veil set`), else the --bg-color background.
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    pub input: Option<String>,

    /// How the wallpaper is fitted to the output. Also the default for `veil set`. For a video, `tile` acts as `center` [default: the config file's, else fill].
    #[arg(long, value_enum)]
    pub mode: Option<ScaleMode>,

    /// Colour (#rrggbb) of areas the wallpaper does not cover, and of the background shown without a wallpaper. Also the default for `veil set` [default: the config file's, else #000000].
    #[arg(long, value_name = "COLOR", value_hint = ValueHint::Other)]
    pub bg_color: Option<Color>,

    /// When a video wallpaper is paused because it cannot be seen: `never`, `fullscreen` (a fullscreen window on its output) or `covered` (also windows that cover the whole output, translucent ones included). Needs a compositor whose IPC veil follows (Hyprland, sway); elsewhere videos always play [default: the config file's, else fullscreen].
    #[arg(long, value_enum, value_name = "WHEN")]
    pub pause_on: Option<PauseOn>,

    /// Seconds a video may go without the compositor taking its frames (the screen is locked, the monitor is off) before it is stopped and its pipeline released; it starts again from the beginning when its output shows it again. 0 never stops it [default: the config file's, else 2].
    #[arg(long, value_name = "SECONDS", value_hint = ValueHint::Other)]
    pub release_unseen_after: Option<u64>,

    /// Configuration file to read instead of $XDG_CONFIG_HOME/veil/config.toml. It must exist. Read once, at start-up.
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath)]
    pub config: Option<PathBuf>,
}

/// The default of `--release-unseen-after`, in seconds.
pub const RELEASE_UNSEEN_AFTER: u64 = 2;

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct SetArgs {
    /// Output to change; it must be one the daemon draws on (or waits for). Without it, every output changes, and outputs connected later show the new wallpaper too.
    #[arg(short, long, value_hint = ValueHint::Other)]
    pub output: Option<String>,

    /// Path to the new wallpaper: a video or an image, told apart by the content of the file.
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    pub input: String,

    /// How the wallpaper is fitted to the output [default: the daemon's --mode].
    #[arg(long, value_enum)]
    pub mode: Option<ScaleMode>,

    /// Background colour (#rrggbb) [default: the daemon's --bg-color].
    #[arg(long, value_name = "COLOR", value_hint = ValueHint::Other)]
    pub bg_color: Option<Color>,

    /// Show the wallpaper until the daemon stops, without saving it: a restarted daemon shows the wallpaper saved before.
    #[arg(long)]
    pub no_persist: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct ClearArgs {
    /// Output to clear; it must be one the daemon draws on (or waits for). Without it, every output is cleared, and outputs connected later show the background colour too.
    #[arg(short, long, value_hint = ValueHint::Other)]
    pub output: Option<String>,

    /// Colour (#rrggbb) to show [default: the daemon's --bg-color].
    #[arg(long, value_name = "COLOR", value_hint = ValueHint::Other)]
    pub bg_color: Option<Color>,

    /// Clear until the daemon stops, without saving it: a restarted daemon shows the wallpaper saved before.
    #[arg(long)]
    pub no_persist: bool,
}

/// The output a command is for.
#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct OutputArgs {
    /// Output it is for; it must be one the daemon draws on. Without it, every output the daemon draws on.
    #[arg(short, long, value_hint = ValueHint::Other)]
    pub output: Option<String>,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct ReportArgs {
    /// Print the answer as JSON, for scripts and status bars.
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the wallpaper daemon.
    Daemon(DaemonArgs),

    /// Change the wallpaper of a running daemon. It is saved (in $XDG_STATE_HOME/veil/state.json), and a daemon started without --input shows it again.
    Set(SetArgs),

    /// Show only the background colour, and let go of the wallpaper's file. Saved like `veil set`.
    Clear(ClearArgs),

    /// Pause video wallpapers until `veil resume`, whatever is on screen. Not saved: a restarted daemon plays.
    Pause(OutputArgs),

    /// End a `veil pause`. A video that a window hides stays paused (see `veil daemon --pause-on`).
    Resume(OutputArgs),

    /// `veil resume` if the outputs are paused by `veil pause`, `veil pause` otherwise. Prints `paused` or `playing`.
    Toggle(OutputArgs),

    /// Show what the daemon shows on each output.
    Status(ReportArgs),

    /// List the outputs the compositor has, and which ones the daemon draws on.
    Outputs(ReportArgs),

    /// Stop the daemon. Returns once it has ended. Under systemd, use `systemctl --user stop veil`.
    #[command(visible_alias = "stop")]
    Quit,

    /// Check that a daemon is running: prints `pong`, or exits with status 3.
    Ping,

    /// The configuration file ($XDG_CONFIG_HOME/veil/config.toml).
    Config(ConfigArgs),

    /// Print the completion script of a shell.
    ///
    /// The packages install the scripts where the shells look for them. To install one by hand (create the directory first if it does not exist):
    ///
    /// bash: veil completions bash > ~/.local/share/bash-completion/completions/veil
    ///
    /// zsh: veil completions zsh > ~/.zfunc/_veil, with fpath+=(~/.zfunc) before compinit in ~/.zshrc
    ///
    /// fish: veil completions fish > ~/.config/fish/completions/veil.fish
    ///
    /// nushell: mkdir ($nu.user-autoload-dirs | first); veil completions nushell | save -f ($nu.user-autoload-dirs | first | path join veil.nu)
    Completions(CompletionsArgs),

    /// Write the man pages (veil.1, veil-set.1, ...) into a directory. For
    /// packaging: the packages run it when they are built.
    #[command(hide = true)]
    ManPages(ManPagesArgs),
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct CompletionsArgs {
    /// The shell to complete in.
    #[arg(value_enum)]
    pub shell: Shell,
}

/// A shell `veil completions` writes a script for.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    #[value(alias = "nu")]
    Nushell,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct ManPagesArgs {
    /// Directory to write them into; it is created if needed.
    #[arg(value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub dir: PathBuf,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum ConfigCommand {
    /// Read the configuration file as `veil daemon` would, check that its wallpapers can be shown, and print what the daemon would start with. Exits with status 2 if the file is wrong, 1 if a wallpaper cannot be shown. Needs no daemon.
    Check(ConfigCheckArgs),
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct ConfigCheckArgs {
    /// Configuration file to check [default: $XDG_CONFIG_HOME/veil/config.toml].
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath)]
    pub config: Option<PathBuf>,
}

impl Command {
    /// Whether a failure of this command is also shown as a desktop
    /// notification. Not for commands that only ask (`status`, `outputs`,
    /// `ping`): scripts and status bars run them, often in a loop, and read
    /// the exit status. Not for `config check` either, which is run in a
    /// terminal, nor for `completions` and `man-pages`, which are run in a
    /// terminal or by a package build. The others are often run from a key
    /// binding, with no terminal to read.
    pub fn notifies_errors(&self) -> bool {
        !matches!(
            self,
            Command::Status(_)
                | Command::Outputs(_)
                | Command::Ping
                | Command::Config(_)
                | Command::Completions(_)
                | Command::ManPages(_)
        )
    }
}

impl Cli {
    pub fn parse_args() -> Self {
        Cli::parse()
    }

    /// Whether a failure is also shown as a desktop notification (see
    /// [`Command::notifies_errors`]).
    pub fn notifies_errors(&self) -> bool {
        self.command.as_ref().is_none_or(Command::notifies_errors)
    }

    /// Returns the command to run, mapping the deprecated top-level
    /// `--daemon [--output X] [--input Y]` form to `Command::Daemon`.
    pub fn resolve(self) -> Result<Command, VeilError> {
        match self.command {
            Some(command) => Ok(command),
            None if self.daemon => {
                tracing::warn!("`veil --daemon` is deprecated; use `veil daemon` instead");
                Ok(Command::Daemon(DaemonArgs {
                    output: self.output.into_iter().collect(),
                    input: self.input,
                    ..DaemonArgs::default()
                }))
            }
            None => Err(VeilError::MissingCommand),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    fn parse(args: &[&str]) -> Result<Command, VeilError> {
        Cli::try_parse_from(args)
            .expect("valid command line")
            .resolve()
    }

    #[test]
    fn daemon_subcommand_needs_no_arguments() {
        // No hardcoded output and no required input.
        assert_eq!(
            parse(&["veil", "daemon"]).expect("resolves"),
            Command::Daemon(DaemonArgs::default())
        );
    }

    #[test]
    fn daemon_subcommand_accepts_output_and_input() {
        let expected = Command::Daemon(DaemonArgs {
            output: vec!["HDMI-A-1".into()],
            input: Some("a.mp4".into()),
            ..DaemonArgs::default()
        });
        assert_eq!(
            parse(&["veil", "daemon", "-o", "HDMI-A-1", "-i", "a.mp4"]).expect("resolves"),
            expected
        );
        assert_eq!(
            parse(&["veil", "daemon", "--output", "HDMI-A-1", "--input", "a.mp4"])
                .expect("resolves"),
            expected
        );
    }

    #[test]
    fn daemon_accepts_several_outputs() {
        let Command::Daemon(args) =
            parse(&["veil", "daemon", "-o", "DP-3", "--output", "eDP-1"]).expect("resolves")
        else {
            panic!("expected the daemon command");
        };
        assert_eq!(args.output, ["DP-3", "eDP-1"]);
        // `veil set` changes one output, or every output.
        assert!(Cli::try_parse_from(["veil", "set", "-i", "a.png", "-o", "A", "-o", "B"]).is_err());
    }

    #[test]
    fn deprecated_daemon_flag_maps_to_daemon_subcommand() {
        assert_eq!(
            parse(&["veil", "--daemon"]).expect("resolves"),
            Command::Daemon(DaemonArgs::default())
        );
        assert_eq!(
            parse(&["veil", "--daemon", "--output", "eDP-1", "--input", "a.png"])
                .expect("resolves"),
            Command::Daemon(DaemonArgs {
                output: vec!["eDP-1".into()],
                input: Some("a.png".into()),
                ..DaemonArgs::default()
            })
        );
    }

    #[test]
    fn daemon_flags_that_are_not_given_are_none() {
        // So that the configuration file can fill them (`config_file::settings`,
        // which has the built-in defaults).
        let Command::Daemon(args) = parse(&["veil", "daemon"]).expect("resolves") else {
            panic!("expected the daemon command");
        };
        assert_eq!(args.mode, None);
        assert_eq!(args.bg_color, None);
        assert_eq!(args.pause_on, None);
        assert_eq!(args.release_unseen_after, None);
        assert_eq!(args.config, None);
        let settings = crate::config_file::settings(&args, None);
        assert_eq!(settings.defaults.mode, ScaleMode::Fill);
        assert_eq!(settings.defaults.bg_color, Color { r: 0, g: 0, b: 0 });
        assert_eq!(settings.policy.pause_on, PauseOn::Fullscreen);
        assert_eq!(
            settings.policy.release_unseen_after,
            Some(std::time::Duration::from_secs(RELEASE_UNSEEN_AFTER))
        );
    }

    #[test]
    fn daemon_accepts_mode_and_bg_color() {
        let Command::Daemon(args) =
            parse(&["veil", "daemon", "--mode", "tile", "--bg-color", "#102030"])
                .expect("resolves")
        else {
            panic!("expected the daemon command");
        };
        assert_eq!(args.mode, Some(ScaleMode::Tile));
        assert_eq!(
            args.bg_color,
            Some(Color {
                r: 0x10,
                g: 0x20,
                b: 0x30
            })
        );
    }

    #[test]
    fn every_mode_name_parses() {
        for (name, mode) in [
            ("fill", ScaleMode::Fill),
            ("fit", ScaleMode::Fit),
            ("stretch", ScaleMode::Stretch),
            ("center", ScaleMode::Center),
            ("tile", ScaleMode::Tile),
        ] {
            let Command::Daemon(args) =
                parse(&["veil", "daemon", "--mode", name]).expect("resolves")
            else {
                panic!("expected the daemon command");
            };
            assert_eq!(args.mode, Some(mode), "{name}");
        }
    }

    #[test]
    fn daemon_accepts_the_pause_flags_and_a_config_file() {
        let Command::Daemon(args) = parse(&[
            "veil",
            "daemon",
            "--release-unseen-after",
            "0",
            "--config",
            "/etc/veil.toml",
        ])
        .expect("resolves") else {
            panic!("expected the daemon command");
        };
        assert_eq!(args.release_unseen_after, Some(0));
        assert_eq!(args.config, Some(PathBuf::from("/etc/veil.toml")));
        for (name, pause_on) in [
            ("never", PauseOn::Never),
            ("fullscreen", PauseOn::Fullscreen),
            ("covered", PauseOn::Covered),
        ] {
            let Command::Daemon(args) =
                parse(&["veil", "daemon", "--pause-on", name]).expect("resolves")
            else {
                panic!("expected the daemon command");
            };
            assert_eq!(args.pause_on, Some(pause_on), "{name}");
        }
    }

    #[test]
    fn config_check_takes_an_optional_file() {
        assert_eq!(
            parse(&["veil", "config", "check"]).expect("resolves"),
            Command::Config(ConfigArgs {
                command: ConfigCommand::Check(ConfigCheckArgs { config: None }),
            })
        );
        assert_eq!(
            parse(&["veil", "config", "check", "--config", "a.toml"]).expect("resolves"),
            Command::Config(ConfigArgs {
                command: ConfigCommand::Check(ConfigCheckArgs {
                    config: Some(PathBuf::from("a.toml")),
                }),
            })
        );
        assert!(Cli::try_parse_from(["veil", "config"]).is_err());
        assert!(Cli::try_parse_from(["veil", "config", "reload"]).is_err());
        assert!(Cli::try_parse_from(["veil", "set", "-i", "a.png", "--config", "a.toml"]).is_err());
    }

    #[test]
    fn set_subcommand_requires_input() {
        assert!(Cli::try_parse_from(["veil", "set"]).is_err());
        assert_eq!(
            parse(&["veil", "set", "-i", "a.png"]).expect("resolves"),
            Command::Set(SetArgs {
                output: None,
                input: "a.png".into(),
                mode: None,
                bg_color: None,
                no_persist: false,
            })
        );
    }

    #[test]
    fn set_subcommand_accepts_mode_and_bg_color() {
        assert_eq!(
            parse(&[
                "veil",
                "set",
                "-i",
                "a.png",
                "--mode",
                "fit",
                "--bg-color",
                "ffffff"
            ])
            .expect("resolves"),
            Command::Set(SetArgs {
                output: None,
                input: "a.png".into(),
                mode: Some(ScaleMode::Fit),
                bg_color: Some(Color {
                    r: 255,
                    g: 255,
                    b: 255
                }),
                no_persist: false,
            })
        );
    }

    #[test]
    fn set_saves_unless_told_not_to() {
        let Command::Set(args) = parse(&["veil", "set", "-i", "a.png"]).expect("resolves") else {
            panic!("expected the set command");
        };
        assert!(!args.no_persist);
        let Command::Set(args) =
            parse(&["veil", "set", "-i", "a.png", "--no-persist"]).expect("resolves")
        else {
            panic!("expected the set command");
        };
        assert!(args.no_persist);
        // The daemon has no such flag.
        assert!(Cli::try_parse_from(["veil", "daemon", "--no-persist"]).is_err());
    }

    #[test]
    fn the_control_commands_parse() {
        let output = |name: Option<&str>| OutputArgs {
            output: name.map(Into::into),
        };
        for (args, expected) in [
            (&["veil", "pause"][..], Command::Pause(output(None))),
            (
                &["veil", "pause", "-o", "DP-3"],
                Command::Pause(output(Some("DP-3"))),
            ),
            (&["veil", "resume"], Command::Resume(output(None))),
            (
                &["veil", "resume", "--output", "DP-3"],
                Command::Resume(output(Some("DP-3"))),
            ),
            (&["veil", "toggle"], Command::Toggle(output(None))),
            (
                &["veil", "toggle", "-o", "DP-3"],
                Command::Toggle(output(Some("DP-3"))),
            ),
            (
                &["veil", "status"],
                Command::Status(ReportArgs { json: false }),
            ),
            (
                &["veil", "status", "--json"],
                Command::Status(ReportArgs { json: true }),
            ),
            (
                &["veil", "outputs"],
                Command::Outputs(ReportArgs { json: false }),
            ),
            (
                &["veil", "outputs", "--json"],
                Command::Outputs(ReportArgs { json: true }),
            ),
            (&["veil", "quit"], Command::Quit),
            (&["veil", "stop"], Command::Quit),
            (&["veil", "ping"], Command::Ping),
            (
                &["veil", "clear"],
                Command::Clear(ClearArgs {
                    output: None,
                    bg_color: None,
                    no_persist: false,
                }),
            ),
            (
                &[
                    "veil",
                    "clear",
                    "-o",
                    "DP-3",
                    "--bg-color",
                    "#102030",
                    "--no-persist",
                ],
                Command::Clear(ClearArgs {
                    output: Some("DP-3".into()),
                    bg_color: Some(Color {
                        r: 0x10,
                        g: 0x20,
                        b: 0x30,
                    }),
                    no_persist: true,
                }),
            ),
        ] {
            assert_eq!(parse(args).expect("resolves"), expected, "{args:?}");
        }
        for args in [
            &["veil", "quit", "-o", "DP-3"][..],
            &["veil", "ping", "--json"],
            &["veil", "pause", "--json"],
            &["veil", "status", "-o", "DP-3"],
            &["veil", "clear", "-i", "a.png"],
            &["veil", "pause", "-o", "A", "-o", "B"],
        ] {
            assert!(Cli::try_parse_from(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn completions_take_a_shell_and_man_pages_a_directory() {
        for (name, shell) in [
            ("bash", Shell::Bash),
            ("zsh", Shell::Zsh),
            ("fish", Shell::Fish),
            ("nushell", Shell::Nushell),
            ("nu", Shell::Nushell),
        ] {
            assert_eq!(
                parse(&["veil", "completions", name]).expect("resolves"),
                Command::Completions(CompletionsArgs { shell }),
                "{name}"
            );
        }
        assert_eq!(
            parse(&["veil", "man-pages", "share/man/man1"]).expect("resolves"),
            Command::ManPages(ManPagesArgs {
                dir: PathBuf::from("share/man/man1"),
            })
        );
        for args in [
            &["veil", "completions"][..],
            &["veil", "completions", "powershell"],
            &["veil", "man-pages"],
        ] {
            assert!(Cli::try_parse_from(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn only_commands_that_act_notify_their_errors() {
        for (args, notifies) in [
            (&["veil", "set", "-i", "a.png"][..], true),
            (&["veil", "clear"], true),
            (&["veil", "pause"], true),
            (&["veil", "resume"], true),
            (&["veil", "toggle"], true),
            (&["veil", "quit"], true),
            (&["veil", "daemon"], true),
            (&["veil", "--daemon"], true),
            (&["veil", "status"], false),
            (&["veil", "outputs", "--json"], false),
            (&["veil", "ping"], false),
            (&["veil", "config", "check"], false),
            (&["veil", "completions", "bash"], false),
            (&["veil", "man-pages", "/tmp/man"], false),
        ] {
            let cli = Cli::try_parse_from(args).expect("valid command line");
            assert_eq!(cli.notifies_errors(), notifies, "{args:?}");
        }
    }

    #[test]
    fn bare_invocation_prints_help() {
        let err = Cli::try_parse_from(["veil"]).expect_err("bare invocation is a usage error");
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn cli_without_command_does_not_resolve() {
        // Unreachable from the command line (clap prints help first), but `Cli` is
        // public and can be built directly.
        let cli = Cli {
            daemon: false,
            output: None,
            input: None,
            command: None,
        };
        assert!(matches!(cli.resolve(), Err(VeilError::MissingCommand)));
    }

    #[test]
    fn invalid_command_lines_are_rejected() {
        for args in [
            // Top-level --output/--input only exist for the deprecated --daemon alias.
            &["veil", "--output", "DP-1"][..],
            &["veil", "--input", "a.png"],
            // The deprecated flag cannot be combined with a subcommand.
            &["veil", "--daemon", "daemon"],
            &["veil", "--daemon", "set", "-i", "a.png"],
            &["veil", "daemon", "--daemon"],
            &["veil", "frobnicate"],
            // Unknown mode, malformed colour, and the new flags on the deprecated alias.
            &["veil", "daemon", "--mode", "zoom"],
            &["veil", "daemon", "--bg-color", "red"],
            &["veil", "set", "-i", "a.png", "--bg-color", "#fff"],
            &["veil", "--daemon", "--mode", "fit"],
            // Unknown pause level; `veil set` has none.
            &["veil", "daemon", "--pause-on", "hidden"],
            &["veil", "daemon", "--release-unseen-after", "-1"],
            &["veil", "daemon", "--release-unseen-after", "1.5"],
            &["veil", "set", "-i", "a.png", "--pause-on", "never"],
        ] {
            assert!(Cli::try_parse_from(args).is_err(), "{args:?}");
        }
    }
}
