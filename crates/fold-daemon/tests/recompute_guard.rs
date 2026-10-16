use std::fs;
use std::path::{Path, PathBuf};

/// The one file that holds the SQL writing the status cache.
const CACHE_SQL: &str = "fold-store/src/tasks/recompute.rs";
/// The one file that calls it.
const CACHE_CALLER: &str = "fold-daemon/src/services/tasks/recompute.rs";
/// The one file that writes completion rows.
const COMPLETION_SQL: &str = "fold-store/src/tasks/completions.rs";

#[cfg(test)]
fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Every Rust source under `crates/*/src` and every migration, as paths relative to `crates/` with their text.
#[cfg(test)]
fn sources() -> Vec<(String, String)> {
    let root = crates_dir();
    let mut found = Vec::new();
    let mut stack = Vec::new();
    for entry in fs::read_dir(&root).unwrap() {
        let krate = entry.unwrap().path();
        stack.push(krate.join("src"));
        stack.push(krate.join("migrations"));
    }
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|ext| ext == "rs" || ext == "sql")
            {
                let relative = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                found.push((relative, fs::read_to_string(&path).unwrap()));
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
fn normalise(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// True when `clause` assigns `column`: the column as a whole word followed by `=`.
#[cfg(test)]
fn assigns(clause: &str, column: &str) -> bool {
    let bytes = clause.as_bytes();
    clause.match_indices(column).any(|(at, _)| {
        let before = at == 0 || !is_word_byte(bytes[at - 1]);
        let rest = clause[at + column.len()..].trim_start();
        before && rest.starts_with('=') && !rest.starts_with("==")
    })
}

/// True when the text has an `INSERT` or `REPLACE` into `tasks` whose column list names `status` or `blocked`: a new row takes the column default, and recompute sets the cache.
#[cfg(test)]
fn inserts_status_cache(text: &str) -> bool {
    text.match_indices("into ").any(|(at, keyword)| {
        if at > 0 && is_word_byte(text.as_bytes()[at - 1]) {
            return false;
        }
        let rest = text[at + keyword.len()..].trim_start_matches(['"', '`', '[']);
        let Some(rest) = rest.strip_prefix("tasks") else {
            return false;
        };
        let rest = rest.trim_start_matches(['"', '`', ']']).trim_start();
        let Some(columns) = rest.strip_prefix('(') else {
            return false;
        };
        let columns = columns.split(')').next().unwrap_or(columns);
        columns
            .split(',')
            .map(|column| column.trim().trim_matches(['"', '`', '[', ']']))
            .any(|column| column == "status" || column == "blocked")
    })
}

/// True when the text has an `UPDATE tasks ... SET` whose assignments name `status` or `blocked`, or an insert that sets them.
#[cfg(test)]
fn writes_status_cache(text: &str) -> bool {
    let text = normalise(text);
    if inserts_status_cache(&text) {
        return true;
    }
    text.match_indices("update ").any(|(at, keyword)| {
        if at > 0 && is_word_byte(text.as_bytes()[at - 1]) {
            return false;
        }
        let mut rest = &text[at + keyword.len()..];
        // `UPDATE OR <action> tasks` is still an update of the table.
        if let Some(after_or) = rest.strip_prefix("or ") {
            rest = after_or.split_once(' ').map_or("", |(_, tail)| tail);
        }
        let rest = rest.trim_start_matches(['"', '`', '[']);
        let Some(rest) = rest.strip_prefix("tasks") else {
            return false;
        };
        let rest = rest.trim_start_matches(['"', '`', ']']);
        let Some(clause) = rest.trim_start().strip_prefix("set ") else {
            return false;
        };
        let end = clause.find([';', '"']).unwrap_or(clause.len());
        let clause = &clause[..end];
        let clause = clause.split(" where ").next().unwrap_or(clause);
        assigns(clause, "status") || assigns(clause, "blocked")
    })
}

#[test]
fn the_detector_finds_status_and_blocked_writes_in_any_layout() {
    for sql in [
        "UPDATE tasks SET status = ?2, blocked = ?3 WHERE number = ?1",
        "update tasks\n    set blocked=1",
        "UPDATE tasks SET title = ?1, status = 'done' WHERE id = 1;",
        "UPDATE OR IGNORE tasks SET status = 'todo'",
        "UPDATE \"tasks\" SET blocked = 0",
        "INSERT INTO tasks (number, title, status) VALUES (1, 'a', 'todo')",
        "insert or replace into \"tasks\" (number,blocked) values (1, 1)",
        "REPLACE INTO tasks (number, status) VALUES (1, 'done')",
    ] {
        assert!(writes_status_cache(sql), "{sql}");
    }
    for sql in [
        "UPDATE tasks SET title = ?2, description = ?3, updated_at = ?4 WHERE number = ?1",
        "UPDATE tasks SET archived_at = 1 WHERE status = 'done'",
        "UPDATE workspaces SET status = 'abandoned'",
        "UPDATE tasks_archive SET status = 'done'",
        "SELECT status, blocked FROM tasks",
        "UPDATE tasks SET human_review_status = 1",
        "INSERT INTO tasks (number, title, description, created_by, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        "INSERT INTO task_labels (task_id, status) VALUES (1, 'x')",
        "INSERT INTO tasks_archive (number, status) VALUES (1, 'done')",
    ] {
        assert!(!writes_status_cache(sql), "{sql}");
    }
}

#[test]
fn only_recompute_writes_tasks_status_or_tasks_blocked() {
    let sources = sources();
    assert!(
        sources.iter().any(|(path, _)| path == CACHE_SQL),
        "{CACHE_SQL} is missing"
    );
    let writers: Vec<&str> = sources
        .iter()
        .filter(|(_, text)| writes_status_cache(text))
        .map(|(path, _)| path.as_str())
        .collect();
    assert_eq!(writers, [CACHE_SQL]);
}

#[test]
fn only_recompute_calls_the_cache_writer() {
    let callers: Vec<String> = sources()
        .into_iter()
        .filter(|(_, text)| text.replace("fn write_cache(", "").contains("write_cache("))
        .map(|(path, _)| path)
        .collect();
    assert_eq!(callers, [CACHE_CALLER]);
}

#[test]
fn only_completions_insert_writes_completion_rows() {
    // A migration that rebuilds a table copies the rows that exist; it is not a command that completes a task.
    let writers: Vec<String> = sources()
        .into_iter()
        .filter(|(path, _)| !path.contains("/migrations/"))
        .filter(|(_, text)| normalise(text).contains("into completions"))
        .map(|(path, _)| path)
        .collect();
    assert_eq!(writers, [COMPLETION_SQL]);
}
