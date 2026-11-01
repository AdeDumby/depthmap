//! Remembers which maps were opened recently, so the app can offer to pick up
//! where you left off.
//!
//! The list lives in a plain text file, one path per line, most recent first:
//!
//! * Windows: `%APPDATA%\depthmap\recent.txt`
//! * macOS:   `~/Library/Application Support/depthmap/recent.txt`
//! * Linux:   `~/.config/depthmap/recent.txt`
//!
//! Maps themselves are never copied there; they stay wherever you saved them.

use std::fs;
use std::path::{Path, PathBuf};

/// Where imported node icons are kept: `Documents/Depthmap/Custom Icons`.
pub fn custom_icons_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
        .join("Depthmap")
        .join("Custom Icons")
}

/// Where the "import PNG" dialog starts.
pub fn icon_import_start_dir() -> Option<PathBuf> {
    dirs::picture_dir().or_else(dirs::document_dir)
}

/// How many maps are remembered.
pub const MAX_RECENT: usize = 10;

#[derive(Debug, Clone)]
pub struct Recent {
    /// Where the list is stored; `None` keeps it in memory only.
    file: Option<PathBuf>,
    paths: Vec<PathBuf>,
}

impl Recent {
    /// The list in the user's config folder.
    pub fn load() -> Self {
        Self::load_from(Self::default_file())
    }

    /// The list stored in `file` (`None` = in memory only). A missing or
    /// unreadable file is just an empty list.
    pub fn load_from(file: Option<PathBuf>) -> Self {
        let paths = file
            .as_deref()
            .and_then(|f| fs::read_to_string(f).ok())
            .map(|text| {
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        let mut recent = Self { file, paths };
        recent.paths.truncate(MAX_RECENT);
        recent
    }

    pub fn default_file() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("depthmap").join("recent.txt"))
    }

    /// Where maps are suggested to live when nothing else is known:
    /// `Documents/depthmap` (created on first save).
    pub fn default_maps_dir() -> Option<PathBuf> {
        dirs::document_dir().map(|d| d.join("depthmap"))
    }

    /// Most recent first.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// The most recently used map that still exists.
    pub fn last_existing(&self) -> Option<&Path> {
        self.paths.iter().map(PathBuf::as_path).find(|p| p.is_file())
    }

    /// Folder the file dialogs should start in: where the last map lives,
    /// otherwise the default maps folder if it exists.
    pub fn dialog_dir(&self) -> Option<PathBuf> {
        self.paths
            .iter()
            .filter_map(|p| p.parent())
            .find(|d| d.is_dir())
            .map(Path::to_path_buf)
            .or_else(|| Self::default_maps_dir().filter(|d| d.is_dir()))
    }

    /// Marks `path` as the most recently used map and saves the list.
    pub fn push(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(MAX_RECENT);
        self.save();
    }

    /// Forgets a map (e.g. one that was deleted).
    pub fn remove(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
        self.save();
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let text: String = self
            .paths
            .iter()
            .map(|p| format!("{}\n", p.display()))
            .collect();
        // Losing the recent list is harmless, so a failed write is ignored.
        let _ = fs::write(file, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("depthmap-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn most_recent_first_without_duplicates() {
        let mut r = Recent::load_from(None);
        r.push(Path::new("/maps/a.dmap"));
        r.push(Path::new("/maps/b.dmap"));
        r.push(Path::new("/maps/a.dmap"));
        let names: Vec<_> = r.paths().iter().map(|p| p.display().to_string()).collect();
        assert_eq!(names, ["/maps/a.dmap", "/maps/b.dmap"]);
    }

    #[test]
    fn list_is_capped() {
        let mut r = Recent::load_from(None);
        for i in 0..25 {
            r.push(Path::new(&format!("/maps/{i}.dmap")));
        }
        assert_eq!(r.paths().len(), MAX_RECENT);
        assert_eq!(r.paths()[0], Path::new("/maps/24.dmap"));
    }

    #[test]
    fn survives_a_restart() {
        let dir = temp_dir("recent");
        let file = dir.join("sub").join("recent.txt"); // folder is created on demand
        let mut r = Recent::load_from(Some(file.clone()));
        r.push(Path::new("/maps/one.dmap"));
        r.push(Path::new("/maps/two words.dmap"));

        let again = Recent::load_from(Some(file));
        assert_eq!(again.paths(), r.paths());
        assert_eq!(again.paths()[0], Path::new("/maps/two words.dmap"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_or_garbage_files_give_an_empty_list() {
        let dir = temp_dir("recent-bad");
        assert!(Recent::load_from(Some(dir.join("nope.txt"))).paths().is_empty());
        let garbage = dir.join("g.txt");
        fs::write(&garbage, "\n   \n\n").unwrap();
        assert!(Recent::load_from(Some(garbage)).paths().is_empty());
        assert!(Recent::load_from(None).paths().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn last_existing_skips_deleted_maps_and_dialog_dir_follows_the_last_map() {
        let dir = temp_dir("recent-exist");
        let real = dir.join("real.dmap");
        fs::write(&real, b"x").unwrap();
        let mut r = Recent::load_from(None);
        r.push(&real);
        r.push(&dir.join("deleted.dmap")); // most recent, but gone
        assert_eq!(r.last_existing(), Some(real.as_path()));
        assert_eq!(r.dialog_dir().as_deref(), Some(dir.as_path()));

        r.remove(&real);
        assert_eq!(r.last_existing(), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
