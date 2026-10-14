pub fn version_line() -> String {
    format!("fold {}", env!("CARGO_PKG_VERSION"))
}

#[expect(clippy::print_stdout, reason = "the CLI's output helper")]
pub fn line(text: &str) {
    println!("{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_names_the_workspace_version() {
        assert_eq!(version_line(), "fold 0.1.0");
    }
}
