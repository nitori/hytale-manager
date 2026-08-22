//! Rendering commands for whichever shell the operator is actually in.
//!
//! Keying off the platform is not enough: on Windows `hy` may be run from `cmd`,
//! PowerShell, or Git Bash, and Git Bash wants POSIX quoting and `/c/Users/...` paths. A
//! backslash path pasted into bash silently loses characters, so this is worth getting
//! right even though it only affects what we print.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// A POSIX shell on a POSIX system.
    Posix,
    /// Git Bash or MSYS2 on Windows: POSIX syntax over Windows paths.
    Msys,
    /// `cmd` or PowerShell.
    WindowsNative,
}

impl Shell {
    /// `MSYSTEM` is the marker Git Bash exports; `OSTYPE` is a bash builtin that usually
    /// is not. Cygwin sets neither and mounts drives at `/cygdrive/c`, so it is treated as
    /// Windows-native rather than guessed at.
    pub fn detect() -> Self {
        if !cfg!(windows) {
            Self::Posix
        } else if std::env::var_os("MSYSTEM").is_some() {
            Self::Msys
        } else {
            Self::WindowsNative
        }
    }

    pub fn quote(self, part: &str) -> String {
        match self {
            Self::Posix | Self::Msys => quote_posix(part),
            Self::WindowsNative => quote_windows(part),
        }
    }

    /// A path as this shell would accept it.
    pub fn path(self, path: &Path) -> String {
        let raw = path.to_string_lossy();
        match self {
            Self::Msys => to_msys_path(&raw),
            _ => raw.into_owned(),
        }
    }

    pub fn quoted_path(self, path: &Path) -> String {
        self.quote(&self.path(path))
    }
}

/// `C:\Users\you` becomes `/c/Users/you`, and `\\host\share` becomes `//host/share`.
///
/// A drive-relative path like `C:foo` — meaning "foo, relative to the current directory on
/// C:" — has no POSIX spelling, so it is left alone rather than mangled into `/cfoo`.
pub fn to_msys_path(path: &str) -> String {
    let slashed = path.replace('\\', "/");
    let bytes = slashed.as_bytes();

    let is_drive = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/');
    if !is_drive {
        return slashed;
    }

    let drive = (bytes[0] as char).to_ascii_lowercase();
    format!("/{drive}{}", &slashed[2..])
}

fn quote_posix(part: &str) -> String {
    if !part.is_empty() && !part.contains([' ', '\t', '"', '\'', '\\', '$', '`', '*', '?']) {
        return part.to_string();
    }
    format!("'{}'", part.replace('\'', r"'\''"))
}

/// Follows the backslash rules `CommandLineToArgvW` applies: a run of backslashes is
/// doubled only when it precedes a quote. `%VAR%` and `$var` are left alone — `cmd` and
/// PowerShell disagree about them, and this string is for reading, not re-parsing.
fn quote_windows(part: &str) -> String {
    if !part.is_empty() && !part.contains([' ', '\t', '"']) {
        return part.to_string();
    }

    let mut out = String::with_capacity(part.len() + 2);
    out.push('"');
    let mut backslashes = 0;
    for c in part.chars() {
        match c {
            '\\' => {
                backslashes += 1;
                out.push(c);
            }
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
                backslashes = 0;
                out.push('"');
            }
            _ => {
                backslashes = 0;
                out.push(c);
            }
        }
    }
    // Trailing backslashes would otherwise escape the closing quote.
    out.extend(std::iter::repeat_n('\\', backslashes));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every variant is exercised on every platform; only `detect` is conditional.

    #[test]
    fn posix_quoting_uses_single_quotes() {
        assert_eq!(Shell::Posix.quote("-Xmx4G"), "-Xmx4G");
        assert_eq!(
            Shell::Posix.quote("/opt/jdk 25/bin/java"),
            "'/opt/jdk 25/bin/java'"
        );
        assert_eq!(Shell::Posix.quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn windows_quoting_uses_double_quotes() {
        // Single quotes are literal to cmd.exe, so POSIX quoting would not resolve.
        assert_eq!(
            Shell::WindowsNative.quote(r"C:\Program Files\Java\bin\java.exe"),
            "\"C:\\Program Files\\Java\\bin\\java.exe\""
        );
        assert_eq!(
            Shell::WindowsNative.quote(r"C:\jdk\bin\java.exe"),
            r"C:\jdk\bin\java.exe"
        );
    }

    #[test]
    fn windows_quoting_follows_the_backslash_rules() {
        assert_eq!(
            Shell::WindowsNative.quote(r"C:\dir with space\"),
            "\"C:\\dir with space\\\\\""
        );
        assert_eq!(Shell::WindowsNative.quote(r#"a\"b"#), r#""a\\\"b""#);
    }

    #[test]
    fn msys_rewrites_drive_letters() {
        assert_eq!(to_msys_path(r"C:\Users\you\game"), "/c/Users/you/game");
        assert_eq!(to_msys_path("D:/srv"), "/d/srv");
        assert_eq!(to_msys_path("C:"), "/c");
    }

    #[test]
    fn msys_rewrites_unc_paths() {
        assert_eq!(to_msys_path(r"\\host\share\game"), "//host/share/game");
    }

    #[test]
    fn msys_leaves_posix_and_relative_paths_alone() {
        assert_eq!(to_msys_path("/c/already/posix"), "/c/already/posix");
        // Drive-relative: `foo` on C:'s current directory. `/cfoo` would be a lie.
        assert_eq!(to_msys_path("C:foo"), "C:foo");
    }

    #[test]
    fn msys_quotes_the_rewritten_path() {
        // A backslash path in bash loses `\U`, so both halves have to change together.
        assert_eq!(
            Shell::Msys.quoted_path(Path::new(r"C:\Program Files\jdk\bin\java.exe")),
            "'/c/Program Files/jdk/bin/java.exe'"
        );
    }

    #[test]
    fn only_msys_rewrites_paths() {
        let path = Path::new(r"C:\game");
        assert_eq!(Shell::WindowsNative.path(path), r"C:\game");
        assert_eq!(Shell::Msys.path(path), "/c/game");
    }
}
