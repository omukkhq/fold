#![cfg(test)]

use std::fs;
use std::path::Path;

use fold_vcs::ignore::IgnoreRules;
use fold_vcs::import::{ImportOptions, import_dir};
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::platform::Capabilities;

fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

fn ignored(rules: &IgnoreRules, path: &str, is_dir: bool) -> bool {
    rules.reload();
    rules.is_ignored(&RepoPath::parse(path.as_bytes()).unwrap(), is_dir)
}

#[test]
fn the_exclude_file_ignores_least_and_a_bang_in_an_ignore_file_brings_a_path_back() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let rules = IgnoreRules::new(root);
    assert!(!ignored(&rules, "notes.secret", false));
    write(root, ".fold/exclude", "/.fold/\n*.secret\nlocal/\n");
    assert!(ignored(&rules, "notes.secret", false));
    assert!(ignored(&rules, "src/deep/key.secret", false));
    assert!(ignored(&rules, "local", true));
    assert!(ignored(&rules, "local/x.txt", false));
    write(root, ".gitignore", "!keep.secret\n");
    write(root, "src/.foldignore", "!*.secret\n");
    assert!(!ignored(&rules, "keep.secret", false));
    assert!(!ignored(&rules, "src/deep/key.secret", false));
    assert!(ignored(&rules, "other.secret", false));
    write(root, ".foldignore", "!local/\n");
    assert!(!ignored(&rules, "local/x.txt", false));
    // A change takes effect at the next reload, which every scan makes first.
    write(root, ".fold/exclude", "/.fold/\n");
    assert!(!ignored(&rules, "other.secret", false));
}

#[test]
fn a_workspace_reads_the_exclude_file_of_the_project_that_holds_it() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let workspace = project.join(".fold/ws/t-1");
    fs::create_dir_all(&workspace).unwrap();
    write(project, ".fold/exclude", "scratch/\n");
    let rules = IgnoreRules::new(&workspace);
    assert!(ignored(&rules, "scratch/out.txt", false));
    assert!(!ignored(&rules, "src/lib.rs", false));
    assert!(!ignored(&IgnoreRules::none(), "scratch/out.txt", false));

    // A project folder below a folder named .fold reads its own exclude file.
    let nested = project.join(".fold/other");
    write(
        &nested,
        ".fold/exclude",
        "mine/
",
    );
    let rules = IgnoreRules::new(&nested);
    assert!(ignored(&rules, "mine/a.txt", false));
    assert!(!ignored(&rules, "scratch/out.txt", false));
}

#[test]
fn the_check_directory_reads_the_exclude_file_of_its_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let checks = project.join(".fold/checks/main");
    fs::create_dir_all(&checks).unwrap();
    write(project, ".fold/exclude", "node_modules/\n");
    let rules = IgnoreRules::new(&checks);
    assert!(ignored(&rules, "node_modules/x/index.js", false));
    assert!(!ignored(&rules, "src/lib.rs", false));
}

#[test]
fn an_import_of_the_project_folder_leaves_excluded_paths_out() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(&root, "README.md", "# demo\n");
    write(&root, "local/notes.md", "mine\n");
    write(&root, ".fold/exclude", "local/\n");
    let fold_dir = dir.path().join("objects-home");
    fs::create_dir_all(&fold_dir).unwrap();
    let objects = ObjectStore::open(&fold_dir).unwrap();
    let opts = ImportOptions {
        max_file_bytes: 1 << 20,
        caps: Capabilities::probe(&fold_dir).unwrap(),
    };
    let imported = import_dir(&objects, &root, &IgnoreRules::new(&root), None, &opts).unwrap();
    assert_eq!(imported.files, 1);
}

#[test]
fn only_a_rule_line_is_an_ignore_pattern() {
    use fold_vcs::ignore::is_ignore_pattern;
    for good in ["*.log", "build/", "/local", "!keep.log", "a/**/b"] {
        assert!(is_ignore_pattern(good), "{good}");
    }
    for bad in ["", "   ", "# comment", "a\nb", "a\tb", "[z-a]"] {
        assert!(!is_ignore_pattern(bad), "{bad:?}");
    }
}

#[test]
fn a_bang_line_that_matches_fold_brings_its_data_folder_back() {
    use fold_vcs::ignore::brings_back_fold_dir;
    for back in ["!.fold", "!/.fold/", "!*", "!.f*"] {
        assert!(brings_back_fold_dir(back), "{back}");
    }
    for kept in ["!keep.log", "!.fold/ws", ".fold", "*.log", "!src/.fold"] {
        assert!(!brings_back_fold_dir(kept), "{kept}");
    }
}

#[test]
fn the_claude_lines_leave_out_folds_skills_and_the_local_settings_and_nothing_else_of_claude() {
    use fold_vcs::git_remote::exclude_claude_files;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, ".fold/exclude", "/.fold/\nnotes/\n");
    let added = exclude_claude_files(&root.join(".fold")).unwrap();
    assert_eq!(added, fold_vcs::ignore::claude_excludes());
    let rules = IgnoreRules::new(root);
    for path in [
        ".claude/skills/fold/SKILL.md",
        ".claude/skills/fold/REFERENCE.md",
        ".claude/skills/fold-main/SKILL.md",
        ".claude/skills/fold-task-worker/SKILL.md",
        ".claude/settings.local.json",
        "notes/a.md",
    ] {
        assert!(ignored(&rules, path, false), "{path}");
    }
    for path in [
        ".claude/settings.json",
        ".claude/skills/deploy/SKILL.md",
        ".claude/skills/fold-custom/SKILL.md",
        ".claude/agents/reviewer.md",
        ".claude/commands/ship.md",
        "src/.claude/skills/fold/SKILL.md",
        "src/.claude/settings.local.json",
    ] {
        assert!(!ignored(&rules, path, false), "{path}");
    }
    // The note goes on top and fold's lines after the user's, which stay where they are; a second call adds nothing.
    assert!(
        exclude_claude_files(&root.join(".fold"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(root.join(".fold/exclude")).unwrap(),
        "# fold writes this file. To keep a path, add a ! line to .foldignore.\n/.fold/\nnotes/\n/.claude/skills/fold/\n/.claude/skills/fold-area-lead/\n/.claude/skills/fold-review-lead/\n/.claude/skills/fold-task-worker/\n/.claude/skills/fold-reviewer/\n/.claude/skills/fold-main/\n/.claude/settings.local.json\n"
    );
}

#[test]
fn a_missing_line_comes_back_and_a_moved_note_stays_where_the_user_put_it() {
    use fold_vcs::git_remote::exclude_claude_files;
    use fold_vcs::ignore::EXCLUDE_NOTE;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let kept: Vec<String> = fold_vcs::ignore::claude_excludes()
        .into_iter()
        .filter(|line| line != "/.claude/skills/fold-main/")
        .collect();
    let file = format!("/.fold/\nmine/\n{EXCLUDE_NOTE}\n{}\n", kept.join("\n"));
    write(root, ".fold/exclude", &file);
    assert_eq!(
        exclude_claude_files(&root.join(".fold")).unwrap(),
        ["/.claude/skills/fold-main/"]
    );
    assert_eq!(
        fs::read_to_string(root.join(".fold/exclude")).unwrap(),
        format!("{file}/.claude/skills/fold-main/\n")
    );
}

#[test]
fn a_bang_line_of_the_foldignore_brings_a_claude_path_back() {
    use fold_vcs::git_remote::exclude_claude_files;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".fold")).unwrap();
    exclude_claude_files(&root.join(".fold")).unwrap();
    let rules = IgnoreRules::new(root);
    assert!(ignored(&rules, ".claude/skills/fold/SKILL.md", false));
    assert!(ignored(&rules, ".claude/settings.local.json", false));
    // As in Git, a file below an excluded folder comes back only with its folder.
    write(root, ".foldignore", "!/.claude/skills/fold/SKILL.md\n");
    assert!(ignored(&rules, ".claude/skills/fold/SKILL.md", false));
    write(
        root,
        ".foldignore",
        "!/.claude/skills/fold/\n!/.claude/settings.local.json\n",
    );
    assert!(!ignored(&rules, ".claude/skills/fold/SKILL.md", false));
    assert!(!ignored(&rules, ".claude/settings.local.json", false));
    assert!(ignored(&rules, ".claude/skills/fold-main/SKILL.md", false));
}
