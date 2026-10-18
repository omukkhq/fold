use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use fold_cli::claude_files::{
    self, AGENTS_BEGIN, AGENTS_END, AGENTS_FILE, CLAUDE_FILE, ClaudeFileError, MALFORMED_REASON,
    MCP_FILE, SETTINGS_FILE, SKILL_FILE, agents_md, instructions_file, shadowing_files,
    stale_section, without_section,
};
use tempfile::TempDir;

/// What `.mcp.json` held when fold wrote its server into the project.
const OLD_MCP: &str = r#"{"mcpServers": {"fold": {"command": "fld", "args": ["mcp"], "env": {"FOLD_AGENT": "${FOLD_AGENT:-}"}}}}"#;

/// What `.claude/settings.json` held when fold wrote its permission and hooks into the project.
const OLD_SETTINGS: &str = r#"{"permissions": {"allow": ["mcp__fold"]}, "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "fld hook session-start"}]}], "Stop": [{"hooks": [{"type": "command", "command": "fld hook stop"}]}], "PreToolUse": [{"matcher": "Edit|Write|MultiEdit|NotebookEdit", "hooks": [{"type": "command", "command": "fld hook pre-tool-use"}]}]}}"#;

const STUB: &str = "## fold\n\nStub section.\n";

#[cfg(test)]
fn read(root: &Path, relative: &str) -> String {
    std::fs::read_to_string(root.join(relative)).unwrap()
}

/// The modification time of each of fold's files.
#[cfg(test)]
fn mtimes(root: &Path) -> Vec<std::time::SystemTime> {
    [AGENTS_FILE, SKILL_FILE]
        .map(|relative| {
            std::fs::metadata(root.join(relative))
                .unwrap()
                .modified()
                .unwrap()
        })
        .to_vec()
}

#[cfg(test)]
fn malformed(error: ClaudeFileError) -> fold_core::FoldError {
    match error {
        ClaudeFileError::Malformed(error) => error,
        other => panic!("expected a malformed file, got {other:?}"),
    }
}

#[test]
fn a_fresh_write_gives_the_section_and_the_skill_and_a_refresh_changes_no_byte() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    claude_files::write(root).unwrap();
    assert!(!root.join(MCP_FILE).exists());
    assert!(!root.join(SETTINGS_FILE).exists());
    let agents = read(root, AGENTS_FILE);
    assert!(
        agents.starts_with(&format!("{AGENTS_BEGIN}\n## fold\n")),
        "{agents}"
    );
    assert!(agents.ends_with(&format!("\n{AGENTS_END}\n")), "{agents}");
    let skill = read(root, SKILL_FILE);
    assert!(
        skill.starts_with("---\nname: fold\ndescription: "),
        "{skill}"
    );

    let before = mtimes(root);
    let bytes: Vec<String> = [AGENTS_FILE, SKILL_FILE]
        .map(|relative| read(root, relative))
        .to_vec();
    claude_files::write(root).unwrap();
    claude_files::write(root).unwrap();
    let after: Vec<String> = [AGENTS_FILE, SKILL_FILE]
        .map(|relative| read(root, relative))
        .to_vec();
    assert_eq!(bytes, after);
    assert_eq!(before, mtimes(root), "an unchanged file is not written");
}

/// The expected texts are literals, so they prove that the user's keys keep their order and their values.
#[test]
fn a_refresh_removes_the_old_fold_entries_and_keeps_foreign_ones_in_their_order() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    let mcp = r#"{"other": 1, "mcpServers": {"zeta": {"command": "z-mcp"}, "fold": {"command": "fold", "args": ["serve"]}, "alpha": {"command": "a-mcp", "env": {"Z": "1", "A": "2"}}}}"#;
    std::fs::write(root.join(MCP_FILE), mcp).unwrap();
    let settings = r#"{"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo bye"}]}, {"hooks": [{"type": "command", "command": "fold hook stop"}]}], "SessionStart": [{"hooks": [{"type": "command", "command": "fold hook old-start"}, {"type": "command", "command": "notify-send hi"}]}, {"matcher": "resume", "hooks": [{"type": "command", "command": "fld hook session-start"}]}], "PreToolUse": [{"matcher": "Bash", "hooks": [{"command": "guard", "type": "command"}]}, {"matcher": "Edit|Write|MultiEdit|NotebookEdit", "hooks": [{"type": "command", "command": "fld hook pre-tool-use"}]}], "Notification": []}, "permissions": {"deny": ["Read(.env)", "Edit(/main/**)", "Write(/main/**)"], "allow": ["Bash(ls)", "mcp__fold"]}, "env": {"B": "2", "A": "1"}}"#;
    std::fs::write(root.join(SETTINGS_FILE), settings).unwrap();
    assert_eq!(claude_files::old_entries(root), [MCP_FILE, SETTINGS_FILE]);
    claude_files::write(root).unwrap();

    let mcp_expected = r#"{
  "other": 1,
  "mcpServers": {
    "zeta": {
      "command": "z-mcp"
    },
    "alpha": {
      "command": "a-mcp",
      "env": {
        "Z": "1",
        "A": "2"
      }
    }
  }
}
"#;
    assert_eq!(read(root, MCP_FILE), mcp_expected);
    let settings_expected = r#"{
  "model": "opus",
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "echo bye"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "notify-send hi"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "command": "guard",
            "type": "command"
          }
        ]
      }
    ],
    "Notification": []
  },
  "permissions": {
    "deny": [
      "Read(.env)"
    ],
    "allow": [
      "Bash(ls)"
    ]
  },
  "env": {
    "B": "2",
    "A": "1"
  }
}
"#;
    assert_eq!(read(root, SETTINGS_FILE), settings_expected);
    assert!(claude_files::old_entries(root).is_empty());
    let before = std::fs::metadata(root.join(MCP_FILE))
        .unwrap()
        .modified()
        .unwrap();
    claude_files::write(root).unwrap();
    assert_eq!(read(root, MCP_FILE), mcp_expected);
    assert_eq!(read(root, SETTINGS_FILE), settings_expected);
    let after = std::fs::metadata(root.join(MCP_FILE))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(before, after, "a file without fold entries is not written");
}

#[test]
fn a_project_file_that_held_only_fold_entries_goes_and_a_link_stays() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    std::fs::write(root.join(MCP_FILE), OLD_MCP).unwrap();
    std::fs::write(root.join(SETTINGS_FILE), OLD_SETTINGS).unwrap();
    claude_files::write(root).unwrap();
    assert!(!root.join(MCP_FILE).exists());
    assert!(!root.join(SETTINGS_FILE).exists());
    assert!(root.join(".claude").is_dir(), "the skill keeps the folder");

    std::fs::write(root.join("shared.json"), OLD_MCP).unwrap();
    std::os::unix::fs::symlink("shared.json", root.join(MCP_FILE)).unwrap();
    claude_files::write(root).unwrap();
    assert!(is_link(&root.join(MCP_FILE)));
    assert_eq!(read(root, "shared.json"), "{}\n");
}

#[test]
fn a_malformed_json_file_is_refused_and_nothing_is_written() {
    for (relative, content) in [
        (MCP_FILE, "{\"mcpServers\": "),
        (MCP_FILE, "[]"),
        (MCP_FILE, "{\"mcpServers\": []}"),
        (SETTINGS_FILE, "not json"),
        (
            SETTINGS_FILE,
            "{\"permissions\": {\"allow\": \"mcp__fold\"}}",
        ),
        (SETTINGS_FILE, "{\"hooks\": {\"Stop\": {}}}"),
    ] {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(relative), content).unwrap();
        let error = malformed(claude_files::write(root).unwrap_err());
        assert_eq!(error.code(), "invalid_request", "{content}");
        let details = error.details().unwrap();
        assert_eq!(details["reason"], MALFORMED_REASON);
        let path = root.join(relative);
        assert!(error.message().starts_with(&path.display().to_string()));
        assert_eq!(read(root, relative), content, "the file stays as it was");
        assert!(!root.join(AGENTS_FILE).exists());
        assert!(!root.join(SKILL_FILE).exists());
    }
}

#[test]
fn a_new_agents_file_holds_only_the_marked_section() {
    let block = format!("{AGENTS_BEGIN}\n## fold\n\nStub section.\n{AGENTS_END}\n");
    assert_eq!(agents_md(None, STUB).unwrap(), block);
    assert_eq!(agents_md(Some(""), STUB).unwrap(), block);
}

#[test]
fn an_agents_file_without_markers_keeps_its_bytes_as_the_prefix() {
    let block = format!("{AGENTS_BEGIN}\n## fold\n\nStub section.\n{AGENTS_END}\n");
    let with_newline = "# Rules\r\n\nBe kind.\n";
    assert_eq!(
        agents_md(Some(with_newline), STUB).unwrap(),
        format!("{with_newline}\n{block}")
    );
    let without_newline = "# Rules\n\nBe kind.";
    assert_eq!(
        agents_md(Some(without_newline), STUB).unwrap(),
        format!("{without_newline}\n\n{block}")
    );
}

#[test]
fn a_refresh_replaces_only_the_text_between_the_markers() {
    let before = "# Rules\n\nkeep this\n";
    let after = "\n## Later\ntrailing bytes without a newline";
    let existing = format!("{before}{AGENTS_BEGIN}\nold fold text\nmore\n{AGENTS_END}{after}");
    assert_eq!(
        agents_md(Some(&existing), STUB).unwrap(),
        format!("{before}{AGENTS_BEGIN}\n## fold\n\nStub section.\n{AGENTS_END}{after}")
    );
    let refreshed = agents_md(Some(&existing), STUB).unwrap();
    assert_eq!(agents_md(Some(&refreshed), STUB).unwrap(), refreshed);
    let inline = format!("Say {AGENTS_BEGIN} in prose.\n");
    assert!(agents_md(Some(&inline), STUB).unwrap().starts_with(&inline));
}

#[test]
fn unmatched_or_repeated_markers_are_refused_and_leave_the_file_untouched() {
    let pair = format!("{AGENTS_BEGIN}\nx\n{AGENTS_END}\n");
    for (existing, problem) in [
        (
            format!("a\n{AGENTS_BEGIN}\nb\n"),
            "begin marker without an end marker",
        ),
        (
            format!("a\n{AGENTS_END}\n"),
            "end marker without a begin marker",
        ),
        (
            format!("{AGENTS_END}\n{AGENTS_BEGIN}\n"),
            "end marker before its begin marker",
        ),
        (format!("{pair}{pair}"), "more than one fold section"),
        (
            format!("{pair}{AGENTS_END}\n"),
            "more than one fold section",
        ),
    ] {
        let refused = agents_md(Some(&existing), STUB).unwrap_err();
        assert!(refused.contains(problem), "{refused}");

        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(root.join(AGENTS_FILE), &existing).unwrap();
        let error = malformed(claude_files::write(root).unwrap_err());
        assert_eq!(error.code(), "invalid_request");
        assert_eq!(error.details().unwrap()["reason"], MALFORMED_REASON);
        assert!(error.message().contains(AGENTS_FILE));
        assert_eq!(read(root, AGENTS_FILE), existing);
        assert!(!root.join(SKILL_FILE).exists());
    }
}

#[test]
fn shadowing_files_finds_each_name_in_the_root_and_its_parents_nearest_first() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().canonicalize().unwrap().join("parent");
    let root = parent.join("project");
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    std::fs::create_dir_all(parent.join(".claude")).unwrap();
    let home = dir.path().canonicalize().unwrap().join("home");
    let mut expected = Vec::new();
    std::fs::write(root.join("rules.md"), "rules").unwrap();
    for dir in [&root, &parent] {
        for name in ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"] {
            assert_eq!(
                shadowing_files(&root, Some(&home))
                    .into_iter()
                    .filter(|path| path.starts_with(&parent))
                    .collect::<Vec<_>>(),
                expected
            );
            // A root CLAUDE.md file gets fold's section, so only a link to another file hides it.
            if dir == &root && name == CLAUDE_FILE {
                std::os::unix::fs::symlink("rules.md", dir.join(name)).unwrap();
            } else {
                std::fs::write(dir.join(name), "rules").unwrap();
            }
            expected.push(dir.join(name));
        }
    }
    let found: Vec<PathBuf> = shadowing_files(&root, Some(&home))
        .into_iter()
        .filter(|path| path.starts_with(&parent))
        .collect();
    assert_eq!(
        found,
        [
            root.join("CLAUDE.md"),
            root.join(".claude/CLAUDE.md"),
            root.join("CLAUDE.local.md"),
            parent.join("CLAUDE.md"),
            parent.join(".claude/CLAUDE.md"),
            parent.join("CLAUDE.local.md"),
        ]
    );
}

#[test]
fn home_claude_md_is_not_reported() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().canonicalize().unwrap().join("home");
    let root = home.join("work").join("demo");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/CLAUDE.md"), "mine").unwrap();
    let under = |path: &PathBuf| path.starts_with(&home);
    assert!(
        shadowing_files(&root, Some(&home))
            .iter()
            .all(|path| !under(path))
    );
    std::fs::write(home.join("CLAUDE.md"), "shared").unwrap();
    let found: Vec<PathBuf> = shadowing_files(&root, Some(&home))
        .into_iter()
        .filter(under)
        .collect();
    assert_eq!(found, [home.join("CLAUDE.md")]);

    let sandbox = Sandbox::new();
    let project = sandbox.home().join("demo");
    std::fs::create_dir_all(sandbox.home().join(".claude")).unwrap();
    std::fs::write(sandbox.home().join(".claude/CLAUDE.md"), "mine").unwrap();
    let output = sandbox.fold(&["init", project.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert!(sandbox.warnings(&output).is_empty());
    std::fs::write(sandbox.home().join("CLAUDE.md"), "shared").unwrap();
    let output = sandbox.fold(&["init", "--refresh", project.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        sandbox.warnings(&output),
        [format!(
            "warning: {} makes Claude Code skip AGENTS.md; move its text into AGENTS.md and delete it",
            sandbox.home().join("CLAUDE.md").display()
        )]
    );
}

/// A host with its own HOME and data directory and no runtime directory, so no run reaches a daemon.
#[cfg(test)]
struct Sandbox {
    dir: TempDir,
    base: PathBuf,
}

#[cfg(test)]
impl Sandbox {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let base = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(base.join("home")).unwrap();
        Self { dir, base }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }

    fn home(&self) -> PathBuf {
        self.path("home")
    }

    fn fold(&self, args: &[&str]) -> Output {
        self.fold_in(args, self.dir.path())
    }

    fn fold_in(&self, args: &[&str], cwd: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", self.home())
            .env("XDG_DATA_HOME", self.path("data"));
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.output().unwrap()
    }

    /// The warning lines about files inside the sandbox; files above it belong to the machine.
    fn warnings(&self, output: &Output) -> Vec<String> {
        let prefix = format!("warning: {}", self.base.display());
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.starts_with(&prefix))
            .map(str::to_owned)
            .collect()
    }
}

#[test]
fn init_warns_for_each_shadowing_file_writes_the_files_and_exits_0() {
    let sandbox = Sandbox::new();
    let parent = sandbox.path("parent");
    std::fs::create_dir_all(parent.join(".claude")).unwrap();
    let names = ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"];
    for name in names {
        std::fs::write(parent.join(name), "rules").unwrap();
    }
    let project = parent.join("project");
    let output = sandbox.fold(&["init", project.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let warning = |path: PathBuf| {
        format!(
            "warning: {} makes Claude Code skip AGENTS.md; move its text into AGENTS.md and delete it",
            path.display()
        )
    };
    assert_eq!(
        sandbox.warnings(&output),
        names.map(|name| warning(parent.join(name)))
    );
    assert!(!project.join(MCP_FILE).exists());
    assert!(!project.join(SETTINGS_FILE).exists());
    assert!(project.join(AGENTS_FILE).is_file() && project.join(SKILL_FILE).is_file());

    std::fs::create_dir_all(project.join(".claude")).unwrap();
    for name in names {
        std::fs::write(project.join(name), "rules\n").unwrap();
    }
    let output = sandbox.fold_in(&["init", "--refresh"], &project.join(".claude"));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let expected: Vec<String> = names[1..]
        .iter()
        .map(|name| warning(project.join(name)))
        .chain(names.map(|name| warning(parent.join(name))))
        .collect();
    assert_eq!(sandbox.warnings(&output), expected);
    let claude = read(&project, CLAUDE_FILE);
    assert!(claude.starts_with("rules\n\n") && claude.contains(AGENTS_BEGIN));
    assert_eq!(read(&project, AGENTS_FILE), "");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        stdout.ends_with(&format!(
            "refreshed the Claude Code files at {}\n",
            project.display()
        )),
        "{stdout}"
    );
}

/// Every file below `dir` by relative path, with its bytes.
#[cfg(test)]
fn tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(dir).unwrap().to_owned();
                files.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

#[test]
fn refresh_never_touches_the_workspaces() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("project");
    let output = sandbox.fold(&["init", project.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");

    let workspace = project.join(".fold/ws/t-1");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("AGENTS.md"), "workspace copy\n").unwrap();
    let ws = tree(&project.join(".fold/ws"));
    let output = sandbox.fold_in(&["init", "--refresh"], &workspace);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(tree(&project.join(".fold/ws")), ws);
    assert!(project.join(SKILL_FILE).is_file());
}

#[test]
fn refresh_outside_a_project_and_with_a_malformed_file_fails() {
    let sandbox = Sandbox::new();
    let outside = sandbox.path("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let output = sandbox.fold_in(&["init", "--refresh"], &outside);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("project_not_found"));

    let project = sandbox.path("project");
    assert!(
        sandbox
            .fold(&["init", project.to_str().unwrap()])
            .status
            .success()
    );
    std::fs::write(project.join(MCP_FILE), "{").unwrap();
    let output = sandbox.fold(&["init", "--refresh", project.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains(&format!(
            "invalid_request: {} is not valid JSON",
            project.join(MCP_FILE).display()
        )),
        "{stderr}"
    );
    assert_eq!(read(&project, MCP_FILE), "{");
}

/// The number of fold sections in a text.
#[cfg(test)]
fn sections(text: &str) -> usize {
    text.lines().filter(|line| *line == AGENTS_BEGIN).count()
}

#[cfg(test)]
fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .unwrap()
        .file_type()
        .is_symlink()
}

#[test]
fn the_section_goes_into_claude_md_when_it_is_a_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join(CLAUDE_FILE), "# mine\n").unwrap();
    assert_eq!(instructions_file(root), CLAUDE_FILE);
    claude_files::write(root).unwrap();
    let claude = read(root, CLAUDE_FILE);
    assert!(
        claude.starts_with(&format!("# mine\n\n{AGENTS_BEGIN}\n")),
        "{claude}"
    );
    assert!(!root.join(AGENTS_FILE).exists());

    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join(CLAUDE_FILE), "# claude\n").unwrap();
    std::fs::write(root.join(AGENTS_FILE), "# agents\n").unwrap();
    claude_files::write(root).unwrap();
    assert_eq!(sections(&read(root, CLAUDE_FILE)), 1);
    assert_eq!(read(root, AGENTS_FILE), "# agents\n");
    let canonical = root.canonicalize().unwrap();
    assert!(
        shadowing_files(&canonical, None)
            .iter()
            .all(|path| !path.starts_with(&canonical))
    );
    assert_eq!(stale_section(root), None);
}

#[test]
fn the_section_goes_into_agents_md_when_claude_md_is_absent_or_a_link() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    assert_eq!(instructions_file(root), AGENTS_FILE);
    std::fs::write(root.join(AGENTS_FILE), "# agents\n").unwrap();
    claude_files::write(root).unwrap();
    assert!(read(root, AGENTS_FILE).starts_with("# agents\n\n"));
    assert!(!root.join(CLAUDE_FILE).exists());

    for agents in [None, Some("# agents\n")] {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        if let Some(text) = agents {
            std::fs::write(root.join(AGENTS_FILE), text).unwrap();
        }
        std::os::unix::fs::symlink(AGENTS_FILE, root.join(CLAUDE_FILE)).unwrap();
        assert_eq!(instructions_file(root), AGENTS_FILE);
        claude_files::write(root).unwrap();
        claude_files::write(root).unwrap();
        assert!(is_link(&root.join(CLAUDE_FILE)));
        assert!(!is_link(&root.join(AGENTS_FILE)));
        let text = read(root, AGENTS_FILE);
        assert!(text.starts_with(agents.unwrap_or(AGENTS_BEGIN)), "{text}");
        assert_eq!(sections(&text), 1);
        let canonical = root.canonicalize().unwrap();
        assert!(
            shadowing_files(&canonical, None)
                .iter()
                .all(|path| !path.starts_with(&canonical))
        );
        assert_eq!(stale_section(root), None);
    }
}

#[test]
fn an_agents_md_link_to_claude_md_stays_a_link_and_the_file_holds_one_section() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join(CLAUDE_FILE), "# claude\n").unwrap();
    std::os::unix::fs::symlink(CLAUDE_FILE, root.join(AGENTS_FILE)).unwrap();
    assert_eq!(instructions_file(root), CLAUDE_FILE);
    claude_files::write(root).unwrap();
    claude_files::write(root).unwrap();
    assert!(is_link(&root.join(AGENTS_FILE)));
    let text = read(root, CLAUDE_FILE);
    assert!(text.starts_with("# claude\n\n"));
    assert_eq!(sections(&text), 1);
    assert_eq!(stale_section(root), None);
}

#[test]
fn an_instructions_link_that_leads_outside_the_root_or_nowhere_is_refused() {
    let outside = TempDir::new().unwrap();
    let target = outside.path().join("rules.md");
    std::fs::write(&target, "# shared\n").unwrap();
    std::fs::create_dir(outside.path().join("folder")).unwrap();
    for (link, problem) in [
        (target.clone(), "leads outside the project folder"),
        (PathBuf::from("missing.md"), "leads to no file"),
        (PathBuf::from(".claude"), "leads to no regular file"),
    ] {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".claude")).unwrap();
        std::os::unix::fs::symlink(&link, root.join(AGENTS_FILE)).unwrap();
        let error = match claude_files::write(root).unwrap_err() {
            ClaudeFileError::Link(error) => error,
            other => panic!("expected a refused link, got {other:?}"),
        };
        assert_eq!(error.code(), "invalid_path");
        assert!(error.message().ends_with(problem), "{}", error.message());
        assert!(error.message().contains(AGENTS_FILE));
        assert_eq!(
            error.hint(),
            Some(
                "point AGENTS.md at a file inside the project folder or replace the link with a file, then run the command again"
            )
        );
        assert!(is_link(&root.join(AGENTS_FILE)));
        assert!(!root.join(SKILL_FILE).exists(), "nothing is written");
    }
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "# shared\n");
}

/// Claude Code reads the file a `CLAUDE.md` link leads to, so a link to anything but `AGENTS.md` still hides the section and keeps its warning.
#[test]
fn a_claude_md_link_to_another_file_gets_agents_md_and_keeps_the_warning() {
    let outside = TempDir::new().unwrap();
    let target = outside.path().join("rules.md");
    std::fs::write(&target, "# shared\n").unwrap();
    let dir = TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::os::unix::fs::symlink(&target, root.join(CLAUDE_FILE)).unwrap();
    claude_files::write(&root).unwrap();
    assert!(is_link(&root.join(CLAUDE_FILE)));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "# shared\n");
    assert_eq!(sections(&read(&root, AGENTS_FILE)), 1);
    assert!(shadowing_files(&root, None).contains(&root.join(CLAUDE_FILE)));
}

#[test]
fn the_section_moves_between_the_files_and_the_old_one_keeps_its_other_bytes() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join(AGENTS_FILE), "# agents\n").unwrap();
    claude_files::write(root).unwrap();
    let with_tail = format!("{}tail\n", read(root, AGENTS_FILE));
    std::fs::write(root.join(AGENTS_FILE), &with_tail).unwrap();

    std::fs::write(root.join(CLAUDE_FILE), "# claude\n").unwrap();
    assert_eq!(stale_section(root), Some(AGENTS_FILE));
    claude_files::write(root).unwrap();
    assert_eq!(read(root, AGENTS_FILE), "# agents\n\ntail\n");
    assert_eq!(sections(&read(root, CLAUDE_FILE)), 1);
    assert_eq!(stale_section(root), None);
    let before = std::fs::metadata(root.join(AGENTS_FILE))
        .unwrap()
        .modified()
        .unwrap();
    claude_files::write(root).unwrap();
    let after = std::fs::metadata(root.join(AGENTS_FILE))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(before, after, "a file without a section is not written");

    std::fs::remove_file(root.join(CLAUDE_FILE)).unwrap();
    claude_files::write(root).unwrap();
    let agents = read(root, AGENTS_FILE);
    assert!(agents.starts_with("# agents\n\ntail\n\n"), "{agents}");
    assert_eq!(sections(&agents), 1);
}

#[test]
fn stale_markers_that_cannot_be_removed_refuse_the_write() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join(CLAUDE_FILE), "# claude\n").unwrap();
    let broken = format!("# agents\n{AGENTS_BEGIN}\n");
    std::fs::write(root.join(AGENTS_FILE), &broken).unwrap();
    assert_eq!(stale_section(root), Some(AGENTS_FILE));
    let error = malformed(claude_files::write(root).unwrap_err());
    assert!(error.message().contains(AGENTS_FILE), "{}", error.message());
    assert_eq!(read(root, CLAUDE_FILE), "# claude\n");
    assert_eq!(read(root, AGENTS_FILE), broken);
}

#[test]
fn removing_the_section_keeps_every_byte_outside_the_markers() {
    assert_eq!(without_section("no markers\n").unwrap(), None);
    let text = format!("a\r\n{AGENTS_BEGIN}\r\nx\n{AGENTS_END}\nb");
    assert_eq!(without_section(&text).unwrap().as_deref(), Some("a\r\nb"));
    let last = format!("a\n{AGENTS_BEGIN}\n{AGENTS_END}");
    assert_eq!(without_section(&last).unwrap().as_deref(), Some("a\n"));
    assert!(without_section(&format!("{AGENTS_END}\n")).is_err());
}

#[test]
fn refresh_with_a_refused_link_prints_the_hint() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("project");
    let output = sandbox.fold(&["init", project.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    std::fs::remove_file(project.join(AGENTS_FILE)).unwrap();
    std::os::unix::fs::symlink(sandbox.path("rules.md"), project.join(AGENTS_FILE)).unwrap();
    std::fs::write(sandbox.path("rules.md"), "# shared\n").unwrap();
    let output = sandbox.fold(&["init", "--refresh", project.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.contains("invalid_path"), "{stderr}");
    assert!(
        stderr.contains("point AGENTS.md at a file inside the project folder"),
        "{stderr}"
    );
}
