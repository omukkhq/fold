use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The longest folder name Claude Code gives a project; a longer name is cut to this length and followed by `-` and a hash of the whole name.
const MAX_FOLDER_NAME: usize = 200;

/// The name of the folder in which Claude Code keeps the transcripts of the sessions it runs in one project folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectFolder {
    /// The whole name.
    Named(String),
    /// The start of the name, up to and including the `-` before the hash. fold cannot compute the hash, so a caller finds the folder by this prefix.
    Prefixed(String),
}

/// The folder name for the project folder `root`. This is Claude Code's own layout, not a documented interface: every UTF-16 unit of the root other than an ASCII letter or digit becomes `-`, and a name longer than 200 characters is cut to 200 and followed by `-` and a hash, which Claude Code itself matches by that prefix.
pub fn claude_project_folder(root: &Path) -> ProjectFolder {
    let mut name = String::new();
    for c in root.to_string_lossy().chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else {
            name.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    if name.len() <= MAX_FOLDER_NAME {
        ProjectFolder::Named(name)
    } else {
        name.truncate(MAX_FOLDER_NAME);
        name.push('-');
        ProjectFolder::Prefixed(name)
    }
}

/// The transcript file name of the session `id`, which is `None` unless `id` is a session id, so a stored id can never name a path outside Claude Code's project folder.
pub fn transcript_file_name(id: &str) -> Option<String> {
    is_session_id(id).then(|| format!("{id}.jsonl"))
}

/// A UUID in its hyphenated text form, the only form of id that fold gives a Claude Code session.
pub fn is_session_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// Claude Code's own folder from the environment `var`: `CLAUDE_CONFIG_DIR`, or `.claude` in the home directory. A relative one is relative to the folder in which `claude` runs, which the caller joins. It is `None` when fold cannot be sure which folder Claude Code keeps the project's transcripts in: Claude Code takes a set but empty `CLAUDE_CONFIG_DIR` as given, and `CLAUDE_CODE_PROJECT_DIR_NAME` can give the project folder another name.
pub fn claude_config_dir(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if var("CLAUDE_CODE_PROJECT_DIR_NAME").is_some_and(|name| !name.is_empty()) {
        return None;
    }
    match var("CLAUDE_CONFIG_DIR") {
        Some(dir) if dir.is_empty() => None,
        Some(dir) => Some(PathBuf::from(dir)),
        None => var("HOME")
            .filter(|home| !home.is_empty())
            .map(|home| PathBuf::from(home).join(".claude")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0f8e2c1a-5b7d-4c3e-9a1f-2d4b6c8e0a13";

    #[test]
    fn every_character_but_an_ascii_letter_or_digit_becomes_a_dash() {
        assert_eq!(
            claude_project_folder(Path::new("/home/ana/my.app/web_2")),
            ProjectFolder::Named("-home-ana-my-app-web-2".to_owned())
        );
        assert_eq!(
            claude_project_folder(Path::new("/home/u/programming/fold")),
            ProjectFolder::Named("-home-u-programming-fold".to_owned())
        );
    }

    #[test]
    fn a_character_outside_utf16s_basic_plane_counts_as_two_dashes() {
        assert_eq!(
            claude_project_folder(Path::new("/home/ana/\u{1f642}app")),
            ProjectFolder::Named("-home-ana---app".to_owned())
        );
        assert_eq!(
            claude_project_folder(Path::new("/srv/caf\u{e9}")),
            ProjectFolder::Named("-srv-caf-".to_owned())
        );
    }

    #[test]
    fn a_long_name_is_cut_to_a_prefix_before_the_hash() {
        let exact = format!("/{}", "a".repeat(199));
        assert_eq!(
            claude_project_folder(Path::new(&exact)),
            ProjectFolder::Named(format!("-{}", "a".repeat(199)))
        );
        let long = format!("/{}", "a".repeat(250));
        assert_eq!(
            claude_project_folder(Path::new(&long)),
            ProjectFolder::Prefixed(format!("-{}-", "a".repeat(199)))
        );
    }

    #[test]
    fn session_ids_are_uuids_only() {
        assert!(is_session_id(ID));
        assert!(!is_session_id(""));
        assert!(!is_session_id("../../../../etc/passwd-aaaaaaaaaaaaaaaaaaa"));
        assert!(!is_session_id("0f8e2c1a-5b7d-4c3e-9a1f-2d4b6c8e0a1g"));
        assert!(!is_session_id("0f8e2c1a05b7d-4c3e-9a1f-2d4b6c8e0a13"));
        assert_eq!(transcript_file_name(ID), Some(format!("{ID}.jsonl")));
        assert_eq!(transcript_file_name("../x"), None);
    }

    #[test]
    fn the_config_dir_is_none_when_the_transcripts_folder_is_unsure() {
        let dir = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, OsString)> = vars
                .iter()
                .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
                .collect();
            claude_config_dir(move |name| {
                vars.iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.clone())
            })
        };
        assert_eq!(
            dir(&[("HOME", "/home/ana")]),
            Some(PathBuf::from("/home/ana/.claude"))
        );
        assert_eq!(
            dir(&[("HOME", "/home/ana"), ("CLAUDE_CONFIG_DIR", "/etc/cc")]),
            Some(PathBuf::from("/etc/cc"))
        );
        assert_eq!(
            dir(&[("HOME", "/home/ana"), ("CLAUDE_CONFIG_DIR", "")]),
            None
        );
        assert_eq!(
            dir(&[
                ("CLAUDE_CONFIG_DIR", "/etc/cc"),
                ("CLAUDE_CODE_PROJECT_DIR_NAME", "app")
            ]),
            None
        );
        assert_eq!(
            dir(&[("HOME", "/home/ana"), ("CLAUDE_CODE_PROJECT_DIR_NAME", "")]),
            Some(PathBuf::from("/home/ana/.claude"))
        );
        assert_eq!(dir(&[("HOME", "")]), None);
        assert_eq!(dir(&[]), None);
    }
}
