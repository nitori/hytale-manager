//! The legacy `jvm.options` argfile.
//!
//! `start.sh` reads this and passes it to the JVM as an `@argfile`; the server never does.
//! `hy run` replaces that script, so `[java] options` is the source of truth and this is
//! read exactly once, at adoption, to carry an operator's `-Xmx8G` across. Nothing writes.

use std::path::Path;

use crate::error::Result;

/// Quoting and line continuation are unsupported: no realistic JVM argument needs them,
/// and a mangled `-Xmx` is better caught by the JVM than reinterpreted here.
pub fn parse(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(|line| match line.find('#') {
            Some(start) => &line[..start],
            None => line,
        })
        .flat_map(str::split_whitespace)
        .map(str::to_string)
        .collect()
}

pub fn read(path: &Path) -> Result<Option<Vec<String>>> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    Ok(Some(parse(&contents)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_format() {
        let options = parse("-Xms2G\n-Xmx4G\n-XX:+UseG1GC\n");
        assert_eq!(options, ["-Xms2G", "-Xmx4G", "-XX:+UseG1GC"]);
    }

    #[test]
    fn strips_comments_and_blank_lines() {
        let options = parse("# memory\n\n-Xmx4G   # cap\n   \n-XX:+UseG1GC\n");
        assert_eq!(options, ["-Xmx4G", "-XX:+UseG1GC"]);
    }

    #[test]
    fn accepts_several_arguments_on_one_line() {
        assert_eq!(parse("-Xms2G -Xmx4G"), ["-Xms2G", "-Xmx4G"]);
    }

    /// The shipped wrapper documents the format in comments.
    #[test]
    fn an_all_comment_file_yields_nothing() {
        assert!(parse("# -Xms2G\n# -Xmx4G\n").is_empty());
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(&dir.path().join("jvm.options")).unwrap(), None);
    }
}
