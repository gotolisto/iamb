//! The named pipe's name and the two files beside it (the 2.8 review's finding 12). Plain Rust,
//! built and tested on every system; the endpoint (`endpoint`, Windows only) uses it.
//!
//! The pipe namespace is shared by every user of the machine, and whoever creates a name
//! first owns it. With a name anyone could work out (`iamb-service-<user SID>`), another local
//! user could create it first and keep the worker from serving its endpoint. So the name ends
//! in a secret: 128 random bits from the system's generator, as 32 hex digits, drawn anew for
//! every bind:
//!
//! ```text
//! \\.\pipe\iamb-service-<user SID>-<32 hex digits>
//! ```
//!
//! The server writes the secret to `<run>/<kind>.pipe` (`service.pipe`, `sync.pipe`), in the
//! data root's `run` directory, which the endpoint restricts to the current user before it
//! writes there; clients read it from there. Only the user (and administrators) can read it,
//! so no other user can know the name in time to take it. Should a name be taken all the same,
//! the server draws another ([`BIND_ATTEMPTS`] in all). Which server owns the endpoint of a
//! data root is decided by an exclusive lock on `<run>/<kind>.pipe.lock`, held for the
//! listener's life, as on macOS: a second server of the same user gets `EndpointInUse` without
//! touching the name file.

use std::path::PathBuf;

use iamb_platform::EndpointName;

/// Hex digits in a secret.
pub(crate) const SECRET_LEN: usize = 32;

/// How many secrets `bind` tries before it reports the name as taken.
pub(crate) const BIND_ATTEMPTS: usize = 4;

/// The pipe's full name for the endpoint `stem` (`iamb-service`, `iamb-sync`), the user `sid`
/// and `secret`.
pub(crate) fn pipe_name(stem: &str, sid: &str, secret: &str) -> String {
    format!(r"\\.\pipe\{stem}-{sid}-{secret}")
}

/// 16 random bytes as a secret: lower-case hex.
pub(crate) fn secret_from(bytes: [u8; 16]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(SECRET_LEN);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The secret in the text of a name file: exactly [`SECRET_LEN`] lower-case hex digits, a line
/// break after them allowed. Anything else names no pipe (a file half written by something
/// else, or not by Iamb).
pub(crate) fn parse_secret(text: &str) -> Option<&str> {
    let secret = text.strip_suffix('\n').unwrap_or(text);
    let secret = secret.strip_suffix('\r').unwrap_or(secret);
    (secret.len() == SECRET_LEN
        && secret
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then_some(secret)
}

/// `<run>/<kind>.pipe`: the secret of the pipe being served.
pub(crate) fn name_file(name: &EndpointName) -> PathBuf {
    name.run_dir()
        .join(format!("{}.pipe", name.kind().as_str()))
}

/// `<run>/<kind>.pipe.lock`: held by the server that owns the endpoint.
pub(crate) fn lock_file(name: &EndpointName) -> PathBuf {
    name.run_dir()
        .join(format!("{}.pipe.lock", name.kind().as_str()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use iamb_platform::EndpointKind;

    use super::*;

    #[test]
    fn the_name_ends_in_the_secret_after_the_user() {
        let secret = secret_from([0xAB; 16]);
        assert_eq!(secret, "ab".repeat(16));
        assert_eq!(
            pipe_name("iamb-service", "S-1-5-21-1", &secret),
            format!(r"\\.\pipe\iamb-service-S-1-5-21-1-{secret}")
        );
        assert_ne!(secret_from([1; 16]), secret_from([2; 16]));
        assert_eq!(secret_from([0; 16]).len(), SECRET_LEN);
    }

    #[test]
    fn only_a_whole_secret_is_read_from_the_name_file() {
        let secret = secret_from([0x5c; 16]);
        assert_eq!(parse_secret(&secret), Some(secret.as_str()));
        assert_eq!(parse_secret(&format!("{secret}\n")), Some(secret.as_str()));
        assert_eq!(
            parse_secret(&format!("{secret}\r\n")),
            Some(secret.as_str())
        );
        for bad in [
            String::new(),
            secret[..31].to_owned(),
            format!("{secret}0"),
            secret.to_uppercase(),
            format!(r"{}\..\x", &secret[..28]),
            format!("{}zz", &secret[..30]),
            format!(" {}", &secret[..31]),
        ] {
            assert_eq!(parse_secret(&bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_files_are_in_the_run_directory_per_endpoint() {
        let service = EndpointName::new(EndpointKind::Service, "/data/run");
        let sync = EndpointName::new(EndpointKind::Sync, "/data/run");
        assert_eq!(name_file(&service), Path::new("/data/run/service.pipe"));
        assert_eq!(
            lock_file(&service),
            Path::new("/data/run/service.pipe.lock")
        );
        assert_eq!(name_file(&sync), Path::new("/data/run/sync.pipe"));
        assert_eq!(lock_file(&sync), Path::new("/data/run/sync.pipe.lock"));
    }
}
