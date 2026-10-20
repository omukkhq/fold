use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fold_core::transcript::{ProjectFolder, claude_project_folder, transcript_file_name};

/// The variables from which Claude Code finds the folder of its transcripts, as [`fold_core::transcript::claude_config_dir`] reads them.
pub const LOCATION_ENV: [&str; 3] = ["CLAUDE_CONFIG_DIR", "CLAUDE_CODE_PROJECT_DIR_NAME", "HOME"];

/// The path of the transcript that Claude Code keeps for the session `id` when it runs in the folder `root` with its own folder `config_dir`. For a root whose folder name fits, it is the path whether or not the file exists yet; for a longer root, whose folder name ends in a hash fold cannot compute, it is the file in a folder of the cut name that holds it, and `None` when no such folder holds it. `None` for an id that is not a session id. It never opens the file, which holds the conversation. Blocking.
pub fn transcript_path(config_dir: &Path, root: &Path, id: &str) -> Option<PathBuf> {
    let file = transcript_file_name(id)?;
    let projects = config_dir.join("projects");
    match claude_project_folder(root) {
        ProjectFolder::Named(name) => Some(projects.join(name).join(file)),
        ProjectFolder::Prefixed(prefix) => std::fs::read_dir(&projects)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(&prefix))
            })
            .map(|entry| entry.path().join(&file))
            .find(|path| path.is_file()),
    }
}

/// The value of `name` in `env`, where a later entry wins as it does for a process that gets them in order.
pub fn env_value(env: &[(String, String)], name: &str) -> Option<OsString> {
    env.iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| OsString::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0f8e2c1a-5b7d-4c3e-9a1f-2d4b6c8e0a13";

    #[test]
    fn a_fitting_root_names_its_transcript_whether_or_not_it_exists() {
        let config = tempfile::tempdir().unwrap();
        let root = Path::new("/home/ana/my.app/web_2");
        let expected = config
            .path()
            .join("projects/-home-ana-my-app-web-2")
            .join(format!("{ID}.jsonl"));
        assert_eq!(transcript_path(config.path(), root, ID), Some(expected));
        assert_eq!(
            transcript_path(config.path(), root, "../../etc/passwd"),
            None
        );
    }

    #[test]
    fn a_long_root_finds_the_cut_folder_that_holds_the_transcript() {
        let config = tempfile::tempdir().unwrap();
        let root = PathBuf::from(format!("/{}", "a".repeat(250)));
        assert_eq!(transcript_path(config.path(), &root, ID), None);
        let cut = format!("-{}-1x2y3z", "a".repeat(199));
        let folder = config.path().join("projects").join(cut);
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join(format!("{ID}.jsonl"));
        std::fs::write(&file, "").unwrap();
        assert_eq!(transcript_path(config.path(), &root, ID), Some(file));
        assert_eq!(
            transcript_path(config.path(), &root, &ID.replace('0', "1")),
            None
        );
        let other = "11111111-2222-4333-8444-555555555555";
        let uncut = config
            .path()
            .join("projects")
            .join(format!("-{}", "a".repeat(250)));
        std::fs::create_dir_all(&uncut).unwrap();
        std::fs::write(uncut.join(format!("{other}.jsonl")), "").unwrap();
        assert_eq!(
            transcript_path(config.path(), &root, other),
            None,
            "only the cut name, a dash and the hash"
        );
    }

    #[test]
    fn the_later_of_two_entries_wins() {
        let env = vec![
            ("HOME".to_owned(), "/home/a".to_owned()),
            ("HOME".to_owned(), "/home/b".to_owned()),
        ];
        assert_eq!(env_value(&env, "HOME"), Some(OsString::from("/home/b")));
        assert_eq!(env_value(&env, "CLAUDE_CONFIG_DIR"), None);
    }
}
