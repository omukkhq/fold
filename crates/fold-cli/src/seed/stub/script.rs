use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

/// What one stub worker does, turn by turn: the file `.fold/dev-stub/<agent>.json` of a seeded project. The seed writes it before it starts the worker; the stub reads it when its process starts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub turns: Vec<Turn>,
}

/// The work of one user turn. The worker runs its turns in order, one per user turn fold sends, counted over the agent's whole life, across resumes and fresh sessions. A user turn after the last one, or one that `when` does not match, is answered without running anything.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    /// The turn runs only for a user turn whose text contains this, such as the notice of a change request; another user turn leaves it next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// The text the turn ends with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
}

/// One step. Every string in a step can name a value an earlier call of the same agent saved, as `${name}`. A path is relative to the worker's folder, the project root, and never leaves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// A call of a tool of the `fold` MCP server.
    Call(Call),
    /// Writes a whole file, creating it and its folders when they are new.
    Write {
        path: String,
        text: String,
    },
    /// Puts `text` after the 1-based line `after` of a file, or at its end when the file is shorter.
    Insert {
        path: String,
        after: usize,
        text: String,
    },
    /// Replaces the 1-based line `line` of a file with `text`, which may hold several lines.
    Replace {
        path: String,
        line: usize,
        text: String,
    },
    Remove {
        path: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub tool: String,
    #[serde(default)]
    pub arguments: Map<String, Value>,
    /// The values to keep for later steps: a name for each JSON pointer into the tool's result, such as `"ws": "/workspace"`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub save: BTreeMap<String, String>,
    /// The call must fail with this error code; a success fails the step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// The call is repeated until this JSON pointer into its result holds a value other than null, as an agent polls for work another agent does; a refused call ends the step at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScriptError {
    #[error("the script names ${{{0}}}, which no earlier call saved")]
    Unknown(String),
    #[error("{0} is not a path inside the project")]
    Outside(String),
    #[error("the result of {tool} has nothing at {pointer}")]
    Missing { tool: String, pointer: String },
}

/// `text` with each `${name}` replaced by its saved value. A `$` that starts no `${…}` stays as it is.
pub fn expand(text: &str, vars: &BTreeMap<String, String>) -> Result<String, ScriptError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let name = &after[..end];
        let value = vars
            .get(name)
            .ok_or_else(|| ScriptError::Unknown(name.to_owned()))?;
        out.push_str(value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `value` with [`expand`] applied to every string in it, keys excepted.
pub fn expand_value(value: &Value, vars: &BTreeMap<String, String>) -> Result<Value, ScriptError> {
    Ok(match value {
        Value::String(text) => Value::String(expand(text, vars)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| expand_value(item, vars))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, item)| Ok((key.clone(), expand_value(item, vars)?)))
                .collect::<Result<_, ScriptError>>()?,
        ),
        other => other.clone(),
    })
}

/// The value at `pointer` in a tool's result, as text: a string as it is, any other value as its JSON.
pub fn saved(result: &Value, tool: &str, pointer: &str) -> Result<String, ScriptError> {
    match result.pointer(pointer) {
        None | Some(Value::Null) => Err(ScriptError::Missing {
            tool: tool.to_owned(),
            pointer: pointer.to_owned(),
        }),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(other) => Ok(other.to_string()),
    }
}

/// `path` below `root`. An absolute path, a `..` and an empty path are refused, so a script never writes outside the project.
pub fn inside(root: &Path, path: &str) -> Result<PathBuf, ScriptError> {
    let relative = Path::new(path);
    let plain = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    if path.is_empty() || !plain {
        return Err(ScriptError::Outside(path.to_owned()));
    }
    Ok(root.join(relative))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn vars() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("ws".to_owned(), "ws-3".to_owned()),
            ("dir".to_owned(), ".fold/ws/t-1".to_owned()),
        ])
    }

    #[test]
    fn saved_values_fill_every_string_and_an_unknown_name_fails() {
        assert_eq!(
            expand("${dir}/src/a.js", &vars()).unwrap(),
            ".fold/ws/t-1/src/a.js"
        );
        assert_eq!(expand("$5 and ${open", &vars()).unwrap(), "$5 and ${open");
        assert_eq!(
            expand("${nope}", &vars()),
            Err(ScriptError::Unknown("nope".to_owned()))
        );
        let arguments = json!({"ws": "${ws}", "list": ["${ws}", 3], "n": {"deep": "${dir}"}});
        assert_eq!(
            expand_value(&arguments, &vars()).unwrap(),
            json!({"ws": "ws-3", "list": ["ws-3", 3], "n": {"deep": ".fold/ws/t-1"}})
        );
    }

    #[test]
    fn a_saved_value_is_text_and_null_counts_as_missing() {
        let result = json!({"id": "P-2", "version": 1, "proposal": null});
        assert_eq!(saved(&result, "t", "/id").unwrap(), "P-2");
        assert_eq!(saved(&result, "t", "/version").unwrap(), "1");
        assert!(saved(&result, "t", "/proposal").is_err());
        assert!(saved(&result, "t", "/absent").is_err());
    }

    #[test]
    fn a_path_stays_inside_the_project() {
        let root = Path::new("/p");
        assert_eq!(
            inside(root, ".fold/ws/t-1/a.js").unwrap(),
            Path::new("/p/.fold/ws/t-1/a.js")
        );
        for bad in ["", "/etc/passwd", "../x", "a/../../x"] {
            assert!(inside(root, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_format_reads_each_step_and_refuses_unknown_fields() {
        let text = r#"{"turns": [
            {"steps": [
                {"call": {"tool": "create_workspace", "arguments": {"task": "T-1"}, "save": {"dir": "/path"}}},
                {"write": {"path": "${dir}/a.js", "text": "a\n"}},
                {"insert": {"path": "${dir}/a.js", "after": 1, "text": "b"}},
                {"replace": {"path": "${dir}/a.js", "line": 1, "text": "c"}},
                {"remove": {"path": "${dir}/b.js"}},
                {"call": {"tool": "get_task", "arguments": {"id": "T-1"}, "until": "/proposal"}}
            ], "reply": "done"},
            {"when": "changes", "steps": []}
        ]}"#;
        let script: Script = serde_json::from_str(text).unwrap();
        assert_eq!(script.turns.len(), 2);
        assert_eq!(script.turns[0].steps.len(), 6);
        assert_eq!(script.turns[1].when.as_deref(), Some("changes"));
        let again: Script = serde_json::from_str(&serde_json::to_string(&script).unwrap()).unwrap();
        assert_eq!(again, script);
        assert!(serde_json::from_str::<Script>(r#"{"turns": [{"stepz": []}]}"#).is_err());
        assert!(
            serde_json::from_str::<Script>(r#"{"turns": [{"steps": [{"jump": {}}]}]}"#).is_err()
        );
    }
}
