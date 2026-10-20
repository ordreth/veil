//! `docs/ipc.md` against the protocol types: every command, field, error
//! code, result field and value the code has is documented, nothing the code
//! does not have is, and every example request and response reads as the
//! types say. The conventions this relies on are written at the top of the
//! document.
//!
//! The lists below match on every variant, without a wildcard: a new command,
//! code, state or kind does not compile until it is added here, and this test
//! then fails until it is documented.

use clap::{CommandFactory, ValueEnum};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeSet, VecDeque};
use veil::config::Cli;
use veil::error;
use veil::ipc::client::RESPONSE_TIMEOUT;
use veil::ipc::protocol::{ErrorCode, Outcome, Request, Response, PROTOCOL_VERSION};
use veil::ipc::report::{
    Kind, OutputInfo, OutputList, OutputStatus, Size, State, Status, Toggled, VideoInfo,
    WallpaperInfo,
};
use veil::ipc::server::{MAX_CONNECTIONS, MAX_REQUEST_LEN, READ_TIMEOUT};
use veil::policy::PauseOn;
use veil::scaling::{Color, ScaleMode};

const DOC: &str = include_str!("../docs/ipc.md");

/// One line of the document, and whether it is inside a fenced code block
/// (the fences themselves count as inside).
struct Line<'a> {
    text: &'a str,
    code: bool,
}

fn lines() -> Vec<Line<'static>> {
    let mut code = false;
    DOC.lines()
        .map(|text| {
            let fence = text.starts_with("```");
            if fence {
                code = !code;
            }
            Line {
                text,
                code: code || fence,
            }
        })
        .collect()
}

/// The level and title of a heading, outside code blocks.
fn heading(line: &Line<'static>) -> Option<(usize, &'static str)> {
    if line.code {
        return None;
    }
    let level = line.text.chars().take_while(|&c| c == '#').count();
    let title = line.text.get(level..)?.strip_prefix(' ')?;
    (level > 0).then_some((level, title))
}

/// The lines of the one section titled `title`, up to the next heading of
/// the same or a higher level.
fn section(title: &str) -> Vec<Line<'static>> {
    let lines = lines();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| heading(line).is_some_and(|(_, t)| t == title))
        .map(|(index, _)| index)
        .collect();
    let [start] = starts[..] else {
        panic!("docs/ipc.md has {} sections titled {title:?}", starts.len());
    };
    let level = heading(&lines[start]).map_or(0, |(level, _)| level);
    lines
        .into_iter()
        .skip(start + 1)
        .take_while(|line| heading(line).is_none_or(|(l, _)| l > level))
        .collect()
}

/// The names in backticks in the first cell of the table rows of a section.
fn rows(title: &str) -> BTreeSet<String> {
    section(title)
        .iter()
        .filter(|line| !line.code)
        .filter_map(|line| line.text.strip_prefix("| `"))
        .filter_map(|rest| rest.split_once('`'))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// The keys of the JSON object `value` serializes to.
fn keys(value: &impl Serialize) -> BTreeSet<String> {
    match serde_json::to_value(value).expect("serialize") {
        Value::Object(map) => map.keys().cloned().collect(),
        other => panic!("not an object: {other}"),
    }
}

/// The name `value` has on the wire.
fn wire_name(value: &impl Serialize) -> String {
    match serde_json::to_value(value).expect("serialize") {
        Value::String(name) => name,
        other => panic!("not a string: {other}"),
    }
}

/// Every command, with every field it has set.
fn every_request() -> Vec<Request> {
    let output = || Some("DP-3".to_string());
    let color = Some(Color { r: 1, g: 2, b: 3 });
    let requests = vec![
        Request::Set {
            output: output(),
            input: "/a.png".into(),
            mode: Some(ScaleMode::Fit),
            bg_color: color,
            persist: false,
        },
        Request::Clear {
            output: output(),
            bg_color: color,
            persist: false,
        },
        Request::Pause { output: output() },
        Request::Resume { output: output() },
        Request::Toggle { output: output() },
        Request::Status {},
        Request::Outputs {},
        Request::Quit {},
        Request::Ping {},
    ];
    // A new command does not compile here: add it above too.
    let covered: BTreeSet<usize> = requests
        .iter()
        .map(|request| match request {
            Request::Set { .. } => 0,
            Request::Clear { .. } => 1,
            Request::Pause { .. } => 2,
            Request::Resume { .. } => 3,
            Request::Toggle { .. } => 4,
            Request::Status {} => 5,
            Request::Outputs {} => 6,
            Request::Quit {} => 7,
            Request::Ping {} => 8,
        })
        .collect();
    assert_eq!(covered, (0..9).collect(), "every command, once");
    requests
}

fn every_error_code() -> Vec<ErrorCode> {
    let codes = vec![
        ErrorCode::InvalidRequest,
        ErrorCode::TooLarge,
        ErrorCode::Timeout,
        ErrorCode::Forbidden,
        ErrorCode::Busy,
        ErrorCode::Superseded,
        ErrorCode::UnknownOutput,
        ErrorCode::NotFound,
        ErrorCode::UnsupportedMedia,
        ErrorCode::Failed,
    ];
    let covered: BTreeSet<usize> = codes
        .iter()
        .map(|code| match code {
            ErrorCode::InvalidRequest => 0,
            ErrorCode::TooLarge => 1,
            ErrorCode::Timeout => 2,
            ErrorCode::Forbidden => 3,
            ErrorCode::Busy => 4,
            ErrorCode::Superseded => 5,
            ErrorCode::UnknownOutput => 6,
            ErrorCode::NotFound => 7,
            ErrorCode::UnsupportedMedia => 8,
            ErrorCode::Failed => 9,
            // Never sent: what a client reads a code it does not know as.
            ErrorCode::Unknown => 10,
        })
        .collect();
    assert_eq!(covered, (0..10).collect(), "every code that is sent, once");
    codes
}

fn every_state() -> Vec<State> {
    let states = vec![
        State::Disconnected,
        State::Starting,
        State::Shown,
        State::Playing,
        State::Paused,
        State::Released,
        State::Stopped,
    ];
    let covered: BTreeSet<usize> = states
        .iter()
        .map(|state| match state {
            State::Disconnected => 0,
            State::Starting => 1,
            State::Shown => 2,
            State::Playing => 3,
            State::Paused => 4,
            State::Released => 5,
            State::Stopped => 6,
            State::Unknown => 7,
        })
        .collect();
    assert_eq!(covered, (0..7).collect(), "every state that is sent, once");
    states
}

fn every_kind() -> Vec<Kind> {
    let kinds = vec![Kind::Color, Kind::Image, Kind::Video];
    let covered: BTreeSet<usize> = kinds
        .iter()
        .map(|kind| match kind {
            Kind::Color => 0,
            Kind::Image => 1,
            Kind::Video => 2,
            Kind::Unknown => 3,
        })
        .collect();
    assert_eq!(covered, (0..3).collect(), "every kind that is sent, once");
    kinds
}

fn size() -> Size {
    Size {
        width: 1920,
        height: 1080,
    }
}

fn wallpaper() -> WallpaperInfo {
    WallpaperInfo {
        input: Some("/a.mp4".into()),
        mode: ScaleMode::Fill,
        bg_color: Color::default(),
    }
}

fn video() -> VideoInfo {
    VideoInfo {
        size: Some(size()),
        fps: Some(30.0),
        rendered: Some(1),
        dropped: Some(0),
    }
}

fn output_status() -> OutputStatus {
    OutputStatus {
        name: Some("DP-3".into()),
        connected: true,
        wallpaper: wallpaper(),
        kind: Kind::Video,
        state: State::Playing,
        paused_by_user: false,
        hidden: false,
        size: Some(size()),
        scale: Some(1.0),
        video: Some(video()),
        next: None,
    }
}

fn status() -> Status {
    Status {
        version: "0.1.0".into(),
        pid: 1,
        uptime_secs: 1,
        pause_on: PauseOn::Fullscreen,
        default: None,
        outputs: vec![output_status()],
    }
}

fn output_info() -> OutputInfo {
    OutputInfo {
        name: Some("DP-3".into()),
        description: None,
        connected: true,
        wallpaper: true,
    }
}

#[test]
fn every_command_and_its_fields_are_documented() {
    let requests = every_request();
    for request in &requests {
        let mut fields = keys(request);
        fields.remove("cmd");
        assert_eq!(
            rows(&format!("`{}`", request.name())),
            fields,
            "the field table of `{}`",
            request.name()
        );
    }
    let commands: BTreeSet<String> = section("Requests")
        .iter()
        .filter_map(heading)
        .filter_map(|(_, title)| title.strip_prefix('`')?.strip_suffix('`'))
        .map(String::from)
        .collect();
    let names: BTreeSet<String> = requests.iter().map(|r| r.name().to_string()).collect();
    assert_eq!(commands, names, "a section per command, and no other");
}

#[test]
fn the_response_fields_are_documented() {
    let mut fields = keys(&Response::error(ErrorCode::Failed, "m"));
    fields.extend(keys(&Response::report(&Toggled { paused: true })));
    assert_eq!(rows("Fields"), fields);
}

#[test]
fn every_error_code_is_documented() {
    let codes = every_error_code().iter().map(wire_name).collect();
    assert_eq!(rows("Error codes"), codes);
}

#[test]
fn every_result_field_is_documented() {
    for (title, fields) in [
        ("`Status`", keys(&status())),
        ("`OutputStatus`", keys(&output_status())),
        ("`WallpaperInfo`", keys(&wallpaper())),
        ("`VideoInfo`", keys(&video())),
        ("`Size`", keys(&size())),
        (
            "`OutputList`",
            keys(&OutputList {
                outputs: vec![output_info()],
            }),
        ),
        ("`OutputInfo`", keys(&output_info())),
        ("`Toggled`", keys(&Toggled { paused: true })),
    ] {
        assert_eq!(rows(title), fields, "{title}");
    }
}

#[test]
fn every_value_is_documented() {
    fn names<T: Serialize>(values: impl IntoIterator<Item = T>) -> BTreeSet<String> {
        values.into_iter().map(|value| wire_name(&value)).collect()
    }
    assert_eq!(rows("`state`"), names(every_state()));
    assert_eq!(rows("`kind`"), names(every_kind()));
    assert_eq!(rows("`mode`"), names(ScaleMode::value_variants()));
    assert_eq!(rows("`pause_on`"), names(PauseOn::value_variants()));
}

#[test]
fn the_limits_are_the_daemon_s() {
    let text: String = section("Limits")
        .iter()
        .map(|line| format!("{}\n", line.text))
        .collect();
    for value in [
        format!("| {} s |", READ_TIMEOUT.as_secs()),
        format!("| {} KiB |", MAX_REQUEST_LEN / 1024),
        format!("| {MAX_CONNECTIONS} |"),
        format!("| {} s |", RESPONSE_TIMEOUT.as_secs()),
    ] {
        assert!(text.contains(&value), "{value:?} in the limits");
    }
    // The same numbers in the error codes.
    let codes: String = section("Error codes")
        .iter()
        .map(|line| format!("{}\n", line.text))
        .collect();
    for value in [
        format!("within {} seconds", READ_TIMEOUT.as_secs()),
        format!("longer than {} KiB", MAX_REQUEST_LEN / 1024),
        format!("{MAX_CONNECTIONS} clients"),
    ] {
        assert!(codes.contains(&value), "{value:?} in the error codes");
    }
}

#[test]
fn every_exit_status_is_documented() {
    let statuses = [
        0,
        error::EXIT_FAILURE,
        error::EXIT_USAGE,
        error::EXIT_DAEMON_NOT_RUNNING,
        error::EXIT_ALREADY_RUNNING,
        error::EXIT_COMPOSITOR_GONE,
    ]
    .iter()
    .map(u8::to_string)
    .collect();
    assert_eq!(rows("Exit statuses"), statuses);
}

#[test]
fn every_veil_command_is_listed() {
    let text: String = section("Commands")
        .iter()
        .map(|line| format!("{}\n", line.text))
        .collect();
    let cli = Cli::command();
    for command in cli.get_subcommands().filter(|c| !c.is_hide_set()) {
        let mut names: Vec<String> = vec![command.get_name().to_string()];
        names.extend(command.get_visible_aliases().map(String::from));
        for name in names {
            let mut spelt = vec![format!("veil {name}")];
            let nested: Vec<_> = command
                .get_subcommands()
                .filter(|c| !c.is_hide_set())
                .map(|c| format!("veil {name} {}", c.get_name()))
                .collect();
            if !nested.is_empty() {
                spelt = nested;
            }
            for spelt in spelt {
                assert!(
                    text.contains(&format!("`{spelt}")),
                    "`{spelt}` in the commands"
                );
            }
        }
    }
}

/// `value`, read as `T` and written back, is `value` again: no field is
/// missing, none is unknown, and every value has the right type.
fn reads_as<T: Serialize + DeserializeOwned>(value: &Value, what: &str) {
    let typed: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("{what} does not read: {e}\n{value}"));
    let back = serde_json::to_value(&typed).expect("serialize");
    assert_eq!(&back, value, "{what} has fields or values the type has not");
}

/// A documented response: it reads as a `Response` of this protocol version,
/// field for field, and its result as the type of `cmd` (when it is known).
fn check_response(json: &str, cmd: Option<&str>) -> Response {
    let value: Value =
        serde_json::from_str(json).unwrap_or_else(|e| panic!("not JSON: {e}\n{json}"));
    let response: Response = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("not a response: {e}\n{json}"));
    assert_eq!(response.version, PROTOCOL_VERSION, "{json}");
    let mut envelope = value.clone();
    if let Some(map) = envelope.as_object_mut() {
        map.remove("result");
    }
    let mut written = serde_json::to_value(&response).expect("serialize");
    if let Some(map) = written.as_object_mut() {
        map.remove("result");
    }
    assert_eq!(written, envelope, "the envelope of {json}");
    if let Some(result) = &response.result {
        // Without the command, the result is the type whose fields it has.
        let cmd = cmd.or_else(|| {
            let fields: BTreeSet<String> = result.as_object()?.keys().cloned().collect();
            [
                ("status", keys(&status())),
                (
                    "outputs",
                    keys(&OutputList {
                        outputs: Vec::new(),
                    }),
                ),
                ("toggle", keys(&Toggled { paused: true })),
            ]
            .into_iter()
            .find(|(_, keys)| *keys == fields)
            .map(|(cmd, _)| cmd)
        });
        match cmd {
            Some("status") => reads_as::<Status>(result, json),
            Some("outputs") => reads_as::<OutputList>(result, json),
            Some("toggle") => reads_as::<Toggled>(result, json),
            other => panic!("a result for {other:?}: {json}"),
        }
    }
    response
}

#[test]
fn every_example_reads_as_the_types_say() {
    let mut commands = BTreeSet::new();
    let mut codes = BTreeSet::new();
    let mut examples = 0;

    // Each `echo '<request>' | socat ...` waits for its response: a
    // `# <response>` line, or a `json` block. They come in the order of the
    // requests. A `json` block with no request waiting is a response on its
    // own, or a request if it has a `cmd`.
    let mut waiting: VecDeque<Option<Request>> = VecDeque::new();
    let mut answer = |json: &str, waiting: &mut VecDeque<Option<Request>>| {
        let (request, decoded) = match waiting.pop_front() {
            Some(decoded) => (true, decoded),
            None => (false, None),
        };
        let response = check_response(json, decoded.as_ref().map(Request::name));
        if let Outcome::Error { code, .. } = response.outcome {
            codes.insert(wire_name(&code));
            // A request the daemon refuses as invalid does not decode, and
            // every other one does.
            if request {
                assert_eq!(
                    code == ErrorCode::InvalidRequest,
                    decoded.is_none(),
                    "{decoded:?} answered {json}"
                );
            }
        } else if request {
            assert!(
                decoded.is_some(),
                "a request that does not decode answered {json}"
            );
        }
    };

    let mut block: Option<String> = None;
    for line in lines().iter().filter(|line| line.code) {
        if let Some(text) = &mut block {
            if line.text != "```" {
                text.push_str(line.text);
                text.push('\n');
                continue;
            }
            let json = std::mem::take(text);
            block = None;
            if waiting.is_empty() && json.contains("\"cmd\"") {
                let request = Request::decode(json.trim().as_bytes())
                    .unwrap_or_else(|e| panic!("{e}: {json}"));
                commands.insert(request.name().to_string());
            } else {
                answer(&json, &mut waiting);
            }
            examples += 1;
        } else if line.text == "```json" {
            block = Some(String::new());
        } else if let Some(rest) = line.text.strip_prefix("echo '") {
            let (request, _) = rest
                .split_once("' | socat")
                .unwrap_or_else(|| panic!("an example request: {}", line.text));
            let decoded = Request::decode(request.as_bytes()).ok();
            if let Some(decoded) = &decoded {
                commands.insert(decoded.name().to_string());
            }
            waiting.push_back(decoded);
        } else if let Some(json) = line.text.strip_prefix("# {") {
            assert!(!waiting.is_empty(), "a response without a request: {json}");
            answer(&format!("{{{json}"), &mut waiting);
            examples += 1;
        }
    }
    assert!(waiting.is_empty(), "requests without a response");

    assert!(examples > 20, "only {examples} examples found");
    let names: BTreeSet<String> = every_request()
        .iter()
        .map(|r| r.name().to_string())
        .collect();
    assert_eq!(commands, names, "an example of every command");
    for code in ["invalid_request", "not_found", "unsupported_media"] {
        assert!(codes.contains(code), "an example of {code}");
    }
}
