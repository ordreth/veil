//! The control socket when the daemon has run out of file descriptors.
//!
//! This is a test binary of its own, with a single test: it uses up the file
//! descriptors of the whole process, which would break any test running next
//! to it.

mod common;

use common::{read_response, TestServer, PATIENCE, PING};
use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};
use std::fs::File;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::Duration;
use veil::ipc::protocol::Response;
use veil::ipc::server::ServerConfig;

/// File descriptor limit for the test, so that using them all up is quick
/// wherever it runs.
const FD_LIMIT: u64 = 256;

const EMFILE: i32 = 24;

#[test]
fn a_failed_accept_pauses_the_server_instead_of_spinning() {
    let config = ServerConfig {
        accept_retry: Duration::from_millis(400),
        ..ServerConfig::default()
    };
    let server = TestServer::start("fd-limit", config);
    server.assert_alive();

    let limit = getrlimit(Resource::Nofile);
    if limit.current.is_none_or(|current| current > FD_LIMIT) {
        setrlimit(
            Resource::Nofile,
            Rlimit {
                current: Some(FD_LIMIT),
                maximum: limit.maximum,
            },
        )
        .expect("lower the file descriptor limit");
    }

    // Use up every descriptor, then give one back for the client's socket.
    let mut fillers = Vec::new();
    let exhausted = loop {
        match File::open("/dev/null") {
            Ok(file) => fillers.push(file),
            Err(e) => break e,
        }
    };
    assert_eq!(exhausted.raw_os_error(), Some(EMFILE), "{exhausted}");
    fillers.pop();

    // Connecting needs no help from the server, but accepting needs a
    // descriptor the server cannot get: the connection waits, and the
    // listening socket stays readable.
    let mut client = UnixStream::connect(&server.socket).expect("connect");
    client.set_read_timeout(Some(PATIENCE)).expect("timeout");

    let before = server.wakeups();
    std::thread::sleep(Duration::from_millis(250));
    let wakeups = server.wakeups() - before;
    assert!(
        wakeups <= 5,
        "the event loop woke up {wakeups} times while it could not accept"
    );

    // With descriptors available again, the waiting client is served at the
    // next attempt, without having to reconnect.
    drop(fillers);
    client.write_all(PING).expect("write");
    assert_eq!(read_response(&client), Response::ok());
    server.assert_alive();
}
