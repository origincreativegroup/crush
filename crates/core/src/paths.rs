use std::path::PathBuf;

/// App data layout. One place decides this so nothing else guesses.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
}

impl AppPaths {
    /// macOS: ~/Library/Application Support/Crush. Overridable via config/env.
    pub fn resolve(override_dir: Option<&PathBuf>) -> anyhow::Result<Self> {
        let root = match override_dir {
            Some(d) => d.clone(),
            None => directories::ProjectDirs::from("dev", "crush", "Crush")
                .ok_or_else(|| anyhow::anyhow!("no app data dir available"))?
                .data_dir()
                .to_path_buf(),
        };
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    pub fn db(&self) -> PathBuf {
        self.root.join("library.db")
    }
    pub fn thumbs(&self) -> PathBuf {
        self.root.join("thumbs")
    }
    pub fn models(&self) -> PathBuf {
        self.root.join("models")
    }
    pub fn debug(&self) -> PathBuf {
        self.root.join("debug")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
}
