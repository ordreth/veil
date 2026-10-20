//! The control socket end to end: framing, limits, timeouts, permissions and
//! concurrent clients, against a real server (see `common`).

mod common;

use common::{code_of, read_response, read_to_close, set, TestServer, OUTPUT, PATIENCE, PING};
use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};
use veil::ipc::protocol::{ErrorCode, Outcome, Request, Response};
use veil::ipc::report::{OutputList, Status, Toggled};
use veil::ipc::server::{self, ServerConfig};
use veil::ipc::{client, peer};

/// Read timeout of the servers that are expected to time a client out. Long
/// enough that an answered request never comes close to it.
const SHORT_TIMEOUT: Duration = Duration::from_millis(600);

#[test]
fn socket_is_private_to_its_owner() {
    let server = TestServer::start("mode", ServerConfig::default());
    let metadata = std::fs::metadata(&server.socket).expect("socket metadata");
    assert!(metadata.file_type().is_socket());
    // `srw-------`
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
    server.assert_alive();
}

#[test]
fn ping_is_answered_with_the_pinned_wire_format() {
    let server = TestServer::start("ping", ServerConfig::default());
    let mut stream = server.connect();
    stream.write_all(PING).expect("write");
    assert_eq!(
        read_to_close(&stream),
        b"{\"version\":1,\"status\":\"ok\"}\n"
    );
    assert_eq!(server.requests(), [Request::Ping {}]);
}

#[test]
fn the_client_and_the_server_understand_each_other() {
    let server = TestServer::start("client", ServerConfig::default());

    let response = client::send(&server.socket, &Request::Ping {}).expect("ping");
    assert_eq!(response, Response::ok());

    let request = Request::Set {
        output: Some(OUTPUT.into()),
        input: "/srv/walls/a.png".into(),
        mode: Some(veil::scaling::ScaleMode::Fit),
        bg_color: None,
        persist: false,
    };
    let response = client::send(&server.socket, &request).expect("set");
    assert_eq!(response, Response::ok());

    // An error of the daemon reaches the client as it was sent.
    let response = client::send(&server.socket, &set("/srv/walls/broken.png")).expect("set");
    assert_eq!(
        response,
        Response::error(ErrorCode::Failed, "cannot decode")
    );

    assert_eq!(
        server.requests(),
        [Request::Ping {}, request, set("/srv/walls/broken.png")]
    );
}

#[test]
fn a_request_may_end_without_newline_or_arrive_in_pieces() {
    let server = TestServer::start("framing", ServerConfig::default());

    // No newline: the request ends when the client stops sending.
    let mut stream = server.connect();
    stream.write_all(b"{\"cmd\":\"ping\"}").expect("write");
    stream.shutdown(Shutdown::Write).expect("shutdown");
    assert_eq!(read_response(&stream), Response::ok());

    // In pieces, with pauses that let the server run dry in between.
    let mut stream = server.connect();
    for piece in [&b"{\"cm"[..], b"d\":\"pi", b"ng\"}", b"\n"] {
        stream.write_all(piece).expect("write");
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(read_response(&stream), Response::ok());

    // One request per connection: what follows the first line is ignored.
    let response = server.exchange(b"{\"cmd\":\"ping\"}\n{\"cmd\":\"set\",\"input\":\"/a.png\"}\n");
    assert_eq!(response, Response::ok());

    assert_eq!(server.requests(), [const { Request::Ping {} }; 3]);
}

#[test]
fn invalid_requests_get_an_error_and_never_reach_the_daemon() {
    let server = TestServer::start("invalid", ServerConfig::default());

    for request in [
        &b"not json\n"[..],
        b"\n",
        b"{}\n",
        b"[]\n",
        b"{\"cmd\":\"explode\"}\n",
        b"{\"cmd\":\"PING\"}\n",
        b"{\"cmd\":\"ping\",\"extra\":1}\n",
        b"{\"cmd\":\"set\"}\n",
        b"{\"cmd\":\"set\",\"input\":42}\n",
        b"{\"cmd\":\"set\",\"input\":\"\"}\n",
        b"{\"cmd\":\"set\",\"input\":\"relative.png\"}\n",
        b"{\"cmd\":\"set\",\"input\":\"/a.png\",\"output\":\"\"}\n",
        b"{\"cmd\":\"set\",\"input\":\"/a.png\",\"mode\":\"zoom\"}\n",
        b"{\"cmd\":\"set\",\"input\":\"/a.png\",\"colour\":\"#000000\"}\n",
        b"{\"cmd\":\"status\",\"output\":\"DP-3\"}\n",
        b"{\"cmd\":\"pause\",\"output\":\"\"}\n",
        b"{\"cmd\":\"clear\",\"input\":\"/a.png\"}\n",
        b"{\"cmd\":\"toggle\",\"output\":7}\n",
        b"{\"cmd\":\"quit\",\"force\":true}\n",
        b"{\"cmd\":\"ping\"\n",
        b"{\"cmd\":\"ping\"}\xff\n",
        b"\0\0\0\0\n",
    ] {
        let response = server.exchange(request);
        assert_eq!(
            code_of(&response),
            ErrorCode::InvalidRequest,
            "{:?}",
            String::from_utf8_lossy(request)
        );
    }

    assert_eq!(server.requests(), []);
    server.assert_alive();
}

/// One row of the conformance table: a request as it goes over the wire, and
/// what it is answered with.
struct Row {
    wire: &'static str,
    /// `None`: `ok`.
    code: Option<ErrorCode>,
}

const fn ok(wire: &'static str) -> Row {
    Row { wire, code: None }
}

const fn refused(wire: &'static str, code: ErrorCode) -> Row {
    Row {
        wire,
        code: Some(code),
    }
}

/// Every command, and every error code the daemon sends, against the real
/// server and the stand-in daemon (`common`). How the real daemon decides
/// (which output, which file) is tested next to its code; this pins what goes
/// over the wire for each of them. Codes the transport sends itself
/// (`too_large`, `timeout`, `forbidden`, `busy`) and `superseded` have tests
/// of their own below.
#[test]
fn every_command_and_every_error_code() {
    let server = TestServer::start("table", ServerConfig::default());
    let table = [
        ok(r#"{"cmd":"ping"}"#),
        ok(r#"{"cmd":"set","input":"/srv/a.png"}"#),
        ok(r#"{"cmd":"set","output":"DP-3","input":"/srv/a.png","mode":"fit","persist":false}"#),
        refused(
            r#"{"cmd":"set","output":"HDMI-A-1","input":"/srv/a.png"}"#,
            ErrorCode::UnknownOutput,
        ),
        refused(
            r#"{"cmd":"set","input":"/srv/broken.png"}"#,
            ErrorCode::Failed,
        ),
        refused(
            r#"{"cmd":"set","input":"/srv/missing.png"}"#,
            ErrorCode::NotFound,
        ),
        refused(
            r#"{"cmd":"set","input":"/srv/notes.txt"}"#,
            ErrorCode::UnsupportedMedia,
        ),
        ok(r#"{"cmd":"clear"}"#),
        ok(r##"{"cmd":"clear","output":"DP-3","bg_color":"#102030","persist":false}"##),
        refused(
            r#"{"cmd":"clear","output":"HDMI-A-1"}"#,
            ErrorCode::UnknownOutput,
        ),
        ok(r#"{"cmd":"pause"}"#),
        ok(r#"{"cmd":"pause","output":"DP-3"}"#),
        refused(
            r#"{"cmd":"pause","output":"HDMI-A-1"}"#,
            ErrorCode::UnknownOutput,
        ),
        ok(r#"{"cmd":"resume"}"#),
        ok(r#"{"cmd":"resume","output":"DP-3"}"#),
        refused(
            r#"{"cmd":"resume","output":"HDMI-A-1"}"#,
            ErrorCode::UnknownOutput,
        ),
        ok(r#"{"cmd":"toggle"}"#),
        ok(r#"{"cmd":"toggle","output":"DP-3"}"#),
        refused(
            r#"{"cmd":"toggle","output":"HDMI-A-1"}"#,
            ErrorCode::UnknownOutput,
        ),
        ok(r#"{"cmd":"status"}"#),
        ok(r#"{"cmd":"outputs"}"#),
        ok(r#"{"cmd":"quit"}"#),
        refused(
            r#"{"cmd":"status","output":"DP-3"}"#,
            ErrorCode::InvalidRequest,
        ),
        refused(r#"{"cmd":"reload"}"#, ErrorCode::InvalidRequest),
    ];
    for row in &table {
        let response = server.exchange(format!("{}\n", row.wire).as_bytes());
        match row.code {
            None => assert_eq!(response.outcome, Outcome::Ok, "{}", row.wire),
            Some(code) => assert_eq!(code_of(&response), code, "{}", row.wire),
        }
    }
    // Everything valid reached the daemon, as it was sent.
    let valid: Vec<Request> = table
        .iter()
        .filter(|row| row.code != Some(ErrorCode::InvalidRequest))
        .map(|row| Request::decode(row.wire.as_bytes()).expect("a valid row"))
        .collect();
    assert_eq!(server.requests(), valid);
}

#[test]
fn reports_reach_the_client_as_their_types() {
    let server = TestServer::start("reports", ServerConfig::default());
    let ask = |request: Request| {
        let response = client::send(&server.socket, &request).expect("answered");
        client::answered(&request, response).expect("ok")
    };

    let status: Status = client::answer_of(ask(Request::Status {})).expect("a status");
    assert_eq!(status, common::status());
    let outputs: OutputList = client::answer_of(ask(Request::Outputs {})).expect("outputs");
    assert_eq!(outputs, common::outputs());
    let toggled: Toggled =
        client::answer_of(ask(Request::Toggle { output: None })).expect("a toggle");
    assert_eq!(toggled, Toggled { paused: true });
    // Nothing to report.
    assert_eq!(ask(Request::Pause { output: None }), None);
    assert_eq!(ask(Request::Ping {}), None);

    // The JSON of a report, as the server sends it.
    let mut stream = server.connect();
    stream.write_all(b"{\"cmd\":\"toggle\"}\n").expect("write");
    assert_eq!(
        read_to_close(&stream),
        b"{\"version\":1,\"status\":\"ok\",\"result\":{\"paused\":true}}\n"
    );
}

#[test]
fn clients_that_leave_early_do_no_harm() {
    let server = TestServer::start("early", ServerConfig::default());

    // Connects and goes away without a word.
    drop(server.connect());

    // Sends half a request and goes away.
    let mut stream = server.connect();
    stream.write_all(b"{\"cmd\":").expect("write");
    drop(stream);

    // Sends a request and does not wait for the response.
    let mut stream = server.connect();
    stream.write_all(PING).expect("write");
    drop(stream);

    server.assert_alive();
    // Half a request is not a request. The complete one may or may not have
    // been read before its client was gone; nothing else was handed over.
    let requests = server.requests();
    assert!(
        (1..=2).contains(&requests.len()) && requests.iter().all(|r| *r == Request::Ping {}),
        "{requests:?}"
    );
}

#[test]
fn a_silent_client_is_dropped_after_the_timeout_without_holding_up_others() {
    let config = ServerConfig {
        read_timeout: SHORT_TIMEOUT,
        ..ServerConfig::default()
    };
    let server = TestServer::start("silent", config);

    let connected = Instant::now();
    let silent = server.connect();
    let mut partial = server.connect();
    partial.write_all(b"{\"cmd\":\"pi").expect("write");

    // While both are connected and owe their request, others are served.
    for _ in 0..5 {
        server.assert_alive();
    }
    assert!(
        connected.elapsed() < SHORT_TIMEOUT,
        "the machine is too slow for this test's timing"
    );
    silent.set_nonblocking(true).expect("non-blocking");
    let error = (&silent).read(&mut [0u8; 16]).expect_err("still open");
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    silent.set_nonblocking(false).expect("blocking");

    // Then the server gives up on each of them, on its own.
    for stream in [&silent, &partial] {
        assert_eq!(code_of(&read_response(stream)), ErrorCode::Timeout);
    }
    assert!(connected.elapsed() >= SHORT_TIMEOUT);

    assert_eq!(server.requests(), [const { Request::Ping {} }; 5]);
    server.assert_alive();
}

#[test]
fn sending_slowly_does_not_extend_the_timeout() {
    let config = ServerConfig {
        read_timeout: SHORT_TIMEOUT,
        ..ServerConfig::default()
    };
    let server = TestServer::start("trickle", config);

    // One byte every 50 ms never completes this request in time, and never
    // lets the connection sit idle either.
    let mut stream = server.connect();
    let started = Instant::now();
    for byte in b"{\"cmd\":\"ping\"}                                        \n" {
        if stream.write_all(&[*byte]).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(code_of(&read_response(&stream)), ErrorCode::Timeout);
    assert!(started.elapsed() >= SHORT_TIMEOUT);

    assert_eq!(server.requests(), []);
    server.assert_alive();
}

#[test]
fn an_oversized_request_is_rejected() {
    let server = TestServer::start("oversize", ServerConfig::default());

    // 10 MiB with no end in sight. The server stops reading long before that,
    // so the writes start to fail at some point.
    let mut stream = server.connect();
    let block = [b'x'; 64 * 1024];
    let mut sent = 0usize;
    while sent < 10 * 1024 * 1024 {
        match stream.write(&block) {
            Ok(len) => sent += len,
            Err(_) => break,
        }
    }
    assert!(
        sent < 10 * 1024 * 1024,
        "the server took all {sent} bytes instead of closing"
    );
    assert_eq!(code_of(&read_response(&stream)), ErrorCode::TooLarge);

    // The same as valid JSON: a `set` whose path alone is over the limit.
    let request = serde_json::to_vec(&set(&format!("/{}", "a".repeat(server::MAX_REQUEST_LEN))))
        .expect("serialize");
    let mut stream = server.connect();
    let _ = stream.write_all(&request);
    let _ = stream.write_all(b"\n");
    assert_eq!(code_of(&read_response(&stream)), ErrorCode::TooLarge);

    assert_eq!(server.requests(), []);
    server.assert_alive();
}

#[test]
fn the_size_limit_is_exact() {
    let config = ServerConfig {
        max_request_len: 256,
        ..ServerConfig::default()
    };
    let server = TestServer::start("limit", config);

    // JSON may be padded with spaces: a request of exactly the limit is fine.
    let mut request = b"{\"cmd\":\"ping\"}".to_vec();
    request.resize(256, b' ');
    request.push(b'\n');
    assert_eq!(server.exchange(&request), Response::ok());

    // One byte more is not.
    request.insert(0, b' ');
    assert_eq!(code_of(&server.exchange(&request)), ErrorCode::TooLarge);

    assert_eq!(server.requests(), [Request::Ping {}]);
}

#[test]
fn concurrent_clients_are_all_answered() {
    let server = TestServer::start("concurrent", ServerConfig::default());

    // Eight connections open at once, served in the opposite order.
    let mut streams: Vec<UnixStream> = (0..8).map(|_| server.connect()).collect();
    for stream in streams.iter_mut().rev() {
        stream.write_all(PING).expect("write");
        assert_eq!(read_response(stream), Response::ok());
    }

    // Eight clients sending at the same time.
    std::thread::scope(|scope| {
        for client in 0..8 {
            let socket = &server.socket;
            scope.spawn(move || {
                for request in 0..25 {
                    let request = set(&format!("/srv/{client}/{request}.png"));
                    let response = client::send(socket, &request).expect("set");
                    assert_eq!(response, Response::ok());
                }
            });
        }
    });

    let requests = server.requests();
    assert_eq!(requests.len(), 8 + 8 * 25);
    for client in 0..8 {
        // Each client's requests arrived complete and in its own order.
        let expected: Vec<Request> = (0..25)
            .map(|request| set(&format!("/srv/{client}/{request}.png")))
            .collect();
        let prefix = format!("/srv/{client}/");
        let seen: Vec<Request> = requests
            .iter()
            .filter(|r| matches!(r, Request::Set { input, .. } if input.starts_with(&prefix)))
            .cloned()
            .collect();
        assert_eq!(seen, expected);
    }
}

#[test]
fn connections_over_the_limit_are_turned_away() {
    let config = ServerConfig {
        max_connections: 2,
        ..ServerConfig::default()
    };
    let server = TestServer::start("busy", config);

    let first = server.connect();
    let mut second = server.connect();
    // Both places are taken (connections are accepted in the order they were
    // made), so the third is refused at once, before it sends anything.
    let third = server.connect();
    assert_eq!(code_of(&read_response(&third)), ErrorCode::Busy);

    // A place is free again as soon as its client is gone.
    drop(first);
    let deadline = Instant::now() + PATIENCE;
    loop {
        let response = server.exchange(PING);
        if response == Response::ok() {
            break;
        }
        assert_eq!(code_of(&response), ErrorCode::Busy);
        assert!(Instant::now() < deadline, "the place was never given back");
        std::thread::sleep(Duration::from_millis(10));
    }

    // The client that kept its place was not affected.
    second.write_all(PING).expect("write");
    assert_eq!(read_response(&second), Response::ok());
    assert_eq!(server.requests(), [const { Request::Ping {} }; 2]);
}

#[test]
fn a_client_of_another_user_is_rejected() {
    // Connecting as another user needs root, so the server is told to expect
    // someone else: to this server, the test itself is the other user.
    let config = ServerConfig {
        allowed_uid: peer::own_uid().wrapping_add(1),
        ..ServerConfig::default()
    };
    let server = TestServer::start("uid", config);

    // Refused on connect, before a request is read.
    let stream = server.connect();
    assert_eq!(code_of(&read_response(&stream)), ErrorCode::Forbidden);

    // The client reports the refusal, not its own failed write.
    let response = client::send(&server.socket, &Request::Ping {}).expect("a response");
    assert_eq!(code_of(&response), ErrorCode::Forbidden);

    assert_eq!(server.requests(), []);
}

#[test]
fn the_peer_of_a_connection_is_this_user() {
    let server = TestServer::start("peer", ServerConfig::default());
    let stream = server.connect();
    assert_eq!(peer::peer_uid(&stream).expect("peer"), peer::own_uid());
}

/// Sends a `set` for `input` on a new connection and leaves the answer for later.
fn ask(server: &TestServer, input: &str) -> UnixStream {
    let mut stream = server.connect();
    let request = serde_json::to_vec(&set(input)).expect("serialize");
    stream.write_all(&request).expect("write");
    stream.write_all(b"\n").expect("write");
    stream
}

/// Waits until the daemon stand-in has been handed `count` requests.
fn wait_for_requests(server: &TestServer, count: usize) {
    let deadline = Instant::now() + PATIENCE;
    while server.requests().len() < count {
        assert!(Instant::now() < deadline, "the requests never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_answer_may_come_later_and_is_not_bound_by_the_read_timeout() {
    let config = ServerConfig {
        read_timeout: SHORT_TIMEOUT,
        ..ServerConfig::default()
    };
    let server = TestServer::start("deferred", config);

    // The daemon keeps this request, as it does while a wallpaper loads.
    let waiting = ask(&server, "/srv/hold.png");

    // Others are served meanwhile, and the timeout for sending a request does
    // not apply to a client that has sent its request and waits for the answer.
    let asked = Instant::now();
    while asked.elapsed() < SHORT_TIMEOUT * 2 {
        server.assert_alive();
        std::thread::sleep(Duration::from_millis(50));
    }
    waiting.set_nonblocking(true).expect("non-blocking");
    let error = (&waiting).read(&mut [0u8; 16]).expect_err("no answer yet");
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    waiting.set_nonblocking(false).expect("blocking");

    // The daemon answers when it is ready.
    assert_eq!(
        client::send(&server.socket, &set("/srv/release.png")).expect("release"),
        Response::ok()
    );
    assert_eq!(read_response(&waiting), Response::ok());
}

#[test]
fn a_request_the_daemon_drops_is_answered_with_an_error() {
    let server = TestServer::start("forgotten", ServerConfig::default());
    let waiting = ask(&server, "/srv/hold.png");
    wait_for_requests(&server, 1);

    // The daemon lets go of the kept request without answering it. The client
    // is told so and is not left waiting.
    assert_eq!(
        client::send(&server.socket, &set("/srv/forget.png")).expect("forget"),
        Response::ok()
    );
    assert_eq!(code_of(&read_response(&waiting)), ErrorCode::Failed);
}

#[test]
fn a_client_that_leaves_before_its_answer_does_no_harm() {
    let server = TestServer::start("left-waiting", ServerConfig::default());
    let waiting = ask(&server, "/srv/hold.png");
    wait_for_requests(&server, 1);
    drop(waiting);

    // Answering into the closed connection is harmless.
    assert_eq!(
        client::send(&server.socket, &set("/srv/release.png")).expect("release"),
        Response::ok()
    );
    server.assert_alive();
}

#[test]
fn a_waiting_client_keeps_its_place_until_it_is_answered() {
    let config = ServerConfig {
        max_connections: 3,
        ..ServerConfig::default()
    };
    let server = TestServer::start("waiting-slot", config);

    // Two clients wait for their answers, and a third is connected.
    let first = ask(&server, "/srv/hold.png");
    let second = ask(&server, "/srv/hold.png");
    wait_for_requests(&server, 2);
    let third = server.connect();
    // The waiting ones still count: there is no room for a fourth.
    let fourth = server.connect();
    assert_eq!(code_of(&read_response(&fourth)), ErrorCode::Busy);

    // Once they are answered, their places are free again.
    let mut third = third;
    let request = serde_json::to_vec(&set("/srv/release.png")).expect("serialize");
    third.write_all(&request).expect("write");
    third.write_all(b"\n").expect("write");
    assert_eq!(read_response(&third), Response::ok());
    assert_eq!(read_response(&first), Response::ok());
    assert_eq!(read_response(&second), Response::ok());

    let all: Vec<UnixStream> = (0..3).map(|_| server.connect()).collect();
    for mut stream in all {
        stream.write_all(PING).expect("write");
        assert_eq!(read_response(&stream), Response::ok());
    }
}

#[test]
fn the_client_gives_up_on_a_daemon_that_does_not_answer() {
    let server = TestServer::start("client-timeout", ServerConfig::default());

    let started = Instant::now();
    let error = client::send_with_timeout(
        &server.socket,
        &set("/srv/hold.png"),
        Duration::from_millis(300),
    )
    .expect_err("the daemon keeps the request");
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(error.to_string().contains("did not answer"), "{error}");

    server.assert_alive();
}
