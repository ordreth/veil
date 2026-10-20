//! What is generated from the command line's definition ([`Cli`]): the shell
//! completions (`veil completions <shell>`) and the man pages (`veil
//! man-pages <dir>`, hidden; the packages run it when they are built). Both
//! come from the binary itself, so they describe the binary they ship with
//! and cannot fall behind it.

use std::path::Path;

use clap::CommandFactory;
use clap_complete::Generator;
use clap_mangen::roff::{bold, roman, Roff};
use clap_mangen::Man;

use crate::config::{Cli, Shell};
use crate::error::{
    VeilError, EXIT_ALREADY_RUNNING, EXIT_COMPOSITOR_GONE, EXIT_DAEMON_NOT_RUNNING, EXIT_FAILURE,
    EXIT_USAGE,
};

/// The name the completions complete and the man pages are named after.
const BIN: &str = "veil";

/// The completion script of `shell`.
pub fn completions(shell: Shell) -> Result<String, VeilError> {
    let cmd = completed_command();
    let mut script = Vec::new();
    match shell {
        Shell::Bash => clap_complete::Shell::Bash.try_generate(&cmd, &mut script),
        Shell::Zsh => clap_complete::Shell::Zsh.try_generate(&cmd, &mut script),
        Shell::Fish => clap_complete::Shell::Fish.try_generate(&cmd, &mut script),
        Shell::Nushell => clap_complete_nushell::Nushell.try_generate(&cmd, &mut script),
    }
    .map_err(|source| VeilError::Generate {
        what: format!("the {shell:?} completions"),
        source,
    })?;
    Ok(String::from_utf8_lossy(&script).into_owned())
}

/// The command line as the completions offer it: without what `--help`
/// hides (the deprecated `--daemon` form, `man-pages`), which the generators
/// of `clap_complete` would offer all the same. Only the top level hides
/// anything (a test checks that), so it is the only one built anew.
fn completed_command() -> clap::Command {
    let full = Cli::command();
    let mut cmd = clap::Command::new(BIN)
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(true)
        .args(
            full.get_arguments()
                .filter(|arg| !arg.is_hide_set())
                .cloned(),
        )
        .subcommands(
            full.get_subcommands()
                .filter(|sub| !sub.is_hide_set())
                .cloned(),
        );
    cmd.set_bin_name(BIN);
    cmd.build();
    cmd
}

/// One man page: its file name (`veil-set.1`) and its roff source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManPage {
    pub file_name: String,
    pub text: String,
}

/// The man pages: `veil.1`, and one for every command that is not hidden
/// (`veil-set.1`, `veil-config-check.1`, ...), as `git` and `cargo` have them.
/// `veil.1` also has the exit statuses, the environment and the files, which
/// every command shares; the others point to it.
pub fn man_pages() -> Result<Vec<ManPage>, VeilError> {
    let mut cmd = Cli::command().disable_help_subcommand(true);
    cmd.set_bin_name(BIN);
    cmd.build();
    let mut pages = Vec::new();
    add_pages(&cmd, &mut pages)?;
    Ok(pages)
}

/// Writes the man pages into `dir`, which is created if needed.
pub fn write_man_pages(dir: &Path) -> Result<(), VeilError> {
    let failed = |what: &Path| {
        let what = format!("'{}'", what.display());
        move |source| VeilError::Generate { what, source }
    };
    std::fs::create_dir_all(dir).map_err(failed(dir))?;
    for page in man_pages()? {
        let path = dir.join(&page.file_name);
        std::fs::write(&path, page.text).map_err(failed(&path))?;
    }
    Ok(())
}

/// What `roff` writes before everything it renders (the string `Aq`, an
/// apostrophe); a page needs it once, before its first line.
const ROFF_PREAMBLE: &str = ".ie \\n(.g .ds Aq \\(aq\n.el .ds Aq '\n";

fn add_pages(cmd: &clap::Command, pages: &mut Vec<ManPage>) -> Result<(), VeilError> {
    let man = Man::new(cmd.clone());
    let file_name = man.get_filename();
    let name = cmd.get_display_name().unwrap_or(BIN);
    let failed = |source| VeilError::Generate {
        what: format!("the man page {file_name}"),
        source,
    };

    let mut body = Vec::new();
    man.render_name_section(&mut body).map_err(failed)?;
    man.render_synopsis_section(&mut body).map_err(failed)?;
    man.render_description_section(&mut body).map_err(failed)?;
    if cmd.get_arguments().any(|arg| !arg.is_hide_set()) {
        man.render_options_section(&mut body).map_err(failed)?;
    }
    if cmd.get_subcommands().any(|sub| !sub.is_hide_set()) {
        man.render_subcommands_section(&mut body).map_err(failed)?;
    }
    extra_sections(name).to_writer(&mut body).map_err(failed)?;
    // The title line is written here: `clap_mangen` leaves out an empty date
    // instead of quoting it, and the version then takes its place.
    let text = format!(
        "{ROFF_PREAMBLE}.TH {} 1 \"\" \"{BIN} {}\"\n{}",
        name.to_uppercase(),
        env!("CARGO_PKG_VERSION"),
        String::from_utf8_lossy(&body).replace(ROFF_PREAMBLE, "")
    );
    pages.push(ManPage { file_name, text });

    for sub in cmd.get_subcommands().filter(|sub| !sub.is_hide_set()) {
        add_pages(sub, pages)?;
    }
    Ok(())
}

/// What a page says beyond its command line: on `veil.1` what every command
/// shares, on the others where to find that.
fn extra_sections(name: &str) -> Roff {
    let mut roff = Roff::new();
    if name != BIN {
        if name == "veil-daemon" {
            section(
                &mut roff,
                "SIGNALS",
                &[(
                    "SIGINT, SIGTERM",
                    "The daemon lets go of its surfaces and video pipelines, removes its control \
                     socket and exits with status 0.",
                )],
            );
        }
        roff.control("SH", ["SEE ALSO"]);
        roff.text([
            bold(BIN),
            roman("(1), for the exit statuses, the environment and the files."),
        ]);
        return roff;
    }

    let exit_statuses = [
        (
            0,
            "Success. For the daemon: it was stopped with SIGINT, SIGTERM or veil quit.",
        ),
        (
            EXIT_FAILURE,
            "Any other error. The message is on stderr (the daemon's log).",
        ),
        (
            EXIT_USAGE,
            "The command line cannot be parsed, or the configuration file cannot be read or is \
             wrong (veil daemon, veil config check).",
        ),
        (
            EXIT_DAEMON_NOT_RUNNING,
            "A command for a running daemon (veil set, status, ping, ...): no daemon is running \
             for this compositor.",
        ),
        (
            EXIT_ALREADY_RUNNING,
            "veil daemon: a daemon is already running for this compositor. Nothing was changed.",
        ),
        (
            EXIT_COMPOSITOR_GONE,
            "veil daemon: the compositor went away while the daemon ran.",
        ),
    ]
    .map(|(status, meaning)| (status.to_string(), meaning));
    section(&mut roff, "EXIT STATUS", &exit_statuses);

    section(
        &mut roff,
        "ENVIRONMENT",
        &[
            (
                "XDG_RUNTIME_DIR",
                "Required. The daemon's control socket is there, and a command reaches the daemon \
                 of the compositor it runs in.",
            ),
            (
                "WAYLAND_DISPLAY",
                "The compositor the daemon draws on (wayland-0 if it is not set), and part of the \
                 socket's name.",
            ),
            (
                "XDG_CONFIG_HOME",
                "Where the configuration file is (~/.config if it is not set).",
            ),
            (
                "XDG_STATE_HOME",
                "Where the saved wallpapers are (~/.local/state if it is not set).",
            ),
            (
                "HYPRLAND_INSTANCE_SIGNATURE, SWAYSOCK",
                "The compositor whose IPC the daemon follows to pause videos nobody can see \
                 (veil daemon --pause-on): Hyprland's if the first is set, else sway's. Without \
                 either, videos always play.",
            ),
            (
                "RUST_LOG",
                "Which messages are logged on stderr (default: info).",
            ),
            ("GST_DEBUG", "GStreamer's own log."),
        ],
    );

    section(
        &mut roff,
        "FILES",
        &[
            (
                "$XDG_CONFIG_HOME/veil/config.toml",
                "The configuration file, read by veil daemon when it starts (veil daemon \
                 --config FILE reads another one). veil config check checks it.",
            ),
            (
                "$XDG_STATE_HOME/veil/state.json",
                "The wallpaper of each output, saved by veil set and veil clear, and shown again \
                 by a daemon started without --input.",
            ),
            (
                "$XDG_RUNTIME_DIR/veil-$WAYLAND_DISPLAY.sock",
                "The control socket, and next to it its lock file (.sock.lock), which stays when \
                 the daemon exits. Other programs can use the socket too: one JSON request and one \
                 JSON answer per connection, each on one line, described in docs/ipc.md of veil's \
                 source (the Nix package installs it in share/doc/veil).",
            ),
        ],
    );
    roff
}

/// A section of tagged paragraphs.
fn section(roff: &mut Roff, title: &str, items: &[(impl AsRef<str>, &str)]) {
    roff.control("SH", [title]);
    for (tag, text) in items {
        roff.control("TP", []);
        roff.text([bold(tag.as_ref())]);
        roff.text([roman(*text)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command a user can type, as `veil <words>`.
    const COMMANDS: [&str; 13] = [
        "daemon",
        "set",
        "clear",
        "pause",
        "resume",
        "toggle",
        "status",
        "outputs",
        "quit",
        "ping",
        "config",
        "config check",
        "completions",
    ];

    #[test]
    fn every_shell_completes_every_command_and_nothing_hidden() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::Nushell] {
            let script = completions(shell).expect("a script");
            for command in COMMANDS {
                let last = command.rsplit(' ').next().expect("a word");
                assert!(script.contains(last), "{shell:?}: {command}");
            }
            // Fish names long options without their dashes (`-l bg-color`).
            for word in ["bg-color", "no-persist", "pause-on", "covered"] {
                assert!(script.contains(word), "{shell:?}: {word}");
            }
            // The fish generator does not complete positional arguments, so
            // fish does not offer the shells of `veil completions`.
            assert_eq!(
                script.contains("nushell"),
                shell != Shell::Fish,
                "{shell:?}"
            );
            // The deprecated top-level flag and the packaging command.
            for hidden in ["--daemon", "-l daemon", "man-pages", "man_pages"] {
                assert!(!script.contains(hidden), "{shell:?}: {hidden}");
            }
        }
    }

    #[test]
    fn only_the_top_level_hides_anything() {
        // `completed_command` leaves out what the top level hides, and
        // takes every subcommand as it is.
        fn hides_nothing(cmd: &clap::Command) {
            let name = cmd.get_name();
            assert!(cmd.get_arguments().all(|arg| !arg.is_hide_set()), "{name}");
            for sub in cmd.get_subcommands() {
                assert!(!sub.is_hide_set(), "{name} {}", sub.get_name());
                hides_nothing(sub);
            }
        }
        let cli = Cli::command();
        cli.get_subcommands()
            .filter(|sub| !sub.is_hide_set())
            .for_each(hides_nothing);
    }

    #[test]
    fn there_is_a_man_page_for_every_command() {
        let pages = man_pages().expect("pages");
        let names: Vec<&str> = pages.iter().map(|page| page.file_name.as_str()).collect();
        let mut expected = vec!["veil.1".to_string()];
        expected.extend(COMMANDS.map(|command| format!("veil-{}.1", command.replace(' ', "-"))));
        assert_eq!(names, expected);
        for page in &pages {
            assert!(page.text.contains("\n.TH VEIL"), "{}", page.file_name);
            assert!(!page.text.contains("man\\-pages"), "{}", page.file_name);
            assert!(!page.text.contains("\\-\\-daemon"), "{}", page.file_name);
        }
    }

    fn page(file_name: &str) -> String {
        man_pages()
            .expect("pages")
            .into_iter()
            .find(|page| page.file_name == file_name)
            .expect("the page")
            .text
    }

    #[test]
    fn a_command_page_has_its_options_and_points_to_the_main_page() {
        let set = page("veil-set.1");
        for option in ["\\-\\-input", "\\-\\-no\\-persist", "\\-\\-bg\\-color"] {
            assert!(set.contains(option), "{option}");
        }
        assert!(set.contains("veil set"), "the synopsis");
        assert!(set.contains(".SH \"SEE ALSO\""));
        assert!(page("veil-daemon.1").contains(".SH SIGNALS"));
        assert!(page("veil-config.1").contains("veil\\-config\\-check(1)"));
    }

    #[test]
    fn the_main_page_has_every_exit_status_and_the_files() {
        let main = page("veil.1");
        assert!(main.contains("veil\\-set(1)"), "the commands");
        let exit_status = main
            .split(".SH \"EXIT STATUS\"")
            .nth(1)
            .expect("an EXIT STATUS section");
        for status in [
            0,
            EXIT_FAILURE,
            EXIT_USAGE,
            EXIT_DAEMON_NOT_RUNNING,
            EXIT_ALREADY_RUNNING,
            EXIT_COMPOSITOR_GONE,
        ] {
            assert!(
                exit_status.contains(&format!("\\fB{status}\\fR")),
                "{status}"
            );
        }
        for section in [".SH ENVIRONMENT", ".SH FILES"] {
            assert!(main.contains(section), "{section}");
        }
        assert!(main.contains("state.json"));
    }

    #[test]
    fn the_pages_are_written_into_a_directory_that_is_created() {
        let dir = std::env::temp_dir().join(format!("veil-man-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_man_pages(&dir.join("man1")).expect("written");
        let written = std::fs::read_to_string(dir.join("man1/veil-config-check.1"))
            .expect("the page of a nested command");
        assert_eq!(written, page("veil-config-check.1"));
        std::fs::remove_dir_all(&dir).expect("remove the directory");
    }
}
