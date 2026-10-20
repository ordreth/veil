//! One daemon per control socket, and no socket file without a daemon.
//!
//! A daemon claims its socket path before it does anything else. The claim is
//! an advisory lock on a file next to the socket, held for as long as the
//! process lives. The kernel releases it when the process ends, however it
//! ends, so a daemon that was killed never blocks the next one, and two daemons
//! started at the same moment cannot both win.

use crate::error::VeilError;
use std::fs::{File, OpenOptions, Permissions, TryLockError};
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A daemon's claim on a control socket. Dropping it removes the socket file
/// and gives the claim up; keep it until the daemon has shut down.
#[derive(Debug)]
pub struct SocketGuard {
    path: PathBuf,
    /// Device and inode of the socket file this daemon created.
    file_id: (u64, u64),
    /// The listening socket, held open a second time. A bound socket keeps the
    /// inode of its file in use, also after the file was unlinked. Without
    /// this, the file system could give the same inode number to a file that
    /// replaced ours, and `file_id` would not tell them apart.
    _socket: UnixListener,
    /// The instance lock. It is released when the file is closed.
    _lock: File,
}

impl SocketGuard {
    /// Path of the socket file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        // Only the file this daemon created. If something else is at the path
        // by now, it is not ours to remove.
        let ours = std::fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.file_id);
        if !ours {
            return;
        }
        if let Err(e) = std::fs::remove_file(&self.path) {
            tracing::warn!(
                "could not remove the control socket {}: {e}",
                self.path.display()
            );
        }
    }
}

/// Path of the lock file that belongs to the socket at `socket`.
pub fn lock_path(socket: &Path) -> PathBuf {
    let mut name = socket.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// Claims `socket` for this process and creates the control socket there,
/// readable and writable by its owner only (`srw-------`).
///
/// Fails with [`VeilError::AlreadyRunning`] if another daemon has the claim or
/// is listening on the socket. A socket file that nobody listens on (its
/// daemon was killed) is replaced.
pub fn claim(socket: &Path) -> Result<(UnixListener, SocketGuard), VeilError> {
    let error = |action: &str, path: &Path, e: std::io::Error| {
        VeilError::Ipc(format!("failed to {action} {}: {e}", path.display()))
    };
    let already_running = || VeilError::AlreadyRunning {
        socket: socket.to_path_buf(),
    };

    // The lock file is never removed: unlinking it would let one daemon hold
    // the lock on the old file while another one locks a new file.
    let lock_file = lock_path(socket);
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_file)
        .map_err(|e| error("open", &lock_file, e))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(already_running()),
        Err(TryLockError::Error(e)) => return Err(error("lock", &lock_file, e)),
    }

    // The lock is ours, so every daemon that takes it is gone. Something may
    // still listen on the socket: a veil from before the lock existed, or a
    // daemon whose lock file was deleted. A listener accepts the connection
    // (even while it is busy), and a file nobody listens on refuses it.
    match UnixStream::connect(socket) {
        Ok(_) => return Err(already_running()),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
            tracing::info!(
                "removing {}, left behind by a daemon that did not shut down",
                socket.display()
            );
            match std::fs::remove_file(socket) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(error("remove the old socket", socket, e)),
            }
        }
        Err(e) => return Err(error("check for a daemon on", socket, e)),
    }

    let listener = UnixListener::bind(socket).map_err(|e| error("bind", socket, e))?;
    // The socket file exists now. If the rest fails, it must not stay behind.
    let prepared = (|| {
        // Between `bind` and this call the file has the mode the umask gives
        // it. A connection made in that window is still held to the peer-uid
        // check when it is accepted, and nothing is accepted before the event
        // loop runs.
        std::fs::set_permissions(socket, Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let metadata = std::fs::symlink_metadata(socket)?;
        Ok(((metadata.dev(), metadata.ino()), listener.try_clone()?))
    })();
    let (file_id, pin) = match prepared {
        Ok(prepared) => prepared,
        Err(e) => {
            let _ = std::fs::remove_file(socket);
            return Err(error("set up", socket, e));
        }
    };

    let guard = SocketGuard {
        path: socket.to_path_buf(),
        file_id,
        _socket: pin,
        _lock: lock,
    };
    Ok((listener, guard))
}

/// How often [`wait_for_release`] looks at the claim.
const RELEASE_POLL: Duration = Duration::from_millis(20);

/// Waits until no daemon holds the claim on `socket`: the daemon has ended
/// (`veil quit`). Returns whether that happened within `timeout`.
///
/// It looks by taking a shared lock on the lock file and giving it up at
/// once. A daemon that tries to claim the socket in that moment is turned away
/// as if another one ran; the window is a few system calls long.
pub fn wait_for_release(socket: &Path, timeout: Duration) -> Result<bool, VeilError> {
    let lock_file = lock_path(socket);
    let deadline = Instant::now() + timeout;
    loop {
        let released = match File::open(&lock_file) {
            // No daemon has ever claimed it.
            Err(e) if e.kind() == ErrorKind::NotFound => true,
            Err(e) => {
                return Err(VeilError::Ipc(format!(
                    "failed to open {}: {e}",
                    lock_file.display()
                )))
            }
            // The lock is given up when `file` is closed, at the end of this
            // statement.
            Ok(file) => match file.try_lock_shared() {
                Ok(()) => true,
                Err(TryLockError::WouldBlock) => false,
                Err(TryLockError::Error(e)) => {
                    return Err(VeilError::Ipc(format!(
                        "failed to lock {}: {e}",
                        lock_file.display()
                    )))
                }
            },
        };
        if released {
            return Ok(true);
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        std::thread::sleep(RELEASE_POLL.min(deadline - now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::FileTypeExt;

    /// A socket path of its own for one test, cleaned up when dropped.
    struct TempSocket(PathBuf);

    impl TempSocket {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("veil-inst-{}-{name}.sock", std::process::id()));
            let socket = Self(path);
            socket.clean();
            socket
        }

        fn clean(&self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(lock_path(&self.0));
        }
    }

    impl Drop for TempSocket {
        fn drop(&mut self) {
            self.clean();
        }
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o7777
    }

    #[test]
    fn lock_file_sits_next_to_the_socket() {
        assert_eq!(
            lock_path(Path::new("/run/user/1000/veil-wayland-1.sock")),
            PathBuf::from("/run/user/1000/veil-wayland-1.sock.lock")
        );
    }

    #[test]
    fn claim_creates_a_private_socket_and_lock() {
        let socket = TempSocket::new("create");
        let (_listener, guard) = claim(&socket.0).expect("claim");

        assert_eq!(guard.path(), socket.0);
        let metadata = std::fs::metadata(&socket.0).expect("socket metadata");
        assert!(metadata.file_type().is_socket());
        // `srw-------`
        assert_eq!(mode(&socket.0), 0o600);
        assert_eq!(mode(&lock_path(&socket.0)), 0o600);
    }

    #[test]
    fn a_second_claim_is_refused_while_the_first_is_held() {
        let socket = TempSocket::new("second");
        let (_listener, _guard) = claim(&socket.0).expect("first claim");

        for _ in 0..2 {
            let error = claim(&socket.0).expect_err("the socket is taken");
            assert!(
                matches!(&error, VeilError::AlreadyRunning { socket: s } if *s == socket.0),
                "{error:?}"
            );
        }
        // The refused claims left the first daemon's socket alone.
        UnixStream::connect(&socket.0).expect("the first daemon still listens");
    }

    #[test]
    fn of_daemons_started_at_the_same_moment_exactly_one_wins() {
        let socket = TempSocket::new("race");
        let start = std::sync::Barrier::new(16);

        // Each thread keeps what it got until all are done, like a daemon that
        // keeps running.
        let results: Vec<_> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        claim(&socket.0)
                    })
                })
                .collect();
            threads
                .into_iter()
                .map(|thread| thread.join().expect("claim thread"))
                .collect()
        });

        let winners = results.iter().filter(|result| result.is_ok()).count();
        assert_eq!(winners, 1);
        for result in &results {
            if let Err(error) = result {
                assert!(
                    matches!(error, VeilError::AlreadyRunning { .. }),
                    "{error:?}"
                );
            }
        }
        // The winner's socket survived the others.
        UnixStream::connect(&socket.0).expect("the winner listens");
    }

    #[test]
    fn dropping_the_guard_removes_the_socket_and_frees_the_claim() {
        let socket = TempSocket::new("drop");
        let (listener, guard) = claim(&socket.0).expect("first claim");
        drop(guard);
        assert!(!socket.0.exists(), "the socket file is gone");
        // The lock file stays; it is what the next daemon locks.
        assert!(lock_path(&socket.0).exists());
        drop(listener);

        let (_listener, _guard) = claim(&socket.0).expect("claim again");
        assert!(socket.0.exists());
    }

    #[test]
    fn waiting_for_the_release_ends_when_the_daemon_has_gone() {
        let socket = TempSocket::new("release");
        // Nothing has ever claimed it.
        assert!(wait_for_release(&socket.0, Duration::ZERO).expect("no lock file"));

        let (listener, guard) = claim(&socket.0).expect("claim");
        assert!(!wait_for_release(&socket.0, Duration::from_millis(50)).expect("held"));
        let daemon = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(listener);
            drop(guard);
        });
        assert!(wait_for_release(&socket.0, Duration::from_secs(20)).expect("released"));
        daemon.join().expect("the daemon thread");
        // The next daemon can claim it.
        drop(claim(&socket.0).expect("claim again"));
    }

    #[test]
    fn a_socket_left_by_a_killed_daemon_is_replaced() {
        let socket = TempSocket::new("stale");
        // What `kill -9` leaves: the socket file with nobody listening, and
        // the lock file with nobody holding the lock.
        drop(UnixListener::bind(&socket.0).expect("bind"));
        File::create(lock_path(&socket.0)).expect("lock file");
        assert!(socket.0.exists());
        let error = UnixStream::connect(&socket.0).expect_err("nobody listens");
        assert_eq!(error.kind(), ErrorKind::ConnectionRefused);

        let (_listener, _guard) = claim(&socket.0).expect("claim over the stale socket");
        UnixStream::connect(&socket.0).expect("the new daemon listens");
        assert_eq!(mode(&socket.0), 0o600);
    }

    #[test]
    fn a_listener_without_the_lock_still_counts_as_a_running_daemon() {
        let socket = TempSocket::new("unlocked");
        // A daemon from before the lock existed, or one whose lock file was
        // deleted: it listens, but holds no lock.
        let listener = UnixListener::bind(&socket.0).expect("bind");

        let error = claim(&socket.0).expect_err("somebody listens");
        assert!(
            matches!(error, VeilError::AlreadyRunning { .. }),
            "{error:?}"
        );
        // Its socket was not touched.
        UnixStream::connect(&socket.0).expect("still listening");
        drop(listener);
    }

    #[test]
    fn the_guard_leaves_a_socket_alone_that_is_not_its_own() {
        let socket = TempSocket::new("foreign");
        let (listener, guard) = claim(&socket.0).expect("claim");
        drop(listener);
        // Something else replaced the file in the meantime.
        std::fs::remove_file(&socket.0).expect("remove");
        let _other = UnixListener::bind(&socket.0).expect("bind another socket");

        drop(guard);
        assert!(socket.0.exists(), "the other socket was not removed");
    }

    #[test]
    fn a_path_that_cannot_be_used_is_reported() {
        let socket = std::env::temp_dir()
            .join(format!("veil-inst-{}-no-such-dir", std::process::id()))
            .join("veil.sock");
        let error = claim(&socket).expect_err("the directory does not exist");
        assert!(matches!(error, VeilError::Ipc(_)), "{error:?}");
        assert!(
            error
                .to_string()
                .contains(lock_path(&socket).to_str().expect("UTF-8")),
            "{error}"
        );
    }
}
