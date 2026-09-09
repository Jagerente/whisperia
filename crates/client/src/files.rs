use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{de::DeserializeOwned, Serialize};

pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> (T, Option<String>) {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (T::default(), None),
        Err(e) => return (T::default(), Some(set_aside(path, &e.to_string()))),
    };
    match serde_json::from_str(&text) {
        Ok(value) => (value, None),
        Err(e) => (T::default(), Some(set_aside(path, &e.to_string()))),
    }
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_atomically(path, &serde_json::to_vec_pretty(value)?)
}

pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = beside(path, "new");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)
}

fn set_aside(path: &Path, problem: &str) -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let aside = beside(path, &format!("broken-{seconds}"));
    match fs::rename(path, &aside) {
        Ok(()) => format!(
            "{} could not be read ({problem}), it was kept as {}",
            path.display(),
            aside.display()
        ),
        Err(e) => format!(
            "{} could not be read ({problem}) nor kept aside ({e})",
            path.display()
        ),
    }
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ww-files-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_missing_file_is_the_default_and_no_problem() {
        let dir = folder("missing");
        let (value, problem): (Vec<u32>, _) = load_json(&dir.join("nothing.json"));
        assert!(value.is_empty());
        assert_eq!(problem, None);
    }

    #[test]
    fn what_is_saved_is_read_back_and_nothing_is_left_beside_it() {
        let dir = folder("saved");
        let path = dir.join("list.json");
        save_json(&path, &vec![1u32, 2, 3]).unwrap();
        save_json(&path, &vec![4u32]).unwrap();
        assert_eq!(load_json::<Vec<u32>>(&path), (vec![4], None));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_kept_aside_not_overwritten() {
        let dir = folder("broken");
        let path = dir.join("list.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, "not json").unwrap();

        let (value, problem) = load_json::<Vec<u32>>(&path);
        assert!(value.is_empty());
        assert!(problem.is_some_and(|said| said.contains("kept as")));
        assert!(!path.exists());
        save_json(&path, &vec![7u32]).unwrap();

        let kept: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("list.json.broken-"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read_to_string(dir.join(&kept[0])).unwrap(), "not json");
        let _ = fs::remove_dir_all(dir);
    }
}
