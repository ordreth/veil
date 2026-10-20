//! The wallpapers that survive a restart.
//!
//! After every `veil set` (unless it says `--no-persist`) the daemon saves what
//! each output shows in `$XDG_STATE_HOME/veil/state.json`, and a daemon started
//! without `--input` shows the same again.
//!
//! [`Wallpapers`] is the rule, and pure: which wallpaper an output gets when
//! the daemon draws on it for the first time, and how a `set` changes that.
//! The daemon keeps two of them: the one it goes by while it runs (changed by
//! every `set`), and the one in the file (changed by the `set`s that are
//! saved), which [`Store`] writes, on a thread of its own.
//!
//! The file has a format version ([`STATE_VERSION`]). Like the control
//! protocol's, it is raised only for a change an older veil would misread; a
//! field it does not know is ignored. A file of a newer version is neither
//! read nor replaced. A file is written in the oldest version that can hold
//! its content ([`encode`]), so a veil that knows only that version can still
//! read it.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

use crate::scaling::{Color, ScaleMode, ScaleOptions};

/// The newest version of the state file's format, which this veil reads and
/// writes. Version 2 (since `veil clear`) may have wallpapers without a file;
/// version 1 has none.
pub const STATE_VERSION: u32 = 2;

/// A wallpaper and how to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wallpaper {
    /// Absolute path of the file. `None`: no file, only the background
    /// colour of `options` (`veil clear`).
    pub input: Option<String>,
    pub options: ScaleOptions,
}

impl Wallpaper {
    /// The file `input`, shown with `options`.
    pub fn file(input: impl Into<String>, options: ScaleOptions) -> Self {
        Self {
            input: Some(input.into()),
            options,
        }
    }

    /// No file: the background colour of `options` alone.
    pub fn color(options: ScaleOptions) -> Self {
        Self {
            input: None,
            options,
        }
    }
}

/// The file, or the colour of a wallpaper without one, for log messages.
impl std::fmt::Display for Wallpaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.input {
            Some(input) => write!(f, "'{input}'"),
            None => write!(f, "the background colour {}", self.options.bg_color),
        }
    }
}

/// What each output shows when the daemon draws on it for the first time: at
/// start-up, or when it is connected later.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wallpapers {
    /// For every output without a wallpaper of its own: the latest `set` for
    /// every output (or `--input`). `None`: the daemon's background colour.
    pub default: Option<Wallpaper>,
    /// The wallpaper of each output, by output name: the latest one shown
    /// there.
    pub outputs: BTreeMap<String, Wallpaper>,
}

impl Wallpapers {
    /// The wallpaper of the output `name` (`None` for an output without a
    /// name, which gets the default).
    pub fn for_output(&self, name: Option<&str>) -> Option<&Wallpaper> {
        name.and_then(|name| self.outputs.get(name))
            .or(self.default.as_ref())
    }

    /// `wallpaper` is now the wallpaper of the output `name`: it is shown
    /// there, or stored to be shown when the output is back.
    pub fn shown_on(&mut self, name: &str, wallpaper: Wallpaper) {
        self.outputs.insert(name.to_string(), wallpaper);
    }

    /// A `set` for every output has loaded `wallpaper` (a video: is shown on
    /// one output), or the daemon starts with it. Each output the daemon
    /// has a screen for (`screens`, by name) gets it through
    /// [`Wallpapers::shown_on`] once it is shown there; until then, and if it
    /// cannot be shown there, it keeps the wallpaper it had. `requested` is the
    /// daemon's `--output` list (empty: it draws on every output).
    ///
    /// Without `--output`, the wallpaper becomes the default: an output the
    /// daemon has no screen for gets it when it is connected, whatever it
    /// showed in an earlier run, so its own wallpaper is forgotten. With
    /// `--output`, the default is left to daemons that draw on every output,
    /// and each requested output the daemon has no screen for yet gets the
    /// wallpaper as its own.
    pub fn set_everywhere(&mut self, wallpaper: Wallpaper, screens: &[&str], requested: &[String]) {
        if !requested.is_empty() {
            for name in requested {
                if !screens.contains(&name.as_str()) {
                    self.outputs.insert(name.clone(), wallpaper.clone());
                }
            }
            return;
        }
        let previous = self.default.replace(wallpaper);
        self.outputs
            .retain(|name, _| screens.contains(&name.as_str()));
        // An output that showed the previous default keeps it until the new
        // wallpaper is shown there.
        if let Some(previous) = previous {
            for name in screens {
                self.outputs
                    .entry((*name).to_string())
                    .or_insert_with(|| previous.clone());
            }
        }
    }
}

/// Where the state file is: `$XDG_STATE_HOME/veil/state.json`, or
/// `$HOME/.local/state/veil/state.json` when `XDG_STATE_HOME` is not set (or
/// not an absolute path, which the XDG base directory specification says to
/// ignore). `None` without either.
pub fn state_path() -> Option<PathBuf> {
    state_path_from(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
}

/// [`state_path`] from the values of `XDG_STATE_HOME` and `HOME`.
pub fn state_path_from(state_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute = |value: OsString| Some(PathBuf::from(value)).filter(|path| path.is_absolute());
    let base = state_home
        .and_then(absolute)
        .or_else(|| Some(home.and_then(absolute)?.join(".local/state")))?;
    Some(base.join("veil").join("state.json"))
}

/// The state file as it is written.
#[derive(Debug, Serialize, Deserialize)]
struct StateFile {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<Entry>,
    #[serde(default)]
    outputs: BTreeMap<String, Entry>,
}

/// One wallpaper in the state file.
#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    /// Left out for a wallpaper without a file (version 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input: Option<String>,
    mode: ScaleMode,
    bg_color: Color,
}

/// Only the version, read before the rest, so that a file of a newer version
/// is recognised as such, whatever else has changed in it.
#[derive(Deserialize)]
struct Version {
    version: u32,
}

impl From<&Wallpaper> for Entry {
    fn from(wallpaper: &Wallpaper) -> Self {
        Self {
            input: wallpaper.input.clone(),
            mode: wallpaper.options.mode,
            bg_color: wallpaper.options.bg_color,
        }
    }
}

impl Entry {
    /// The wallpaper of an entry in a file of format `version`.
    fn into_wallpaper(self, version: u32) -> Result<Wallpaper, String> {
        match &self.input {
            Some(input) if !Path::new(input).is_absolute() => {
                return Err(format!("'{input}' is not an absolute path"));
            }
            Some(_) => {}
            None if version < 2 => return Err("an entry has no input".into()),
            None => {}
        }
        Ok(Wallpaper {
            input: self.input,
            options: ScaleOptions {
                mode: self.mode,
                bg_color: self.bg_color,
            },
        })
    }
}

/// What reading the state file found.
#[derive(Debug, PartialEq, Eq)]
pub enum Read {
    /// There is no state file.
    Missing,
    Wallpapers(Wallpapers),
    /// A newer veil wrote it, in this version of the format.
    Newer(u32),
    /// It cannot be used, for this reason.
    Invalid(String),
}

/// The bytes of the state file for `wallpapers`, in the oldest version of the
/// format that holds them: 1 unless a wallpaper has no file.
pub fn encode(wallpapers: &Wallpapers) -> Vec<u8> {
    let without_file = wallpapers
        .default
        .iter()
        .chain(wallpapers.outputs.values())
        .any(|wallpaper| wallpaper.input.is_none());
    let file = StateFile {
        version: if without_file { STATE_VERSION } else { 1 },
        default: wallpapers.default.as_ref().map(Entry::from),
        outputs: wallpapers
            .outputs
            .iter()
            .map(|(name, wallpaper)| (name.clone(), Entry::from(wallpaper)))
            .collect(),
    };
    // A struct of strings and maps cannot fail to serialise; an empty file
    // would only be read as invalid.
    let mut bytes = serde_json::to_vec_pretty(&file).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

/// What the bytes of a state file say.
pub fn decode(bytes: &[u8]) -> Read {
    // serde would also take an array for a struct.
    let value = match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(value @ serde_json::Value::Object(_)) => value,
        Ok(_) => return Read::Invalid("not a JSON object".into()),
        Err(e) => return Read::Invalid(e.to_string()),
    };
    let version = match Version::deserialize(&value) {
        Ok(Version { version }) => version,
        Err(e) => return Read::Invalid(e.to_string()),
    };
    if version > STATE_VERSION {
        return Read::Newer(version);
    }
    if version == 0 {
        return Read::Invalid("version 0 does not exist".into());
    }
    let file = match StateFile::deserialize(value) {
        Ok(file) => file,
        Err(e) => return Read::Invalid(e.to_string()),
    };
    let decoded = (|| {
        Ok::<_, String>(Wallpapers {
            default: file
                .default
                .map(|entry| entry.into_wallpaper(version))
                .transpose()?,
            outputs: file
                .outputs
                .into_iter()
                .map(|(name, entry)| Ok((name, entry.into_wallpaper(version)?)))
                .collect::<Result<_, String>>()?,
        })
    })();
    match decoded {
        Ok(wallpapers) => Read::Wallpapers(wallpapers),
        Err(reason) => Read::Invalid(reason),
    }
}

/// Reads the state file at `path`.
pub fn read(path: &Path) -> Read {
    match std::fs::read(path) {
        Ok(bytes) => decode(&bytes),
        Err(e) if e.kind() == ErrorKind::NotFound => Read::Missing,
        Err(e) => Read::Invalid(e.to_string()),
    }
}

/// Replaces the file at `path` with `bytes`, so that a reader finds either the
/// old content or the new one, never a part: the bytes go to a temporary file
/// in the same directory, which is synced and then renamed over `path`. The
/// directories are created (mode `0700`) if they do not exist; the file has
/// mode `0600`.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "no parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "no file name"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    // Another daemon (of another compositor) may write the same file.
    let mut temporary = OsString::from(".");
    temporary.push(name);
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = dir.join(temporary);

    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return written;
    }
    // The rename itself is on disk once the directory is synced.
    std::fs::File::open(dir)?.sync_all()
}

/// The state file of the daemon: what it says, and the writes of new content.
///
/// A write happens on a short-lived thread (`veil-state`), because the disk
/// may be slow and the daemon calls this from its event loop. Writes are
/// numbered, and a thread does not write content older than what is on disk
/// already, so the file ends with the latest state however the threads are
/// scheduled. Dropping the store waits for the writes that are under way.
pub struct Store {
    /// Where the file is written. `None`: nowhere (there is no state
    /// directory, or the file belongs to a newer veil).
    path: Option<PathBuf>,
    /// What the file says, as far as this daemon knows: what it read, changed
    /// by what it has written since.
    saved: Wallpapers,
    /// The number of the latest write handed to a thread.
    generation: u64,
    /// The number of the latest write that reached the disk.
    on_disk: Arc<Mutex<u64>>,
    writers: Vec<JoinHandle<()>>,
}

impl Store {
    /// Reads the state file at `path` (`None`: there is none to read or
    /// write) and keeps its content. What cannot be read is logged, and the
    /// daemon starts with nothing saved.
    pub fn open(path: Option<PathBuf>) -> Self {
        let mut store = Self {
            path: None,
            saved: Wallpapers::default(),
            generation: 0,
            on_disk: Arc::new(Mutex::new(0)),
            writers: Vec::new(),
        };
        let Some(path) = path else {
            tracing::warn!(
                "neither XDG_STATE_HOME nor HOME is an absolute path; wallpapers are not saved \
                 across restarts"
            );
            return store;
        };
        match read(&path) {
            Read::Missing => {
                tracing::debug!("no saved wallpapers ({} does not exist)", path.display());
            }
            Read::Wallpapers(wallpapers) => {
                tracing::debug!("saved wallpapers read from {}", path.display());
                store.saved = wallpapers;
            }
            Read::Newer(version) => {
                tracing::warn!(
                    "{} was written by a newer veil (format version {version}, this one reads \
                     {STATE_VERSION}); not restoring from it, and not replacing it",
                    path.display()
                );
                return store;
            }
            Read::Invalid(reason) => {
                tracing::warn!(
                    "{} cannot be used ({reason}); starting without saved wallpapers, the next \
                     `veil set` replaces it",
                    path.display()
                );
            }
        }
        store.path = Some(path);
        store
    }

    /// What the file says.
    pub fn saved(&self) -> &Wallpapers {
        &self.saved
    }

    /// Changes what is saved with `change`, and writes the file if that made a
    /// difference.
    pub fn update(&mut self, change: impl FnOnce(&mut Wallpapers)) {
        let before = self.saved.clone();
        change(&mut self.saved);
        if self.saved != before {
            self.write();
        }
    }

    fn write(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        self.writers.retain(|writer| !writer.is_finished());
        self.generation += 1;
        let generation = self.generation;
        let bytes = encode(&self.saved);
        let on_disk = Arc::clone(&self.on_disk);
        let spawned = std::thread::Builder::new()
            .name("veil-state".into())
            .spawn(move || {
                let mut on_disk = on_disk.lock().unwrap_or_else(PoisonError::into_inner);
                if *on_disk > generation {
                    // A thread that started later has written newer content.
                    return;
                }
                match write_atomically(&path, &bytes) {
                    Ok(()) => {
                        *on_disk = generation;
                        tracing::debug!("saved the wallpapers in {}", path.display());
                    }
                    Err(e) => {
                        tracing::warn!("cannot save the wallpapers in {}: {e}", path.display())
                    }
                }
            });
        match spawned {
            Ok(writer) => self.writers.push(writer),
            Err(e) => tracing::warn!("cannot start a thread to save the wallpapers: {e}"),
        }
    }

    /// Blocks until every write that was started is done.
    pub fn wait(&mut self) {
        for writer in self.writers.drain(..) {
            let _ = writer.join();
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn wallpaper(input: &str) -> Wallpaper {
        Wallpaper::file(input, ScaleOptions::default())
    }

    fn fitted(input: &str) -> Wallpaper {
        Wallpaper {
            input: Some(input.into()),
            options: ScaleOptions {
                mode: ScaleMode::Fit,
                bg_color: Color {
                    r: 0x10,
                    g: 0x20,
                    b: 0x30,
                },
            },
        }
    }

    fn outputs(entries: &[(&str, Wallpaper)]) -> BTreeMap<String, Wallpaper> {
        entries
            .iter()
            .map(|(name, wallpaper)| ((*name).to_string(), wallpaper.clone()))
            .collect()
    }

    /// A directory of its own for one test, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("veil-persist-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create the test directory");
            Self(dir)
        }

        /// The state file of a daemon whose `XDG_STATE_HOME` is this directory.
        fn state_file(&self) -> PathBuf {
            state_path_from(Some(self.0.clone().into_os_string()), None).expect("absolute")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_file_is_in_the_state_directory() {
        let path = |state_home: Option<&str>, home: Option<&str>| {
            state_path_from(state_home.map(Into::into), home.map(Into::into))
        };
        assert_eq!(
            path(Some("/state"), Some("/home/u")),
            Some(PathBuf::from("/state/veil/state.json"))
        );
        assert_eq!(
            path(None, Some("/home/u")),
            Some(PathBuf::from("/home/u/.local/state/veil/state.json"))
        );
        // A relative XDG_STATE_HOME is ignored, as the specification says.
        for state_home in ["", "state", "./state"] {
            assert_eq!(
                path(Some(state_home), Some("/home/u")),
                Some(PathBuf::from("/home/u/.local/state/veil/state.json")),
                "{state_home:?}"
            );
        }
        assert_eq!(path(None, None), None);
        assert_eq!(path(Some("state"), Some("home")), None);
        assert_eq!(path(None, Some("")), None);
    }

    #[test]
    fn an_output_gets_its_own_wallpaper_or_the_default() {
        let mut wallpapers = Wallpapers::default();
        assert_eq!(wallpapers.for_output(Some("DP-3")), None);
        assert_eq!(wallpapers.for_output(None), None);

        wallpapers.default = Some(wallpaper("/d.png"));
        wallpapers.shown_on("DP-3", wallpaper("/a.mp4"));
        assert_eq!(
            wallpapers.for_output(Some("DP-3")),
            Some(&wallpaper("/a.mp4"))
        );
        assert_eq!(
            wallpapers.for_output(Some("HDMI-A-1")),
            Some(&wallpaper("/d.png"))
        );
        // An output without a name cannot have one of its own.
        assert_eq!(wallpapers.for_output(None), Some(&wallpaper("/d.png")));
    }

    #[test]
    fn a_set_for_every_output_becomes_the_default_and_forgets_outputs_that_are_not_here() {
        let mut wallpapers = Wallpapers {
            default: Some(wallpaper("/old.png")),
            outputs: outputs(&[
                ("DP-3", wallpaper("/a.mp4")),
                ("eDP-1", wallpaper("/laptop.png")),
            ]),
        };
        // DP-3 and HDMI-A-1 are the daemon's screens; eDP-1 was seen in an
        // earlier run only.
        wallpapers.set_everywhere(wallpaper("/new.png"), &["DP-3", "HDMI-A-1"], &[]);
        assert_eq!(wallpapers.default, Some(wallpaper("/new.png")));
        assert_eq!(
            wallpapers.outputs,
            // Each screen keeps what it shows until the new one is shown
            // there; HDMI-A-1 showed the old default.
            outputs(&[
                ("DP-3", wallpaper("/a.mp4")),
                ("HDMI-A-1", wallpaper("/old.png")),
            ])
        );
        assert_eq!(
            wallpapers.for_output(Some("eDP-1")),
            Some(&wallpaper("/new.png"))
        );

        // Shown on both.
        wallpapers.shown_on("DP-3", wallpaper("/new.png"));
        wallpapers.shown_on("HDMI-A-1", wallpaper("/new.png"));
        for name in ["DP-3", "HDMI-A-1", "eDP-1"] {
            assert_eq!(
                wallpapers.for_output(Some(name)),
                Some(&wallpaper("/new.png")),
                "{name}"
            );
        }
    }

    #[test]
    fn a_set_for_every_output_that_fails_on_one_leaves_it_its_wallpaper() {
        let mut wallpapers = Wallpapers::default();
        // Nothing before: an output where the new one fails shows the
        // background, and nothing is kept for it.
        wallpapers.set_everywhere(wallpaper("/a.png"), &["DP-3"], &[]);
        assert_eq!(wallpapers.outputs, BTreeMap::new());
        wallpapers.shown_on("DP-3", wallpaper("/a.png"));

        wallpapers.set_everywhere(wallpaper("/b.mp4"), &["DP-3", "HDMI-A-1"], &[]);
        // It starts on HDMI-A-1 and fails on DP-3.
        wallpapers.shown_on("HDMI-A-1", wallpaper("/b.mp4"));
        assert_eq!(
            wallpapers.for_output(Some("DP-3")),
            Some(&wallpaper("/a.png"))
        );
        assert_eq!(
            wallpapers.for_output(Some("HDMI-A-1")),
            Some(&wallpaper("/b.mp4"))
        );
        assert_eq!(
            wallpapers.for_output(Some("new")),
            Some(&wallpaper("/b.mp4"))
        );
    }

    #[test]
    fn with_output_a_set_for_every_output_is_for_the_requested_ones() {
        let requested = ["DP-3".to_string(), "HDMI-A-1".to_string()];
        let mut wallpapers = Wallpapers {
            default: Some(wallpaper("/other-daemon.png")),
            outputs: outputs(&[("eDP-1", wallpaper("/laptop.png"))]),
        };
        // HDMI-A-1 has not been connected yet: it gets the wallpaper when it is.
        wallpapers.set_everywhere(wallpaper("/a.png"), &["DP-3"], &requested);
        assert_eq!(wallpapers.default, Some(wallpaper("/other-daemon.png")));
        assert_eq!(
            wallpapers.outputs,
            outputs(&[
                ("HDMI-A-1", wallpaper("/a.png")),
                ("eDP-1", wallpaper("/laptop.png")),
            ])
        );
        // DP-3 gets it when it is shown there.
        assert_eq!(
            wallpapers.for_output(Some("DP-3")),
            Some(&wallpaper("/other-daemon.png"))
        );

        // `--input` at start-up: no screens yet.
        wallpapers.set_everywhere(wallpaper("/b.png"), &[], &requested);
        assert_eq!(
            wallpapers.for_output(Some("DP-3")),
            Some(&wallpaper("/b.png"))
        );
        assert_eq!(
            wallpapers.for_output(Some("HDMI-A-1")),
            Some(&wallpaper("/b.png"))
        );
    }

    #[test]
    fn input_at_start_up_replaces_everything_saved() {
        let mut wallpapers = Wallpapers {
            default: Some(wallpaper("/old.png")),
            outputs: outputs(&[("DP-3", wallpaper("/a.mp4"))]),
        };
        wallpapers.set_everywhere(wallpaper("/input.png"), &[], &[]);
        assert_eq!(
            wallpapers,
            Wallpapers {
                default: Some(wallpaper("/input.png")),
                outputs: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn the_file_format_is_pinned() {
        let wallpapers = Wallpapers {
            default: Some(wallpaper("/srv/d.png")),
            outputs: outputs(&[("DP-3", fitted("/srv/a b.mp4"))]),
        };
        let text = String::from_utf8(encode(&wallpapers)).expect("UTF-8");
        assert_eq!(
            text,
            r##"{
  "version": 1,
  "default": {
    "input": "/srv/d.png",
    "mode": "fill",
    "bg_color": "#000000"
  },
  "outputs": {
    "DP-3": {
      "input": "/srv/a b.mp4",
      "mode": "fit",
      "bg_color": "#102030"
    }
  }
}
"##
        );
        assert_eq!(
            decode(text.as_bytes()),
            Read::Wallpapers(wallpapers.clone())
        );

        let empty = Wallpapers::default();
        let text = String::from_utf8(encode(&empty)).expect("UTF-8");
        assert_eq!(text, "{\n  \"version\": 1,\n  \"outputs\": {}\n}\n");
        assert_eq!(decode(text.as_bytes()), Read::Wallpapers(empty));
    }

    #[test]
    fn a_file_may_leave_out_what_it_does_not_have_and_add_what_this_version_does_not_know() {
        assert_eq!(
            decode(br#"{"version":1}"#),
            Read::Wallpapers(Wallpapers::default())
        );
        // Fields of a later version that did not need a new format version.
        assert_eq!(
            decode(
                br##"{"version":1,"since":"2026","outputs":{"DP-3":
                    {"input":"/a.png","mode":"fit","bg_color":"#102030","loop":"x"}}}"##
            ),
            Read::Wallpapers(Wallpapers {
                default: None,
                outputs: outputs(&[("DP-3", fitted("/a.png"))]),
            })
        );
    }

    #[test]
    fn a_wallpaper_without_a_file_needs_version_2() {
        let cleared = Wallpaper::color(ScaleOptions {
            mode: ScaleMode::Fill,
            bg_color: Color {
                r: 0x10,
                g: 0x20,
                b: 0x30,
            },
        });
        let wallpapers = Wallpapers {
            default: Some(wallpaper("/srv/d.png")),
            outputs: outputs(&[("DP-3", cleared)]),
        };
        let text = String::from_utf8(encode(&wallpapers)).expect("UTF-8");
        assert_eq!(
            text,
            r##"{
  "version": 2,
  "default": {
    "input": "/srv/d.png",
    "mode": "fill",
    "bg_color": "#000000"
  },
  "outputs": {
    "DP-3": {
      "mode": "fill",
      "bg_color": "#102030"
    }
  }
}
"##
        );
        assert_eq!(decode(text.as_bytes()), Read::Wallpapers(wallpapers));
        // Version 1 had no such entries: one without a file is broken.
        assert!(matches!(
            decode(br##"{"version":1,"outputs":{"DP-3":{"mode":"fill","bg_color":"#102030"}}}"##),
            Read::Invalid(_)
        ));
    }

    #[test]
    fn a_wallpaper_says_what_it_is() {
        assert_eq!(wallpaper("/a b.png").to_string(), "'/a b.png'");
        assert_eq!(
            Wallpaper::color(ScaleOptions::default()).to_string(),
            "the background colour #000000"
        );
    }

    #[test]
    fn a_file_of_a_newer_version_is_recognised_whatever_it_holds() {
        assert_eq!(decode(br#"{"version":3}"#), Read::Newer(3));
        assert_eq!(
            decode(br#"{"version":7,"outputs":{"DP-3":{"playlist":["/a.png"]}}}"#),
            Read::Newer(7)
        );
    }

    #[test]
    fn a_corrupt_file_is_invalid() {
        for bytes in [
            &b""[..],
            b"not json",
            b"{",
            br#"{"version":1,"default":{"input":"/a.png","mode":"fill""#,
            b"[1]",
            br#"{}"#,
            br#"{"version":"1"}"#,
            br#"{"version":0}"#,
            br#"{"version":-1}"#,
            br#"{"version":1,"outputs":[]}"#,
            br##"{"version":1,"default":{"input":"/a.png","mode":"zoom","bg_color":"#000000"}}"##,
            br#"{"version":1,"default":{"input":"/a.png","mode":"fill","bg_color":"red"}}"#,
            br#"{"version":1,"default":{"input":"/a.png"}}"#,
            br##"{"version":1,"outputs":{"DP-3":{"input":"a.png","mode":"fill","bg_color":"#000000"}}}"##,
            b"\xff\xfe",
        ] {
            assert!(
                matches!(decode(bytes), Read::Invalid(_)),
                "{:?}: {:?}",
                String::from_utf8_lossy(bytes),
                decode(bytes)
            );
        }
    }

    #[test]
    fn what_is_saved_is_read_back() {
        let dir = TempDir::new("round-trip");
        let path = dir.state_file();
        assert_eq!(read(&path), Read::Missing);

        let mut store = Store::open(Some(path.clone()));
        assert_eq!(store.saved(), &Wallpapers::default());
        store.update(|w| w.set_everywhere(wallpaper("/d.png"), &[], &[]));
        store.update(|w| w.shown_on("DP-3", fitted("/a.mp4")));
        store.wait();
        let expected = Wallpapers {
            default: Some(wallpaper("/d.png")),
            outputs: outputs(&[("DP-3", fitted("/a.mp4"))]),
        };
        assert_eq!(read(&path), Read::Wallpapers(expected.clone()));

        // The next daemon starts with it.
        drop(store);
        let store = Store::open(Some(path.clone()));
        assert_eq!(store.saved(), &expected);

        // Created for the user alone, and nothing left next to it.
        let mode = |path: &Path| {
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(&path), 0o600);
        let veil_dir = path.parent().expect("the veil directory");
        assert_eq!(mode(veil_dir), 0o700);
        let names: Vec<_> = std::fs::read_dir(veil_dir)
            .expect("read the veil directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, ["state.json"]);
    }

    #[test]
    fn the_latest_of_many_writes_is_what_stays() {
        let dir = TempDir::new("latest");
        let path = dir.state_file();
        let mut store = Store::open(Some(path.clone()));
        for n in 0..20 {
            store.update(|w| w.shown_on("DP-3", wallpaper(&format!("/{n}.png"))));
        }
        drop(store);
        assert_eq!(
            read(&path),
            Read::Wallpapers(Wallpapers {
                default: None,
                outputs: outputs(&[("DP-3", wallpaper("/19.png"))]),
            })
        );
    }

    #[test]
    fn nothing_is_written_when_nothing_changed() {
        let dir = TempDir::new("unchanged");
        let path = dir.state_file();
        let mut store = Store::open(Some(path.clone()));
        store.update(|w| w.shown_on("DP-3", wallpaper("/a.png")));
        store.wait();
        // Replaced behind the store's back: an update that changes nothing
        // does not write.
        std::fs::write(&path, "marker").expect("overwrite");
        store.update(|w| w.shown_on("DP-3", wallpaper("/a.png")));
        store.wait();
        assert_eq!(std::fs::read(&path).expect("read"), b"marker");
    }

    #[test]
    fn a_corrupt_file_is_replaced_by_the_next_save() {
        let dir = TempDir::new("corrupt");
        let path = dir.state_file();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create");
        std::fs::write(&path, b"{\"version\":1,\"outp").expect("write a truncated file");
        assert!(matches!(read(&path), Read::Invalid(_)));

        let mut store = Store::open(Some(path.clone()));
        assert_eq!(store.saved(), &Wallpapers::default());
        store.update(|w| w.shown_on("DP-3", wallpaper("/a.png")));
        store.wait();
        assert_eq!(
            read(&path),
            Read::Wallpapers(Wallpapers {
                default: None,
                outputs: outputs(&[("DP-3", wallpaper("/a.png"))]),
            })
        );
    }

    #[test]
    fn a_file_of_a_newer_veil_is_left_alone() {
        let dir = TempDir::new("newer");
        let path = dir.state_file();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create");
        let newer = br#"{"version":3,"outputs":{"DP-3":{"playlist":["/a.png"]}}}"#;
        std::fs::write(&path, newer).expect("write");

        let mut store = Store::open(Some(path.clone()));
        assert_eq!(store.saved(), &Wallpapers::default());
        store.update(|w| w.shown_on("DP-3", wallpaper("/b.png")));
        store.wait();
        assert_eq!(std::fs::read(&path).expect("read"), newer);
    }

    #[test]
    fn a_store_without_a_path_saves_nothing_and_does_not_fail() {
        let mut store = Store::open(None);
        store.update(|w| w.shown_on("DP-3", wallpaper("/a.png")));
        assert_eq!(
            store.saved().for_output(Some("DP-3")),
            Some(&wallpaper("/a.png"))
        );
    }

    #[test]
    fn a_write_that_fails_leaves_the_old_file() {
        let dir = TempDir::new("read-only");
        let path = dir.state_file();
        let mut store = Store::open(Some(path.clone()));
        store.update(|w| w.shown_on("DP-3", wallpaper("/a.png")));
        store.wait();

        let veil_dir = path.parent().expect("parent").to_path_buf();
        std::fs::set_permissions(&veil_dir, std::fs::Permissions::from_mode(0o500))
            .expect("make the directory read-only");
        let probe = veil_dir.join("probe");
        let writable = std::fs::write(&probe, b"").is_ok();
        if writable {
            // Root can write anyway.
            let _ = std::fs::remove_file(&probe);
        } else {
            store.update(|w| w.shown_on("DP-3", wallpaper("/b.png")));
            store.wait();
        }
        std::fs::set_permissions(&veil_dir, std::fs::Permissions::from_mode(0o700))
            .expect("restore the permissions");
        if writable {
            eprintln!("skipped: the directory is writable without permission");
            return;
        }
        assert_eq!(
            read(&path),
            Read::Wallpapers(Wallpapers {
                default: None,
                outputs: outputs(&[("DP-3", wallpaper("/a.png"))]),
            })
        );
    }

    #[test]
    fn an_unreadable_path_is_invalid_and_not_missing() {
        let dir = TempDir::new("directory");
        let path = dir.state_file();
        // A directory where the file should be.
        std::fs::create_dir_all(&path).expect("create");
        assert!(matches!(read(&path), Read::Invalid(_)));
    }
}
