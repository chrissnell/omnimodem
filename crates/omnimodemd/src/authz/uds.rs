//! UDS authorization: socket-file mode hardening + SO_PEERCRED peer-uid check.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Decide whether a peer uid is allowed. Phase 1 policy: the peer must be the
/// same uid the daemon runs as. (An allowlist arrives with multi-user setups.)
pub fn peer_uid_allowed(server_uid: u32, peer_uid: u32) -> bool {
    peer_uid == server_uid
}

/// The uid this process runs as.
pub fn current_uid() -> u32 {
    // Safe: getuid() has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Harden the socket file so only the owner can connect (mode 0600). UDS
/// connect permission is governed by the socket file's mode on Linux.
pub fn harden_socket_mode(path: &Path) -> std::io::Result<()> {
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_uid_allowed_other_denied() {
        assert!(peer_uid_allowed(1000, 1000));
        assert!(!peer_uid_allowed(1000, 1001));
        assert!(!peer_uid_allowed(0, 1000));
    }
}
