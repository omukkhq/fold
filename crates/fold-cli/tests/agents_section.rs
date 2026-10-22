use fold_cli::claude_files::{self, AGENTS_BEGIN, AGENTS_END, AGENTS_FILE};
use tempfile::TempDir;

const SECTION: &str = include_str!("../templates/agents_section.md");

fn marked() -> String {
    format!("{AGENTS_BEGIN}\n{SECTION}{AGENTS_END}\n")
}

#[cfg(test)]
fn read(dir: &TempDir) -> String {
    std::fs::read_to_string(dir.path().join(AGENTS_FILE)).unwrap()
}

#[test]
fn the_section_sends_missing_work_back_and_new_work_to_a_request() {
    for rule in [
        "Send work that this task still misses back with `request_changes`.",
        "Describe new work outside the task with `create_request`; the proposal can still be approved.",
        "Ask the humans with `asks_human` for a product decision",
    ] {
        assert!(SECTION.contains(rule), "{rule}");
    }
}

#[test]
fn a_new_project_agents_file_holds_only_the_marked_section() {
    let dir = TempDir::new().unwrap();
    claude_files::write(dir.path()).unwrap();
    assert_eq!(read(&dir), marked());
}

#[test]
fn an_existing_agents_file_keeps_its_bytes_and_gets_the_section_appended() {
    let dir = TempDir::new().unwrap();
    let existing = "# Project rules\n\nBe kind.\n";
    std::fs::write(dir.path().join(AGENTS_FILE), existing).unwrap();
    claude_files::write(dir.path()).unwrap();
    let text = read(&dir);
    assert!(text.starts_with(existing));
    assert!(text.ends_with(&marked()));
}

#[test]
fn a_refresh_keeps_an_edit_outside_the_markers_byte_for_byte() {
    let dir = TempDir::new().unwrap();
    claude_files::write(dir.path()).unwrap();
    let edited = format!(
        "# Mine\n\nHand-written.\n\n{}\n## Later\nno final newline",
        read(&dir)
    );
    let edited = edited.trim_end_matches('\n').to_owned();
    std::fs::write(dir.path().join(AGENTS_FILE), &edited).unwrap();
    claude_files::write(dir.path()).unwrap();
    let refreshed = read(&dir);
    assert!(refreshed.starts_with("# Mine\n\nHand-written.\n\n"));
    assert!(refreshed.ends_with("## Later\nno final newline"));
    assert_eq!(refreshed, edited);
}

#[test]
fn a_refresh_restores_a_changed_section_and_a_second_refresh_writes_identical_bytes() {
    let dir = TempDir::new().unwrap();
    claude_files::write(dir.path()).unwrap();
    let stale = read(&dir).replace("fold project", "stale text");
    std::fs::write(dir.path().join(AGENTS_FILE), format!("# Mine\n\n{stale}")).unwrap();
    claude_files::write(dir.path()).unwrap();
    let first = read(&dir);
    assert_eq!(first, format!("# Mine\n\n{}", marked()));
    claude_files::write(dir.path()).unwrap();
    assert_eq!(read(&dir), first);
}
