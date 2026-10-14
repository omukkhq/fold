use std::path::{Path, PathBuf};

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
