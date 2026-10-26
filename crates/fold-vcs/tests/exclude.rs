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
