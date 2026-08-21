//! Reading the bootstrap server's console output.
//!
//! There is no API for the device-code flow — the server prints it and reads commands from
//! stdin, so matching on human-readable text is the only channel available. Matching is
//! therefore deliberately loose: markers are found anywhere in a line, so a log prefix or
//! timestamp does not defeat them, and an unrecognised line is passed through rather than
//! discarded.

/// Something worth acting on in a line of console output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// Where the operator should go to authorise.
    Visit(String),
    /// The code to type there.
    Code(String),
    /// A single link with the code embedded.
    DirectLink(String),
    /// Authorisation is being polled for; the server states a deadline.
    Waiting { seconds: Option<u64> },
    Authenticated,
    AuthFailed,
    Other,
}

pub fn classify(line: &str) -> Signal {
    let line = &strip_ansi(line);

    // `Or visit:` must be tested first — it contains `visit:`.
    if let Some(rest) = after(line, "or visit:") {
        return Signal::DirectLink(rest.to_string());
    }
    if let Some(rest) = after(line, "enter code:") {
        return Signal::Code(rest.to_string());
    }
    if let Some(rest) = after(line, "visit:") {
        return Signal::Visit(rest.to_string());
    }
    if contains(line, "authentication successful") {
        return Signal::Authenticated;
    }
    if contains(line, "authentication failed")
        || contains(line, "authorization failed")
        || contains(line, "device code expired")
    {
        return Signal::AuthFailed;
    }
    if let Some(rest) = after(line, "expires in") {
        let seconds = rest
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok());
        return Signal::Waiting { seconds };
    }
    Signal::Other
}

fn contains(line: &str, needle: &str) -> bool {
    line.to_ascii_lowercase().contains(needle)
}

fn after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let index = line.to_ascii_lowercase().find(marker)?;
    Some(line[index + marker.len()..].trim())
}

/// The server colours its console even when stdout is a pipe, so codes have to come off
/// before anything is matched or shown.
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            // A CSI sequence ends at its final byte, in the range @ to ~.
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

/// Divider lines the server draws around the authorisation block, which carry no
/// information of their own. Log-prefixed copies count too.
pub fn is_divider(line: &str) -> bool {
    let line = strip_ansi(line);
    let tail = line.rsplit(']').next().unwrap_or(&line).trim();
    tail.len() >= 8 && tail.chars().all(|c| c == '=')
}

/// The server announces a completed boot; waiting for this beats guessing from silence.
pub fn is_booted(line: &str) -> bool {
    contains(&strip_ansi(line), "server booted")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from the server manual.
    const BLOCK: &str = "\
===================================================================
DEVICE AUTHORIZATION
===================================================================
Visit: https://accounts.hytale.com/device
Enter code: ABCD-1234
Or visit: https://accounts.hytale.com/device?user_code=ABCD-1234
===================================================================
Waiting for authorization (expires in 900 seconds)...";

    #[test]
    fn reads_the_documented_authorisation_block() {
        let signals: Vec<Signal> = BLOCK.lines().map(classify).collect();

        assert!(signals.contains(&Signal::Visit(
            "https://accounts.hytale.com/device".to_string()
        )));
        assert!(signals.contains(&Signal::Code("ABCD-1234".to_string())));
        assert!(signals.contains(&Signal::DirectLink(
            "https://accounts.hytale.com/device?user_code=ABCD-1234".to_string()
        )));
        assert!(signals.contains(&Signal::Waiting { seconds: Some(900) }));
    }

    #[test]
    fn or_visit_is_not_mistaken_for_visit() {
        // `Or visit:` contains `visit:`, so order matters in the matcher.
        assert_eq!(
            classify("Or visit: https://example.test/x"),
            Signal::DirectLink("https://example.test/x".to_string())
        );
    }

    #[test]
    fn a_log_prefix_does_not_defeat_matching() {
        assert_eq!(
            classify("[12:04:31 INFO] [auth] Enter code: WXYZ-9876"),
            Signal::Code("WXYZ-9876".to_string())
        );
    }

    #[test]
    fn success_is_recognised_with_its_trailing_mode() {
        assert_eq!(
            classify("Authentication successful! Mode: OAUTH_DEVICE"),
            Signal::Authenticated
        );
    }

    #[test]
    fn ordinary_output_is_passed_through() {
        assert_eq!(classify("Loading plugins..."), Signal::Other);
        assert_eq!(classify(""), Signal::Other);
    }

    #[test]
    fn dividers_are_recognised() {
        assert!(is_divider("========================================"));
        assert!(!is_divider("DEVICE AUTHORIZATION"));
        assert!(!is_divider("=="));
        // As the server actually emits them, behind a log prefix.
        assert!(is_divider(
            "[2026/08/21 22:44:33   INFO]        [AbstractCommand] ==================="
        ));
    }

    /// Captured from a real bootstrap run: the server colours piped output too.
    #[test]
    fn ansi_codes_do_not_defeat_matching() {
        assert_eq!(
            strip_ansi("\x1b[m[2026/08/21 22:46:28   INFO] Loading\x1b[m"),
            "[2026/08/21 22:46:28   INFO] Loading"
        );
        assert_eq!(
            classify("\x1b[0;32mEnter code:  \x1b[1m4fcNzNAi\x1b[m"),
            Signal::Code("4fcNzNAi".to_string())
        );
    }

    #[test]
    fn a_lone_escape_is_not_swallowed() {
        assert_eq!(strip_ansi("a\x1bb"), "ab");
    }

    #[test]
    fn the_boot_marker_is_recognised() {
        assert!(is_booted(
            "[2026/08/21 22:46:29   INFO] [HytaleServer]   Hytale Server Booted! [Multiplayer]"
        ));
        assert!(!is_booted("Booting up HytaleServer - Version: 0.5.9"));
    }

    /// Captured verbatim from a live bootstrap run on 2026-08-22. Every line carries both a
    /// log prefix and a trailing ANSI reset, and the real code is mixed-case and
    /// unhyphenated rather than the manual's `ABCD-1234`.
    #[test]
    fn reads_a_live_authorisation_block() {
        let block = [
            "\x1b[m[2026/08/21 22:49:22   INFO]        [AbstractCommand] DEVICE AUTHORIZATION\x1b[m",
            "\x1b[m[2026/08/21 22:49:22   INFO]        [AbstractCommand] Visit: https://oauth.accounts.hytale.com/oauth2/device/verify\x1b[m",
            "\x1b[m[2026/08/21 22:49:22   INFO]        [AbstractCommand] Enter code: NkY3gwYf\x1b[m",
            "\x1b[m[2026/08/21 22:49:22   INFO]        [AbstractCommand] Or visit: https://oauth.accounts.hytale.com/oauth2/device/verify?user_code=NkY3gwYf\x1b[m",
            "\x1b[m[2026/08/21 22:49:22   INFO]        [AbstractCommand] Waiting for authorization (expires in 599 seconds)...\x1b[m",
        ];
        let signals: Vec<Signal> = block.iter().map(|l| classify(l)).collect();

        assert_eq!(signals[1], Signal::Visit(
            "https://oauth.accounts.hytale.com/oauth2/device/verify".to_string()
        ));
        // The trailing reset must not end up inside the code.
        assert_eq!(signals[2], Signal::Code("NkY3gwYf".to_string()));
        assert_eq!(signals[3], Signal::DirectLink(
            "https://oauth.accounts.hytale.com/oauth2/device/verify?user_code=NkY3gwYf".to_string()
        ));
        assert_eq!(signals[4], Signal::Waiting { seconds: Some(599) });
    }
}
