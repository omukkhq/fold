use std::path::{Path, PathBuf};

use crate::error::{FoldError, codes};

/// Where the host-wide files live: `<data home>/fold/`. The data home is `XDG_DATA_HOME`, else `<home>/.local/share`. An empty or relative value counts as unset, as the XDG base directory rules require.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPaths {
    fold_dir: PathBuf,
}

impl HostPaths {
    pub fn new(xdg_data_home: Option<&str>, home: Option<&str>) -> Result<Self, FoldError> {
        let data_home = match (absolute(xdg_data_home), absolute(home)) {
            (Some(data), _) => PathBuf::from(data),
            (None, Some(home)) => Path::new(home).join(".local").join("share"),
            (None, None) => {
                return Err(FoldError::new(
                    codes::INVALID_HOST_PATHS,
                    "neither XDG_DATA_HOME nor HOME is set to an absolute path",
                ));
            }
        };
        Ok(Self {
            fold_dir: data_home.join("fold"),
        })
    }

    pub fn registry_db(&self) -> PathBuf {
        self.fold_dir.join("registry.db")
    }
}

fn absolute(value: Option<&str>) -> Option<&str> {
    value.filter(|text| Path::new(text).is_absolute())
}

/// Where the files of one project live. Every method only joins names onto the root; nothing here touches the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPaths {
    root: PathBuf,
}

impl ProjectPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn fold_dir(&self) -> PathBuf {
        self.root.join(".fold")
    }

    pub fn db(&self) -> PathBuf {
        self.fold_dir().join("fold.db")
    }

    pub fn objects(&self) -> PathBuf {
        self.fold_dir().join("objects")
    }

    pub fn index(&self) -> PathBuf {
        self.fold_dir().join("index")
    }

    pub fn config(&self) -> PathBuf {
        self.fold_dir().join("config.toml")
    }

    /// The read-only copy of the current main revision.
    pub fn main_dir(&self) -> PathBuf {
        self.root.join("main")
    }

    /// The parent of every workspace and review checkout directory.
    pub fn ws_dir(&self) -> PathBuf {
        self.root.join("ws")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_paths_fall_back_to_home() {
        let xdg = HostPaths::new(Some("/data"), Some("/home/ana")).unwrap();
        assert_eq!(xdg.registry_db(), Path::new("/data/fold/registry.db"));
        let home = HostPaths::new(None, Some("/home/ana")).unwrap();
        assert_eq!(
            home.registry_db(),
            Path::new("/home/ana/.local/share/fold/registry.db")
        );
        let empty = HostPaths::new(Some(""), Some("/home/ana")).unwrap();
        assert_eq!(empty, home);
        let error = HostPaths::new(None, None).unwrap_err();
        assert_eq!(error.code(), "invalid_host_paths");
        assert!(HostPaths::new(Some(""), Some("")).is_err());
        let relative = HostPaths::new(Some("data"), Some("/home/ana")).unwrap();
        assert_eq!(relative, home);
        assert!(HostPaths::new(None, Some("home/ana")).is_err());
    }

    #[test]
    fn every_path_hangs_off_the_root() {
        let paths = ProjectPaths::new("/srv/demo");
        assert_eq!(paths.root(), Path::new("/srv/demo"));
        assert_eq!(paths.fold_dir(), Path::new("/srv/demo/.fold"));
        assert_eq!(paths.db(), Path::new("/srv/demo/.fold/fold.db"));
        assert_eq!(paths.objects(), Path::new("/srv/demo/.fold/objects"));
        assert_eq!(paths.index(), Path::new("/srv/demo/.fold/index"));
        assert_eq!(paths.config(), Path::new("/srv/demo/.fold/config.toml"));
        assert_eq!(paths.main_dir(), Path::new("/srv/demo/main"));
        assert_eq!(paths.ws_dir(), Path::new("/srv/demo/ws"));
    }
}
