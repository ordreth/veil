//! The configuration file: `$XDG_CONFIG_HOME/veil/config.toml`, or the file
//! of `veil daemon --config`.
//!
//! It holds what the daemon's flags hold, as defaults (`[defaults]`), and the
//! wallpaper of each output (`[output."NAME"]`, and `[output."*"]` for every
//! output without a section of its own). A flag given on the command line wins
//! over the file, the file over the built-in default ([`settings`]). The file
//! is read once, when the daemon starts; a changed file takes effect at the
//! next start.
//!
//! An output with a wallpaper in the file shows it at every start, as with
//! `--input`: a `veil set` changes it until the daemon stops, and is saved, but
//! the next daemon starts with the file's wallpaper again. Outputs the file
//! gives no wallpaper show their saved one (`persist`). The `--input` of the
//! command line is for every output and wins over every section.
//!
//! Reading is pure apart from [`load`]: [`parse`] takes the text and the
//! environment, so `~` and `$VARIABLE` expansion are tested without touching
//! the process's environment. Every error names the file, and the line and
//! column where the file has one. Unknown keys are errors: a misspelt key
//! would otherwise be ignored without a word. There are keys only for what the
//! daemon can do; later keys are added, never renamed.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use toml::Spanned;

use crate::config::{DaemonArgs, RELEASE_UNSEEN_AFTER};
use crate::error::VeilError;
use crate::persist::{Wallpaper, Wallpapers};
use crate::policy::{PauseOn, Policy};
use crate::scaling::{Color, ScaleMode, ScaleOptions};

/// The name of the section for every output without one of its own.
pub const EVERY_OUTPUT: &str = "*";

/// A configuration file, read and checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigFile {
    /// Where it was read from.
    pub path: PathBuf,
    /// `[defaults]`: the daemon's flags. `None` where the file says nothing.
    pub defaults: Defaults,
    /// `[output."NAME"]` by name, `[output."*"]` under [`EVERY_OUTPUT`].
    pub outputs: BTreeMap<String, OutputSection>,
}

/// `[defaults]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Defaults {
    pub mode: Option<ScaleMode>,
    pub bg_color: Option<Color>,
    pub pause_on: Option<PauseOn>,
    pub release_unseen_after: Option<u64>,
    /// The outputs to draw on (`--output`); empty: every output.
    pub outputs: Option<Vec<String>>,
}

/// `[output."NAME"]`: the wallpaper of an output. Without `input` it is the
/// background colour alone, as after `veil clear`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputSection {
    /// An absolute path: `~` and variables expanded, a relative path taken
    /// from the directory of the file.
    pub input: Option<String>,
    pub mode: Option<ScaleMode>,
    pub bg_color: Option<Color>,
}

/// The file as it is written.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    defaults: RawDefaults,
    #[serde(default)]
    output: BTreeMap<String, RawOutput>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDefaults {
    mode: Option<ScaleMode>,
    bg_color: Option<Color>,
    pause_on: Option<PauseOn>,
    release_unseen_after: Option<u64>,
    outputs: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutput {
    input: Option<Spanned<String>>,
    mode: Option<Spanned<ScaleMode>>,
    bg_color: Option<Color>,
}

/// Where the configuration file is looked for: `$XDG_CONFIG_HOME/veil/config.toml`,
/// or `$HOME/.config/veil/config.toml` when `XDG_CONFIG_HOME` is not set (or
/// not an absolute path, which the XDG base directory specification says to
/// ignore). `None` without either.
pub fn config_path() -> Option<PathBuf> {
    config_path_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// [`config_path`] from the values of `XDG_CONFIG_HOME` and `HOME`.
pub fn config_path_from(config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute = |value: OsString| Some(PathBuf::from(value)).filter(|path| path.is_absolute());
    let base = config_home
        .and_then(absolute)
        .or_else(|| Some(home.and_then(absolute)?.join(".config")))?;
    Some(base.join("veil").join("config.toml"))
}

/// The configuration of the daemon: the file `explicit` (`--config`), which
/// must exist, or else the file at [`config_path`], which may be missing
/// (`None`: no configuration file).
pub fn find(explicit: Option<&Path>) -> Result<Option<ConfigFile>, VeilError> {
    match explicit {
        Some(path) => load(path).map(Some),
        None => match config_path() {
            Some(path) if path.exists() => load(&path).map(Some),
            _ => Ok(None),
        },
    }
}

/// Reads and checks the file at `path`, expanding `~` and variables from the
/// process's environment.
pub fn load(path: &Path) -> Result<ConfigFile, VeilError> {
    let text = std::fs::read_to_string(path).map_err(|e| VeilError::Config {
        path: path.to_path_buf(),
        position: None,
        message: e.to_string(),
    })?;
    parse(&text, path, |name| std::env::var_os(name))
}

/// Reads `text`, the content of the file at `path`. `env` gives the value of
/// an environment variable (`HOME` for `~`).
pub fn parse(
    text: &str,
    path: &Path,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<ConfigFile, VeilError> {
    let error = |span: Option<std::ops::Range<usize>>, message: String| VeilError::Config {
        path: path.to_path_buf(),
        position: span.map(|span| position(text, span.start)),
        message,
    };
    let raw: RawFile =
        toml::from_str(text).map_err(|e| error(e.span(), e.message().trim().to_string()))?;
    // A relative path is taken from the directory of the file as it was named
    // (not where a symbolic link leads), so a wallpaper next to the file is
    // found by its name.
    let directory = std::path::absolute(path)
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));

    let mut outputs = BTreeMap::new();
    for (name, section) in raw.output {
        if name.is_empty() {
            return Err(error(
                None,
                "an [output.\"\"] section has no output name".into(),
            ));
        }
        let input = match section.input {
            Some(input) => {
                let span = input.span();
                let expanded = expand(input.get_ref(), &env)
                    .map_err(|message| error(Some(span.clone()), message))?;
                if expanded.is_empty() {
                    return Err(error(Some(span), "`input` is empty".into()));
                }
                let expanded = PathBuf::from(expanded);
                let absolute = match (&directory, expanded.is_absolute()) {
                    (_, true) => expanded,
                    (Some(directory), false) => directory.join(expanded),
                    (None, false) => {
                        return Err(error(
                            Some(span),
                            "a relative `input` needs the directory of the file".into(),
                        ))
                    }
                };
                let absolute = absolute.into_os_string().into_string().map_err(|_| {
                    error(
                        Some(span),
                        "`input` is not valid UTF-8 once expanded".into(),
                    )
                })?;
                Some(absolute)
            }
            None => None,
        };
        if let (None, Some(mode)) = (&input, &section.mode) {
            return Err(error(
                Some(mode.span()),
                format!(
                    "`mode` in [output.\"{name}\"] needs an `input`: a section without one \
                     shows only its background colour"
                ),
            ));
        }
        outputs.insert(
            name,
            OutputSection {
                input,
                mode: section.mode.map(Spanned::into_inner),
                bg_color: section.bg_color,
            },
        );
    }

    Ok(ConfigFile {
        path: path.to_path_buf(),
        defaults: Defaults {
            mode: raw.defaults.mode,
            bg_color: raw.defaults.bg_color,
            pause_on: raw.defaults.pause_on,
            release_unseen_after: raw.defaults.release_unseen_after,
            outputs: raw.defaults.outputs,
        },
        outputs,
    })
}

/// The line and column (both from 1) of the byte `offset` of `text`.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |line| line.chars().count())
        + 1;
    (line, column)
}

/// `value` with a leading `~` (alone or before `/`) replaced by `$HOME`, and
/// `$NAME` and `${NAME}` by the variable's value. `$$` is a `$`. A variable
/// that is not set is an error, and so is a `$` that starts nothing, and
/// `~user`, which is not expanded.
fn expand(value: &str, env: &impl Fn(&str) -> Option<OsString>) -> Result<String, String> {
    let lookup = |name: &str| -> Result<String, String> {
        env(name)
            .ok_or_else(|| format!("the environment variable {name} is not set"))?
            .into_string()
            .map_err(|_| format!("the environment variable {name} is not valid UTF-8"))
    };

    let mut out = String::new();
    let mut rest = value;
    if let Some(after) = rest.strip_prefix('~') {
        if !(after.is_empty() || after.starts_with('/')) {
            return Err(
                "only `~` and `~/` are expanded, not `~user`; write the path out".to_string(),
            );
        }
        let home = lookup("HOME")?;
        if !Path::new(&home).is_absolute() {
            return Err("HOME is not an absolute path, so `~` cannot be expanded".to_string());
        }
        out.push_str(&home);
        rest = after;
    }

    while let Some(dollar) = rest.find('$') {
        out.push_str(&rest[..dollar]);
        let after = &rest[dollar + 1..];
        if let Some(after) = after.strip_prefix('$') {
            out.push('$');
            rest = after;
        } else if let Some(braced) = after.strip_prefix('{') {
            let end = braced
                .find('}')
                .ok_or_else(|| "`${` without its `}`".to_string())?;
            let name = &braced[..end];
            if !is_variable_name(name) {
                return Err(format!("`${{{name}}}` is not a variable name"));
            }
            out.push_str(&lookup(name)?);
            rest = &braced[end + 1..];
        } else {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            let name = &after[..end];
            if !is_variable_name(name) {
                return Err("a `$` that starts no variable name; write `$$` for a `$`".into());
            }
            out.push_str(&lookup(name)?);
            rest = &after[end..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// A shell variable name: a letter or `_`, then letters, digits and `_`.
fn is_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What the daemon starts with: its command line, over the configuration
/// file, over the built-in defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `--input`: the wallpaper of every output, over every section of the
    /// file.
    pub input: Option<String>,
    /// The outputs to draw on; empty: every output.
    pub outputs: Vec<String>,
    /// How a wallpaper is shown unless it says otherwise (`--mode`,
    /// `--bg-color`; also the defaults of `veil set`).
    pub defaults: ScaleOptions,
    pub policy: Policy,
    /// The wallpapers of the file, which the outputs show at start-up. Empty
    /// with `--input`.
    pub configured: Configured,
}

/// The wallpapers a configuration file gives the outputs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Configured {
    /// `[output."*"]`: for every output without a section of its own.
    pub every: Option<Wallpaper>,
    /// `[output."NAME"]`, by name.
    pub outputs: BTreeMap<String, Wallpaper>,
}

impl Configured {
    /// Whether the file gives no output a wallpaper.
    pub fn is_empty(&self) -> bool {
        self.every.is_none() && self.outputs.is_empty()
    }

    /// Puts the file's wallpapers over `wallpapers` (what was saved), as a
    /// `--input` would be: `[output."*"]` like a `veil set` for every output
    /// (`requested` is the daemon's `--output` list), then each named section
    /// as that output's own wallpaper.
    pub fn apply(&self, wallpapers: &mut Wallpapers, requested: &[String]) {
        if let Some(every) = &self.every {
            wallpapers.set_everywhere(every.clone(), &[], requested);
        }
        for (name, wallpaper) in &self.outputs {
            wallpapers.shown_on(name, wallpaper.clone());
        }
    }
}

/// The daemon's settings from its command line `args` and its configuration
/// file `file`: a flag that was given wins over the file, the file over the
/// built-in default. A section's own `mode` and `bg_color` win over the
/// defaults, wherever those come from.
pub fn settings(args: &DaemonArgs, file: Option<&ConfigFile>) -> Settings {
    let empty = ConfigFile::default();
    let file = file.unwrap_or(&empty);
    let defaults = ScaleOptions {
        mode: args.mode.or(file.defaults.mode).unwrap_or_default(),
        bg_color: args.bg_color.or(file.defaults.bg_color).unwrap_or_default(),
    };
    let release_unseen_after = args
        .release_unseen_after
        .or(file.defaults.release_unseen_after)
        .unwrap_or(RELEASE_UNSEEN_AFTER);
    let policy = Policy {
        pause_on: args.pause_on.or(file.defaults.pause_on).unwrap_or_default(),
        release_unseen_after: (release_unseen_after > 0)
            .then(|| Duration::from_secs(release_unseen_after)),
    };
    let outputs = if args.output.is_empty() {
        file.defaults.outputs.clone().unwrap_or_default()
    } else {
        args.output.clone()
    };

    let mut configured = Configured::default();
    if args.input.is_none() {
        for (name, section) in &file.outputs {
            let options = ScaleOptions {
                mode: section.mode.unwrap_or(defaults.mode),
                bg_color: section.bg_color.unwrap_or(defaults.bg_color),
            };
            let wallpaper = match &section.input {
                Some(input) => Wallpaper::file(input.clone(), options),
                None => Wallpaper::color(options),
            };
            if name == EVERY_OUTPUT {
                configured.every = Some(wallpaper);
            } else {
                configured.outputs.insert(name.clone(), wallpaper);
            }
        }
    }

    Settings {
        input: args.input.clone(),
        outputs,
        defaults,
        policy,
        configured,
    }
}

/// `veil config check`: reads the file `explicit` (else the one at
/// [`config_path`]) as the daemon would, and checks that each of its
/// wallpapers can be shown (`engine::media::detect`: the file exists, and is
/// an image or a video). Returns what the daemon would start with, as text.
/// A file that is wrong is [`VeilError::Config`]; wallpapers that cannot be
/// shown are [`VeilError::ConfigInputs`], one message per section.
pub fn check(explicit: Option<&Path>) -> Result<String, VeilError> {
    let Some(file) = find(explicit)? else {
        return Ok(match config_path() {
            Some(path) => format!(
                "no configuration file at {}; the built-in defaults apply\n",
                path.display()
            ),
            None => "neither XDG_CONFIG_HOME nor HOME is set, so there is no configuration \
                     file; the built-in defaults apply\n"
                .to_string(),
        });
    };
    let settings = settings(&DaemonArgs::default(), Some(&file));
    let mut problems = Vec::new();
    // Opened as the daemon opens them at start-up: an image is decoded too,
    // so a damaged picture is found here as well.
    let kinds: BTreeMap<&str, &'static str> = file
        .outputs
        .iter()
        .filter_map(|(name, section)| Some((name.as_str(), section.input.as_deref()?)))
        .filter_map(|(name, input)| {
            match crate::engine::Source::open(input, ScaleOptions::default()) {
                Ok(source) if source.is_video() => Some((name, "video")),
                Ok(_) => Some((name, "image")),
                Err(e) => {
                    problems.push(format!("[output.\"{name}\"]: {e}"));
                    None
                }
            }
        })
        .collect();
    if !problems.is_empty() {
        return Err(VeilError::ConfigInputs(problems));
    }
    Ok(describe(&file.path, &settings, &kinds))
}

/// What `veil config check` prints: the settings, and each output's
/// wallpaper with its kind (`kinds`, by section name).
fn describe(path: &Path, settings: &Settings, kinds: &BTreeMap<&str, &str>) -> String {
    let mut text = format!("{}: ok\n", path.display());
    let outputs = match settings.outputs.as_slice() {
        [] => "every output".to_string(),
        names => names.join(", "),
    };
    let release = match settings.policy.release_unseen_after {
        Some(after) => format!("after {} s", after.as_secs()),
        None => "never".to_string(),
    };
    text += &format!(
        "outputs: {outputs}\nmode: {}\nbg_color: {}\npause_on: {}\nrelease_unseen_after: {release}\n",
        settings.defaults.mode.name(),
        settings.defaults.bg_color,
        settings.policy.pause_on.name(),
    );
    let sections = settings
        .configured
        .outputs
        .iter()
        .map(|(name, wallpaper)| (name.as_str(), wallpaper))
        .chain(settings.configured.every.iter().map(|w| (EVERY_OUTPUT, w)));
    for (name, wallpaper) in sections {
        let shown = match &wallpaper.input {
            Some(input) => format!(
                "{} {input} ({}, {})",
                kinds.get(name).copied().unwrap_or("file"),
                wallpaper.options.mode.name(),
                wallpaper.options.bg_color
            ),
            None => format!("the background colour {}", wallpaper.options.bg_color),
        };
        text += &format!("[output.\"{name}\"]: {shown}\n");
    }
    if settings.configured.every.is_none() {
        text += "other outputs: their saved wallpaper, else the background colour\n";
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "/home/u/.config/veil/config.toml";

    fn env(name: &str) -> Option<OsString> {
        match name {
            "HOME" => Some("/home/u".into()),
            "PICTURES" => Some("/data/pictures".into()),
            "EMPTY" => Some("".into()),
            _ => None,
        }
    }

    fn read(text: &str) -> Result<ConfigFile, VeilError> {
        parse(text, Path::new(PATH), env)
    }

    fn error_of(text: &str) -> (Option<(usize, usize)>, String) {
        match read(text) {
            Err(VeilError::Config {
                path,
                position,
                message,
            }) => {
                assert_eq!(path, Path::new(PATH));
                (position, message)
            }
            other => panic!("expected a config error for {text:?}, got {other:?}"),
        }
    }

    fn section(input: Option<&str>, mode: Option<ScaleMode>) -> OutputSection {
        OutputSection {
            input: input.map(Into::into),
            mode,
            bg_color: None,
        }
    }

    #[test]
    fn the_config_path_follows_the_xdg_rules() {
        assert_eq!(
            config_path_from(Some("/x/config".into()), Some("/home/u".into())),
            Some(PathBuf::from("/x/config/veil/config.toml"))
        );
        // A relative XDG_CONFIG_HOME is ignored.
        for config_home in [None, Some("relative".into()), Some("".into())] {
            assert_eq!(
                config_path_from(config_home, Some("/home/u".into())),
                Some(PathBuf::from("/home/u/.config/veil/config.toml"))
            );
        }
        assert_eq!(config_path_from(None, None), None);
        assert_eq!(config_path_from(None, Some("relative".into())), None);
    }

    #[test]
    fn an_empty_file_says_nothing() {
        let file = read("").expect("an empty file is valid");
        assert_eq!(file.defaults, Defaults::default());
        assert!(file.outputs.is_empty());
        let file = read("# only a comment\n\n[defaults]\n").expect("valid");
        assert_eq!(file.defaults, Defaults::default());
    }

    #[test]
    fn every_key_is_read() {
        let file = read(
            r##"
            [defaults]
            mode = "fit"
            bg_color = "#102030"
            pause_on = "covered"
            release_unseen_after = 0
            outputs = ["DP-3", "eDP-1"]

            [output."DP-3"]
            input = "/videos/a.mp4"
            mode = "center"
            bg_color = "#ffffff"

            [output."*"]
            input = "/pictures/b.png"

            [output.eDP-1]
            bg_color = "#000001"
            "##,
        )
        .expect("valid");
        assert_eq!(
            file.defaults,
            Defaults {
                mode: Some(ScaleMode::Fit),
                bg_color: Some(Color {
                    r: 0x10,
                    g: 0x20,
                    b: 0x30
                }),
                pause_on: Some(PauseOn::Covered),
                release_unseen_after: Some(0),
                outputs: Some(vec!["DP-3".into(), "eDP-1".into()]),
            }
        );
        assert_eq!(
            file.outputs.get("DP-3"),
            Some(&OutputSection {
                input: Some("/videos/a.mp4".into()),
                mode: Some(ScaleMode::Center),
                bg_color: Some(Color {
                    r: 255,
                    g: 255,
                    b: 255
                }),
            })
        );
        assert_eq!(
            file.outputs.get("*"),
            Some(&section(Some("/pictures/b.png"), None))
        );
        assert_eq!(
            file.outputs.get("eDP-1"),
            Some(&OutputSection {
                input: None,
                mode: None,
                bg_color: Some(Color { r: 0, g: 0, b: 1 }),
            })
        );
    }

    #[test]
    fn paths_are_expanded_and_made_absolute() {
        for (input, expected) in [
            ("~/a.png", "/home/u/a.png"),
            ("~", "/home/u"),
            ("$PICTURES/a.png", "/data/pictures/a.png"),
            ("${PICTURES}/a.png", "/data/pictures/a.png"),
            ("/x/${PICTURES}y", "/x//data/picturesy"),
            ("/x/$EMPTY/a.png", "/x//a.png"),
            ("/cost$$/a.png", "/cost$/a.png"),
            ("/a~b/~c.png", "/a~b/~c.png"),
            // Relative to the directory of the file.
            ("a.png", "/home/u/.config/veil/a.png"),
            (
                "../pictures/a.png",
                "/home/u/.config/veil/../pictures/a.png",
            ),
            ("$EMPTY/a.png", "/a.png"),
        ] {
            let file = read(&format!("[output.X]\ninput = '{input}'\n"))
                .unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!(
                file.outputs.get("X").and_then(|s| s.input.as_deref()),
                Some(expected),
                "{input}"
            );
        }
    }

    #[test]
    fn expansion_errors_name_the_line_and_column() {
        for (input, message) in [
            ("$UNSET/a.png", "UNSET is not set"),
            ("${UNSET}/a.png", "UNSET is not set"),
            ("~other/a.png", "~user"),
            ("/a$.png", "write `$$`"),
            ("/a${PICTURES", "without its `}`"),
            ("/a${1X}", "not a variable name"),
            ("$EMPTY", "`input` is empty"),
        ] {
            let text = format!("[defaults]\nmode = \"fill\"\n\n[output.X]\ninput   = '{input}'\n");
            let (position, error) = error_of(&text);
            assert_eq!(position, Some((5, 11)), "{input}");
            assert!(error.contains(message), "{input}: {error}");
        }
    }

    #[test]
    fn invalid_values_name_the_line() {
        for (text, line, message) in [
            ("[defaults]\nmode = \"zoom\"\n", 2, "zoom"),
            ("[defaults]\n\nbg_color = \"red\"\n", 3, "red"),
            ("[defaults]\npause_on = \"hidden\"\n", 2, "hidden"),
            ("[defaults]\nrelease_unseen_after = -1\n", 2, ""),
            ("[defaults]\nrelease_unseen_after = 1.5\n", 2, ""),
            ("[defaults]\noutputs = \"DP-3\"\n", 2, ""),
            ("[output.X]\ninput = 3\n", 2, ""),
            ("[output.X]\nmode = \"fit\"\n", 2, "needs an `input`"),
            ("[defaults]\nmode = \n", 2, ""),
            ("[defaults]\nmode = \"fit\"\nmode = \"fill\"\n", 3, ""),
        ] {
            let (position, error) = error_of(text);
            assert_eq!(position.map(|p| p.0), Some(line), "{text:?}: {error}");
            assert!(error.contains(message), "{text:?}: {error}");
        }
    }

    #[test]
    fn unknown_keys_are_errors() {
        for (text, line, key) in [
            ("[defaults]\nmod = \"fit\"\n", 2, "mod"),
            ("[defaults]\nvolume = 3\n", 2, "volume"),
            ("[output.X]\ninput = \"/a.png\"\nloop = false\n", 3, "loop"),
            ("[outputs.X]\ninput = \"/a.png\"\n", 1, "outputs"),
            ("input = \"/a.png\"\n", 1, "input"),
        ] {
            let (position, error) = error_of(text);
            assert_eq!(position.map(|p| p.0), Some(line), "{text:?}: {error}");
            assert!(error.contains(key), "{text:?}: {error}");
        }
        let (_, error) = error_of("[output.\"\"]\n");
        assert!(error.contains("no output name"), "{error}");
    }

    #[test]
    fn an_error_reads_as_file_line_column() {
        let error = read("[defaults]\nmode = \"zoom\"\n").expect_err("invalid");
        let text = error.to_string();
        assert!(text.starts_with(&format!("{PATH}:2:8: ")), "{text}");
        assert_eq!(error.exit_code(), crate::error::EXIT_USAGE);
    }

    fn args() -> DaemonArgs {
        DaemonArgs::default()
    }

    fn file(text: &str) -> ConfigFile {
        read(text).expect("valid")
    }

    #[test]
    fn built_in_defaults_without_a_file() {
        let settings = settings(&args(), None);
        assert_eq!(settings.input, None);
        assert!(settings.outputs.is_empty());
        assert_eq!(settings.defaults, ScaleOptions::default());
        assert_eq!(settings.policy.pause_on, PauseOn::Fullscreen);
        assert_eq!(
            settings.policy.release_unseen_after,
            Some(Duration::from_secs(RELEASE_UNSEEN_AFTER))
        );
        assert!(settings.configured.is_empty());
    }

    #[test]
    fn the_file_wins_over_the_built_in_defaults() {
        let file = file(
            "[defaults]\nmode = \"fit\"\nbg_color = \"#010203\"\npause_on = \"never\"\n\
             release_unseen_after = 0\noutputs = [\"DP-3\"]\n",
        );
        let settings = settings(&args(), Some(&file));
        assert_eq!(settings.defaults.mode, ScaleMode::Fit);
        assert_eq!(settings.defaults.bg_color, Color { r: 1, g: 2, b: 3 });
        assert_eq!(settings.policy.pause_on, PauseOn::Never);
        assert_eq!(settings.policy.release_unseen_after, None);
        assert_eq!(settings.outputs, ["DP-3"]);
    }

    #[test]
    fn the_command_line_wins_over_the_file() {
        let file = file(
            "[defaults]\nmode = \"fit\"\nbg_color = \"#010203\"\npause_on = \"never\"\n\
             release_unseen_after = 0\noutputs = [\"DP-3\"]\n\n\
             [output.\"*\"]\ninput = \"/a.png\"\n[output.DP-3]\ninput = \"/b.png\"\n",
        );
        let args = DaemonArgs {
            output: vec!["HDMI-A-1".into()],
            input: Some("/c.mp4".into()),
            mode: Some(ScaleMode::Tile),
            bg_color: Some(Color { r: 9, g: 9, b: 9 }),
            pause_on: Some(PauseOn::Covered),
            release_unseen_after: Some(5),
            config: None,
        };
        let settings = settings(&args, Some(&file));
        assert_eq!(settings.defaults.mode, ScaleMode::Tile);
        assert_eq!(settings.defaults.bg_color, Color { r: 9, g: 9, b: 9 });
        assert_eq!(settings.policy.pause_on, PauseOn::Covered);
        assert_eq!(
            settings.policy.release_unseen_after,
            Some(Duration::from_secs(5))
        );
        assert_eq!(settings.outputs, ["HDMI-A-1"]);
        // `--input` is every output's wallpaper: the sections are not used.
        assert_eq!(settings.input.as_deref(), Some("/c.mp4"));
        assert!(settings.configured.is_empty());
    }

    #[test]
    fn sections_take_the_defaults_they_do_not_set() {
        let file = file(
            "[defaults]\nmode = \"fit\"\nbg_color = \"#010203\"\n\n\
             [output.\"*\"]\ninput = \"/a.png\"\n\
             [output.DP-3]\ninput = \"/b.mp4\"\nmode = \"center\"\n\
             [output.eDP-1]\nbg_color = \"#ffffff\"\n\
             [output.HDMI-A-1]\n",
        );
        let fit = ScaleOptions {
            mode: ScaleMode::Fit,
            bg_color: Color { r: 1, g: 2, b: 3 },
        };
        // A `--bg-color` on the command line is the default of the sections.
        let args = DaemonArgs {
            bg_color: Some(Color { r: 7, g: 7, b: 7 }),
            ..args()
        };
        let with_flag = ScaleOptions {
            bg_color: Color { r: 7, g: 7, b: 7 },
            ..fit
        };
        for (args, base) in [(DaemonArgs::default(), fit), (args, with_flag)] {
            let configured = settings(&args, Some(&file)).configured;
            assert_eq!(configured.every, Some(Wallpaper::file("/a.png", base)));
            assert_eq!(
                configured.outputs.get("DP-3"),
                Some(&Wallpaper::file(
                    "/b.mp4",
                    ScaleOptions {
                        mode: ScaleMode::Center,
                        ..base
                    }
                ))
            );
            assert_eq!(
                configured.outputs.get("eDP-1"),
                Some(&Wallpaper::color(ScaleOptions {
                    bg_color: Color {
                        r: 255,
                        g: 255,
                        b: 255
                    },
                    ..base
                }))
            );
            assert_eq!(
                configured.outputs.get("HDMI-A-1"),
                Some(&Wallpaper::color(base))
            );
            assert!(!configured.outputs.contains_key("*"));
        }
    }

    fn wallpaper(input: &str) -> Wallpaper {
        Wallpaper::file(input, ScaleOptions::default())
    }

    /// What the saved state of these tests holds: a default and two outputs
    /// of their own.
    fn saved() -> Wallpapers {
        let mut saved = Wallpapers {
            default: Some(wallpaper("/saved-default.png")),
            ..Wallpapers::default()
        };
        saved.shown_on("DP-3", wallpaper("/saved-dp3.png"));
        saved.shown_on("eDP-1", wallpaper("/saved-edp1.png"));
        saved
    }

    #[test]
    fn the_wildcard_is_for_every_output_without_a_section() {
        let configured = Configured {
            every: Some(wallpaper("/every.png")),
            outputs: [("DP-3".to_string(), wallpaper("/dp3.png"))].into(),
        };
        let mut wallpapers = saved();
        configured.apply(&mut wallpapers, &[]);
        let on = |name| wallpapers.for_output(Some(name)).cloned();
        // Its own section wins over the wildcard and over what was saved.
        assert_eq!(on("DP-3"), Some(wallpaper("/dp3.png")));
        // The wildcard wins over what was saved for an output without a section.
        assert_eq!(on("eDP-1"), Some(wallpaper("/every.png")));
        assert_eq!(on("HDMI-A-1"), Some(wallpaper("/every.png")));
        assert_eq!(
            wallpapers.for_output(None).cloned(),
            Some(wallpaper("/every.png"))
        );
    }

    #[test]
    fn without_a_wildcard_the_saved_wallpapers_stay() {
        let configured = Configured {
            every: None,
            outputs: [("DP-3".to_string(), wallpaper("/dp3.png"))].into(),
        };
        let mut wallpapers = saved();
        configured.apply(&mut wallpapers, &[]);
        let on = |name| wallpapers.for_output(Some(name)).cloned();
        assert_eq!(on("DP-3"), Some(wallpaper("/dp3.png")));
        assert_eq!(on("eDP-1"), Some(wallpaper("/saved-edp1.png")));
        assert_eq!(on("HDMI-A-1"), Some(wallpaper("/saved-default.png")));
    }

    #[test]
    fn with_output_names_the_wildcard_is_for_those_outputs() {
        // As a `--input` with `--output`: the default is left to daemons that
        // draw on every output.
        let configured = Configured {
            every: Some(wallpaper("/every.png")),
            outputs: BTreeMap::new(),
        };
        let mut wallpapers = saved();
        configured.apply(&mut wallpapers, &["eDP-1".to_string(), "DP-4".to_string()]);
        let on = |name| wallpapers.for_output(Some(name)).cloned();
        assert_eq!(on("eDP-1"), Some(wallpaper("/every.png")));
        assert_eq!(on("DP-4"), Some(wallpaper("/every.png")));
        assert_eq!(on("DP-3"), Some(wallpaper("/saved-dp3.png")));
        assert_eq!(wallpapers.default, Some(wallpaper("/saved-default.png")));
    }

    #[test]
    fn the_example_configuration_is_valid() {
        let text = include_str!("../contrib/config.example.toml");
        let file = read(text).expect("the example parses");
        assert!(!file.outputs.is_empty());
        let settings = settings(&args(), Some(&file));
        assert!(!settings.configured.is_empty());
    }

    #[test]
    fn the_check_describes_what_the_daemon_starts_with() {
        let file = file(
            "[defaults]\nbg_color = \"#010203\"\nrelease_unseen_after = 0\n\n\
             [output.DP-3]\ninput = \"/b.mp4\"\nmode = \"fit\"\n\
             [output.\"*\"]\ninput = \"/a.png\"\n[output.eDP-1]\n",
        );
        let settings = settings(&args(), Some(&file));
        let kinds = [("DP-3", "video"), ("*", "image")].into();
        assert_eq!(
            describe(Path::new(PATH), &settings, &kinds),
            format!(
                "{PATH}: ok\noutputs: every output\nmode: fill\nbg_color: #010203\n\
                 pause_on: fullscreen\nrelease_unseen_after: never\n\
                 [output.\"DP-3\"]: video /b.mp4 (fit, #010203)\n\
                 [output.\"eDP-1\"]: the background colour #010203\n\
                 [output.\"*\"]: image /a.png (fill, #010203)\n"
            )
        );
        let restricted = super::settings(
            &args(),
            Some(&read("[defaults]\noutputs = [\"A\", \"B\"]\n").expect("valid")),
        );
        let text = describe(Path::new(PATH), &restricted, &BTreeMap::new());
        assert!(text.contains("outputs: A, B\n"), "{text}");
        assert!(text.contains("release_unseen_after: after 2 s\n"), "{text}");
        assert!(
            text.ends_with("other outputs: their saved wallpaper, else the background colour\n"),
            "{text}"
        );
    }

    #[test]
    fn the_check_refuses_wallpapers_that_cannot_be_shown() {
        let dir = std::env::temp_dir().join(format!("veil-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the directory");
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[output.A]\ninput = \"missing.png\"\n[output.B]\ninput = \"empty.png\"\n[output.C]\n",
        )
        .expect("write the file");
        std::fs::write(dir.join("empty.png"), b"").expect("write the image");
        let error = check(Some(&path)).expect_err("two wallpapers cannot be shown");
        let _ = std::fs::remove_dir_all(&dir);
        let VeilError::ConfigInputs(problems) = &error else {
            panic!("expected the inputs to be refused: {error:?}");
        };
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("[output.\"A\"]: "), "{problems:?}");
        assert!(problems[1].contains("is empty"), "{problems:?}");
        assert_eq!(error.exit_code(), crate::error::EXIT_FAILURE);
    }

    #[test]
    fn a_missing_explicit_file_is_an_error() {
        let path = std::env::temp_dir().join(format!("veil-no-config-{}.toml", std::process::id()));
        let error = find(Some(&path)).expect_err("missing");
        assert!(
            matches!(error, VeilError::Config { position: None, .. }),
            "{error:?}"
        );
        assert!(
            error.to_string().starts_with(&path.display().to_string()),
            "{error}"
        );
    }

    #[test]
    fn a_file_is_read_from_disk() {
        let path = std::env::temp_dir().join(format!("veil-config-{}.toml", std::process::id()));
        std::fs::write(&path, "[output.X]\ninput = \"a.png\"\n").expect("write the file");
        let file = find(Some(&path)).expect("valid").expect("a file");
        let _ = std::fs::remove_file(&path);
        let directory = path.parent().expect("a directory");
        assert_eq!(
            file.outputs.get("X").and_then(|s| s.input.clone()),
            Some(directory.join("a.png").display().to_string())
        );
    }
}
