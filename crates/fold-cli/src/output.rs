use std::io::{self, Write};

pub fn version_line() -> String {
    format!("fold {}", env!("CARGO_PKG_VERSION"))
}

pub fn line(out: &mut impl Write, text: &str) -> io::Result<()> {
    writeln!(out, "{text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_names_the_workspace_version() {
        assert_eq!(version_line(), "fold 0.1.0");
    }

    #[test]
    fn line_writes_the_text_and_a_newline() {
        let mut out = Vec::new();
        line(&mut out, "hello").unwrap();
        assert_eq!(out, b"hello\n");
    }

    #[test]
    fn line_reports_a_write_failure() {
        let mut full: &mut [u8] = &mut [];
        let error = line(&mut full, "x").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    }
}
