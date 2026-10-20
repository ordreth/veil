//! Who is on the other end of a Unix socket. The daemon and the client both
//! talk only to processes of their own user.

use std::os::unix::net::UnixStream;

/// Effective user id of this process.
pub fn own_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// User id of the process that created the other end of `stream` (`SO_PEERCRED`).
/// The kernel records it when the connection is made, so the peer cannot forge it.
pub fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    Ok(rustix::net::sockopt::socket_peercred(stream)?.uid.as_raw())
}
