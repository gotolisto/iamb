//! Links in a formatted preview open in the system browser (UX specification §9 rule 8).
//!
//! The library window asks `template_open_link` for an address; [`checked`] decides whether it
//! may be opened and [`open`] hands it to the platform's default handler. Nothing else is
//! opened: the allowlist is `http`, `https` and `mailto`, the same schemes the service's
//! sanitiser lets through, and an address with a user name or password is refused (it is how an
//! address hides the host it really goes to). The window itself never navigates.
//!
//! The address is passed to the handler as one argument, never through a shell, and only after
//! it has been parsed and written back in its normal form. It is not logged: where a person
//! clicks is theirs.

use std::process::{Command, Stdio};

use url::Url;

use crate::contract::CommandError;

/// The most characters of an address that is opened.
pub const MAX_LINK: usize = 2048;

/// The schemes that may be opened.
const SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Why an address was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Empty, too long, with a control character, or not an address.
    Malformed,
    /// A scheme other than `http`, `https` or `mailto` (`file:`, `javascript:`, `data:`, a
    /// custom scheme, a relative address).
    Scheme,
    /// A user name or password in the address.
    Credentials,
    /// `http` or `https` without a host.
    NoHost,
}

impl From<Refused> for CommandError {
    fn from(r: Refused) -> Self {
        let why = match r {
            Refused::Malformed => "that is not an address Iamb can open",
            Refused::Scheme => "Iamb opens only web and email links",
            Refused::Credentials => "Iamb does not open addresses with a user name or password",
            Refused::NoHost => "that web address has no host",
        };
        Self::new("link_refused", why)
    }
}

/// The address in its normal form if it may be opened.
///
/// # Errors
/// The reason it may not: [`Refused`].
pub fn checked(address: &str) -> Result<Url, Refused> {
    if address.is_empty() || address.len() > MAX_LINK || address.chars().any(char::is_control) {
        return Err(Refused::Malformed);
    }
    // Leading or trailing space is something a parser quietly drops; here it is a refusal.
    if address.trim() != address {
        return Err(Refused::Malformed);
    }
    let url = Url::parse(address).map_err(|_| Refused::Malformed)?;
    if !SCHEMES.contains(&url.scheme()) {
        return Err(Refused::Scheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Refused::Credentials);
    }
    if url.scheme() != "mailto" && url.host_str().is_none_or(str::is_empty) {
        return Err(Refused::NoHost);
    }
    Ok(url)
}

/// The program and arguments that open `url` with the platform's default handler.
fn handler(url: &Url) -> (&'static str, Vec<String>) {
    let url = url.as_str().to_owned();
    if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(windows) {
        // Not `cmd /c start`, which reads its argument as a command line.
        (
            "rundll32",
            vec!["url.dll,FileProtocolHandler".to_owned(), url],
        )
    } else {
        ("xdg-open", vec![url])
    }
}

/// Opens `address` with the platform's default handler, if [`checked`] allows it.
///
/// # Errors
/// `link_refused` if it may not be opened, `unavailable` if the handler cannot be started.
pub fn open(address: &str) -> Result<(), CommandError> {
    let url = checked(address)?;
    let (program, args) = handler(&url);
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| CommandError::new("unavailable", "Iamb could not open the link"))?;
    // Reaped on a thread of its own: a handler may stay open as long as the browser does.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_and_mail_addresses_are_accepted_in_their_normal_form() {
        for (given, normal) in [
            ("https://example.com", "https://example.com/"),
            (
                "http://example.com/a?b=1&c=2#d",
                "http://example.com/a?b=1&c=2#d",
            ),
            ("HTTPS://Example.COM:8443/x", "https://example.com:8443/x"),
            ("mailto:kim@example.com", "mailto:kim@example.com"),
            (
                "mailto:kim@example.com?subject=Hi%20there",
                "mailto:kim@example.com?subject=Hi%20there",
            ),
            ("https://[::1]/", "https://[::1]/"),
        ] {
            assert_eq!(checked(given).expect(given).as_str(), normal, "{given}");
        }
    }

    #[test]
    fn everything_else_is_refused() {
        for (given, why) in [
            ("file:///etc/passwd", Refused::Scheme),
            ("FILE:///C:/Windows/system32/calc.exe", Refused::Scheme),
            ("javascript:alert(1)", Refused::Scheme),
            ("JaVaScRiPt:alert(1)", Refused::Scheme),
            ("data:text/html,<script>1</script>", Refused::Scheme),
            ("vbscript:msgbox(1)", Refused::Scheme),
            ("ftp://example.com/x", Refused::Scheme),
            ("tel:+15550100", Refused::Scheme),
            ("ms-msdt:/id PCWDiagnostic", Refused::Scheme),
            ("slack://open", Refused::Scheme),
            ("iamb://settings", Refused::Scheme),
            ("blob:https://example.com/1", Refused::Scheme),
            ("https://user@example.com/", Refused::Credentials),
            ("https://user:secret@example.com/", Refused::Credentials),
            ("https://:secret@example.com/", Refused::Credentials),
            ("http://example.com@evil.example/", Refused::Credentials),
            ("https://", Refused::Malformed),
            ("example.com", Refused::Malformed),
            ("/relative/path", Refused::Malformed),
            ("//example.com/x", Refused::Malformed),
            ("", Refused::Malformed),
            (" https://example.com/", Refused::Malformed),
            ("https://example.com/\n", Refused::Malformed),
            ("https://exa\u{0}mple.com/", Refused::Malformed),
            ("https://exa mple.com/", Refused::Malformed),
            ("-https://example.com", Refused::Malformed),
        ] {
            assert_eq!(checked(given).map(|u| u.to_string()), Err(why), "{given:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_LINK));
        assert_eq!(checked(&long).map(|_| ()), Err(Refused::Malformed));
    }

    #[test]
    fn a_refusal_has_a_stable_code_and_no_address_in_its_message() {
        let e: CommandError = checked("javascript:alert(1)").expect_err("refused").into();
        assert_eq!(e.code, "link_refused");
        assert!(!e.message.contains("javascript"));
    }

    #[test]
    fn the_handler_gets_the_address_as_one_argument() {
        let url = checked("https://example.com/a b;rm -rf /?x=$(id)").expect("accepted");
        let (_, args) = handler(&url);
        assert_eq!(args.last().map(String::as_str), Some(url.as_str()));
        assert!(!args.last().expect("arg").contains(' '));
    }

    #[test]
    fn an_address_with_a_scheme_only_in_the_text_is_not_opened() {
        let e = open("file:///etc/passwd").expect_err("refused");
        assert_eq!(e.code, "link_refused");
    }
}
