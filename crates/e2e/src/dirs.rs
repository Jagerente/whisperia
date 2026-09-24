use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static MADE: AtomicU64 = AtomicU64::new(0);

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let number = MADE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("ww-{tag}-{}-{number}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, part: impl AsRef<Path>) -> PathBuf {
        self.path.join(part)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_dirs_with_one_tag_are_apart() {
        assert_ne!(TempDir::new("same").path(), TempDir::new("same").path());
    }

    #[test]
    fn a_dir_goes_away_with_its_owner() {
        let dir = TempDir::new("gone");
        std::fs::create_dir_all(dir.join("inside")).unwrap();
        let path = dir.path().to_owned();
        drop(dir);
        assert!(!path.exists());
    }
}
