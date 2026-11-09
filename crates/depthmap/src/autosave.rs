//! Crash and quit recovery. While a map has unsaved changes the app writes a
//! copy to a recovery file now and then (and when you quit); the next start
//! offers to restore it. Your real map file is never touched.
//!
//! The copy lives in the user's data folder (`recovery.dmap`), next to a
//! small text file (`recovery.txt`) with the map's path (or nothing for an
//! untitled map) on the first line and its name on the second.

use crate::editor::Editor;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Seconds between automatic saves while there are unsaved changes.
pub const INTERVAL_SECS: u64 = 60;

/// What a leftover recovery file holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovered {
    /// The map file it was a copy of; `None` for an untitled map.
    pub original: Option<PathBuf>,
    pub name: String,
    pub saved: Option<SystemTime>,
    pub file: PathBuf,
}

/// The default folder for recovery files.
pub fn default_dir() -> PathBuf {
    dirs::data_local_dir().or_else(dirs::home_dir).unwrap_or_default().join("depthmap")
}

fn map_file(dir: &Path) -> PathBuf {
    dir.join("recovery.dmap")
}

fn info_file(dir: &Path) -> PathBuf {
    dir.join("recovery.txt")
}

/// Writes the editor's map to the recovery files in `dir`.
pub fn save(dir: &Path, ed: &Editor) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // Write the map first; the info file marks the recovery as complete.
    let tmp = dir.join("recovery.tmp");
    depthmap_core::io::save(&ed.world, &tmp).map_err(|e| e.to_string())?;
    fs::rename(&tmp, map_file(dir)).map_err(|e| e.to_string())?;
    let original = ed.path.as_deref().map(|p| p.display().to_string()).unwrap_or_default();
    fs::write(info_file(dir), format!("{original}\n{}\n", ed.file_name())).map_err(|e| e.to_string())
}

/// A recovery file left behind, if there is a readable one.
pub fn pending(dir: &Path) -> Option<Recovered> {
    let text = fs::read_to_string(info_file(dir)).ok()?;
    let file = map_file(dir);
    let meta = fs::metadata(&file).ok()?;
    let mut lines = text.lines();
    let original = lines.next().filter(|l| !l.is_empty()).map(PathBuf::from);
    let name = lines.next().unwrap_or("Untitled").to_string();
    Some(Recovered { original, name, saved: meta.modified().ok(), file })
}

/// Removes the recovery files.
pub fn clear(dir: &Path) {
    let _ = fs::remove_file(info_file(dir));
    let _ = fs::remove_file(map_file(dir));
}

/// Tells whether something is stored.
pub fn exists(dir: &Path) -> bool {
    info_file(dir).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use depthmap_core::MapSpec;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("depthmap_autosave_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn saves_finds_restores_and_clears() {
        let dir = tmp("cycle");
        assert!(pending(&dir).is_none());
        let mut ed = Editor::with_spec(MapSpec::new(8, 8, 1));
        ed.world.set(0, 2, 3, depthmap_core::make_cell(4, 1));
        ed.world.set_tile_height(4, 123);
        save(&dir, &ed).unwrap();
        let p = pending(&dir).expect("recovery is found");
        assert_eq!(p.original, None);
        assert!(exists(&dir));

        let mut fresh = Editor::with_spec(MapSpec::new(4, 4, 1));
        fresh.restore_recovered(&p.file, p.original.clone());
        assert_eq!(fresh.world.get(0, 2, 3), depthmap_core::make_cell(4, 1));
        assert_eq!(fresh.world.tile_height(4), 123);
        assert!(fresh.modified(), "restored work counts as unsaved");
        clear(&dir);
        assert!(pending(&dir).is_none() && !exists(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remembers_the_original_file() {
        let dir = tmp("orig");
        let mut ed = Editor::with_spec(MapSpec::new(4, 4, 1));
        ed.path = Some(PathBuf::from("/maps/valley.dmap"));
        save(&dir, &ed).unwrap();
        let p = pending(&dir).unwrap();
        assert_eq!(p.original, Some(PathBuf::from("/maps/valley.dmap")));
        assert_eq!(p.name, ed.file_name());
        let mut e2 = Editor::with_spec(MapSpec::new(4, 4, 1));
        e2.restore_recovered(&p.file, p.original);
        assert_eq!(e2.path, Some(PathBuf::from("/maps/valley.dmap")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_map_file_without_its_info_is_ignored() {
        let dir = tmp("partial");
        fs::create_dir_all(&dir).unwrap();
        fs::write(map_file(&dir), b"junk").unwrap();
        assert!(pending(&dir).is_none(), "an interrupted write must not be offered");
        let _ = fs::remove_dir_all(&dir);
    }
}
