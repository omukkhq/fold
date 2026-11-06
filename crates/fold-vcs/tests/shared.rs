#![cfg(test)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use fold_vcs::ignore::IgnoreRules;
use fold_vcs::paths::RepoPath;
use fold_vcs::shared::{
    NotShareable, check_shareable, link_shared, parse_shared, read_shared, unshared_ignored,
    write_shared,
};

fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

fn path(text: &str) -> RepoPath {
    RepoPath::parse(text.as_bytes()).unwrap()
}

/// A project folder with ignored design files, a secret and build output, a tracked source file, and one workspace.
fn project() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("app");
    write(&root, ".gitignore", "design/\n.env\ntarget/\n*.pen\n");
    write(&root, "design/home.pen", "pen");
    write(&root, "design/notes.md", "notes");
    write(&root, "docs/spec.pen", "spec");
    write(&root, ".env", "KEY=1");
    write(&root, "target/out.bin", "bin");
    write(&root, "src/lib.rs", "fn main() {}");
    write(&root, ".fold/exclude", "/.fold/\n");
    let ws = root.join(".fold/ws/t-1");
    write(&ws, "src/lib.rs", "fn main() {}");
    write(&ws, ".gitignore", "design/\n.env\ntarget/\n*.pen\n");
    (dir, root, ws)
}

#[test]
fn the_list_keeps_valid_paths_in_order_and_skips_the_rest() {
    let text = "# note\n\ndocs/spec.pen\ndesign/\n../escape\n.fold/x\n/abs\ndesign\n";
    assert_eq!(parse_shared(text), [path("design"), path("docs/spec.pen")]);
    let dir = tempfile::tempdir().unwrap();
    assert!(read_shared(dir.path()).unwrap().is_empty());
    write_shared(dir.path(), &[path("design"), path("docs/spec.pen")]).unwrap();
    let written = fs::read_to_string(dir.path().join("shared")).unwrap();
    assert!(written.starts_with("# fold writes this file"));
    assert_eq!(
        read_shared(dir.path()).unwrap(),
        [path("design"), path("docs/spec.pen")]
    );
}

#[test]
fn only_an_ignored_path_reached_without_a_link_and_apart_from_the_others_is_shareable() {
    let (_dir, root, _ws) = project();
    let rules = IgnoreRules::new(&root);
    let design = check_shareable(&root, &rules, &[], path("design")).unwrap();
    // Only the folder pattern `design/` leaves it out, so its link would be recorded without an exclude line.
    assert!(design.needs_exclude);
    let spec = check_shareable(&root, &rules, &[], path("docs/spec.pen")).unwrap();
    assert!(!spec.needs_exclude);
    assert_eq!(
        check_shareable(&root, &rules, &[], path("src/lib.rs")),
        Err(NotShareable::NotIgnored)
    );
    assert_eq!(
        check_shareable(&root, &rules, &[], path("missing.pen")),
        Err(NotShareable::Missing)
    );
    symlink(root.join("design"), root.join("look")).unwrap();
    write(&root, ".gitignore", "design/\n.env\ntarget/\n*.pen\nlook\n");
    assert_eq!(
        check_shareable(&root, &rules, &[], path("look")),
        Err(NotShareable::Link)
    );
    assert_eq!(
        check_shareable(&root, &rules, &[], path("look/home.pen")),
        Err(NotShareable::Link)
    );
    let shared = [path("design")];
    assert_eq!(
        check_shareable(&root, &rules, &shared, path("design/home.pen")),
        Err(NotShareable::Overlaps)
    );
}

#[test]
fn a_workspace_links_each_shared_path_and_drops_the_link_once_unshared() {
    let (_dir, root, ws) = project();
    write(&root, ".fold/exclude", "/.fold/\n/design\n");
    let rules = IgnoreRules::new(&root);
    let shared = [path("design"), path("docs/spec.pen")];
    let linked = link_shared(&root, &rules, &ws, &shared).unwrap();
    assert_eq!((linked.linked, linked.skipped), (2, 0));
    assert_eq!(
        fs::read_link(ws.join("design")).unwrap(),
        root.join("design")
    );
    assert_eq!(
        fs::read_to_string(ws.join("design/home.pen")).unwrap(),
        "pen"
    );
    assert_eq!(
        fs::read_to_string(ws.join("docs/spec.pen")).unwrap(),
        "spec"
    );
    // The main agent's edit in the project folder is what the workspace reads at once.
    write(&root, "design/home.pen", "pen v2");
    assert_eq!(
        fs::read_to_string(ws.join("design/home.pen")).unwrap(),
        "pen v2"
    );
    // A second create or sync keeps the links as they are.
    assert_eq!(link_shared(&root, &rules, &ws, &shared).unwrap().linked, 2);

    let linked = link_shared(&root, &rules, &ws, &[path("docs/spec.pen")]).unwrap();
    assert_eq!(linked.linked, 1);
    assert!(fs::symlink_metadata(ws.join("design")).is_err());
    assert!(fs::symlink_metadata(ws.join("docs/spec.pen")).is_ok());
    assert!(root.join("design/home.pen").is_file());

    // A link the worker removed itself never stops a later sync from linking a new path.
    let linked = link_shared(&root, &rules, &ws, &shared).unwrap();
    assert_eq!(linked.linked, 2);
    fs::remove_file(ws.join("design")).unwrap();
    let linked = link_shared(&root, &rules, &ws, &[path("docs/spec.pen")]).unwrap();
    assert_eq!(linked.linked, 1);
    write(&root, "notes.pen", "notes");
    let linked = link_shared(
        &root,
        &rules,
        &ws,
        &[path("docs/spec.pen"), path("notes.pen")],
    )
    .unwrap();
    assert_eq!(linked.linked, 2);
    assert!(fs::symlink_metadata(ws.join("notes.pen")).is_ok());
    let made = fs::read_to_string(ws.join(".fold/shared-links")).unwrap();
    assert_eq!(made, "docs/spec.pen\nnotes.pen\n");
}

#[test]
fn a_folder_that_only_a_folder_pattern_leaves_out_gets_no_link_without_its_exclude_line() {
    let (_dir, root, ws) = project();
    let rules = IgnoreRules::new(&root);
    let linked = link_shared(&root, &rules, &ws, &[path("design")]).unwrap();
    assert_eq!((linked.linked, linked.skipped), (0, 0));
    assert!(fs::symlink_metadata(ws.join("design")).is_err());
}

#[test]
fn a_workspace_keeps_its_own_entry_and_never_links_through_a_link() {
    let (_dir, root, ws) = project();
    write(&root, ".fold/exclude", "/.fold/\n/design\n");
    let rules = IgnoreRules::new(&root);
    write(&ws, "design/mine.txt", "mine");
    symlink(root.join("src"), ws.join("docs")).unwrap();
    let linked = link_shared(&root, &rules, &ws, &[path("design"), path("docs/spec.pen")]).unwrap();
    assert_eq!((linked.linked, linked.skipped), (0, 2));
    assert_eq!(
        fs::read_to_string(ws.join("design/mine.txt")).unwrap(),
        "mine"
    );
    assert!(fs::symlink_metadata(root.join("src/spec.pen")).is_err());
    // A path that is no longer ignored is tracked, so the workspace holds its file and gets no link.
    write(&root, ".gitignore", "target/\n");
    write(&root, ".fold/exclude", "/.fold/\n");
    let linked = link_shared(&root, &rules, &ws, &[path("design")]).unwrap();
    assert_eq!((linked.linked, linked.skipped), (0, 0));
}

#[test]
fn the_unseen_paths_name_each_ignored_entry_once_apart_from_the_shared_and_folds_own() {
    let (_dir, root, _ws) = project();
    write(&root, ".claude/skills/fold-main/SKILL.md", "skill");
    write(&root, ".claude/settings.local.json", "{}");
    write(
        &root,
        ".fold/exclude",
        "/.fold/\n/.claude/skills/fold-main/\n/.claude/settings.local.json\n",
    );
    let rules = IgnoreRules::new(&root);
    let (unseen, count) = unshared_ignored(&root, &rules, &[], 10).unwrap();
    assert_eq!(unseen, [".env", "design/", "docs/spec.pen", "target/"]);
    assert_eq!(count, 4);
    let (unseen, count) = unshared_ignored(&root, &rules, &[path("design")], 2).unwrap();
    assert_eq!(unseen, [".env", "docs/spec.pen"]);
    assert_eq!(count, 3);
}
