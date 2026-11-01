//! Saved colour palettes. An entry stores a colour *and* the texture that was
//! selected with it, so a palette doubles as a small tileset.
//!
//! Palettes belong to the user, not to a map: they live in one text file in
//! the config folder (`.../depthmap/palettes.txt`):
//!
//! ```text
//! depthmap-palettes 1
//! palette Dungeon
//! 7a5a3e 4
//! 969690 1
//! ```

use std::fs;
use std::path::PathBuf;

use depthmap_core::colors::DEFAULT_COLORS;
use depthmap_core::texture::MAX_TEXTURES;

pub const MAX_NAME_CHARS: usize = 40;
pub const MAX_ENTRIES: usize = 256;
pub const MAX_PALETTES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub rgb: [u8; 3],
    pub texture: u8,
}

impl Entry {
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.rgb[0], self.rgb[1], self.rgb[2])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub name: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
pub struct Palettes {
    /// Where the palettes are stored; `None` keeps them in memory only.
    file: Option<PathBuf>,
    list: Vec<Palette>,
    current: usize,
}

/// A name the file format can hold: no control characters, trimmed, limited.
pub fn clean_name(name: &str) -> Option<String> {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// `#rrggbb`, `rrggbb` or `#rgb` to a colour.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().trim_start_matches('#');
    if !t.is_ascii() {
        return None;
    }
    let byte = |s: &str| u8::from_str_radix(s, 16).ok();
    match t.len() {
        6 => Some([byte(&t[0..2])?, byte(&t[2..4])?, byte(&t[4..6])?]),
        3 => {
            let d = |i: usize| byte(&t[i..i + 1]).map(|v| v * 17);
            Some([d(0)?, d(1)?, d(2)?])
        }
        _ => None,
    }
}

impl Palettes {
    /// The starter palette only, nothing stored.
    pub fn in_memory() -> Self {
        Self::with_file(None)
    }

    fn with_file(file: Option<PathBuf>) -> Self {
        Self { file, list: vec![Self::starter()], current: 0 }
    }

    fn starter() -> Palette {
        Palette {
            name: "Basics".into(),
            entries: DEFAULT_COLORS.iter().map(|&rgb| Entry { rgb, texture: 2 }).collect(),
        }
    }

    pub fn default_file() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("depthmap").join("palettes.txt"))
    }

    /// The palettes in the user's config folder.
    pub fn load() -> Self {
        Self::load_from(Self::default_file())
    }

    /// Reads `file` (a missing or broken file gives the starter palette).
    /// Changes are written back to it.
    pub fn load_from(file: Option<PathBuf>) -> Self {
        let mut p = Self::with_file(file.clone());
        let Some(text) = file.and_then(|f| fs::read_to_string(f).ok()) else {
            return p;
        };
        let parsed = Self::parse(&text);
        if !parsed.is_empty() {
            p.list = parsed;
        }
        p
    }

    fn parse(text: &str) -> Vec<Palette> {
        let mut out: Vec<Palette> = Vec::new();
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("palette ") {
                if out.len() < MAX_PALETTES {
                    if let Some(name) = clean_name(name) {
                        out.push(Palette { name, entries: Vec::new() });
                    }
                }
            } else if let Some(p) = out.last_mut() {
                let mut it = line.split_whitespace();
                let (Some(c), Some(t)) = (it.next(), it.next()) else { continue };
                let (Some(rgb), Ok(texture)) = (parse_hex(c), t.parse::<u8>()) else { continue };
                if (1..=MAX_TEXTURES).contains(&texture) && p.entries.len() < MAX_ENTRIES {
                    p.entries.push(Entry { rgb, texture });
                }
            }
        }
        out
    }

    fn text(&self) -> String {
        let mut s = String::from("depthmap-palettes 1\n");
        for p in &self.list {
            s.push_str(&format!("palette {}\n", p.name));
            for e in &p.entries {
                s.push_str(&format!("{:02x}{:02x}{:02x} {}\n", e.rgb[0], e.rgb[1], e.rgb[2], e.texture));
            }
        }
        s
    }

    /// Writes the file. Losing palettes would be annoying but not fatal, so
    /// callers may ignore the error; the UI shows it in the status line.
    pub fn save(&self) -> Result<(), String> {
        let Some(file) = &self.file else { return Ok(()) };
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        fs::write(file, self.text()).map_err(|e| format!("cannot save palettes: {e}"))
    }

    pub fn list(&self) -> &[Palette] {
        &self.list
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn current(&self) -> &Palette {
        &self.list[self.current]
    }

    pub fn select(&mut self, index: usize) {
        if index < self.list.len() {
            self.current = index;
        }
    }

    fn unique_name(&self, wanted: &str) -> String {
        if !self.list.iter().any(|p| p.name == wanted) {
            return wanted.to_string();
        }
        (2..)
            .map(|n| format!("{wanted} {n}"))
            .find(|n| !self.list.iter().any(|p| &p.name == n))
            .unwrap()
    }

    /// Adds an empty palette and makes it the current one.
    pub fn create(&mut self) -> Option<usize> {
        if self.list.len() >= MAX_PALETTES {
            return None;
        }
        let name = self.unique_name("New palette");
        self.list.push(Palette { name, entries: Vec::new() });
        self.current = self.list.len() - 1;
        Some(self.current)
    }

    /// Renames a palette; false if the name is unusable.
    pub fn rename(&mut self, index: usize, name: &str) -> bool {
        let Some(name) = clean_name(name) else { return false };
        match self.list.get_mut(index) {
            Some(p) => {
                p.name = name;
                true
            }
            None => false,
        }
    }

    /// Deletes a palette. The last one is emptied instead of removed.
    pub fn delete(&mut self, index: usize) {
        if index >= self.list.len() {
            return;
        }
        if self.list.len() == 1 {
            self.list[0].entries.clear();
            return;
        }
        self.list.remove(index);
        self.current = self.current.min(self.list.len() - 1);
        if self.current > index {
            self.current -= 1;
        }
    }

    /// Adds a colour with its texture to a palette. False if it is already
    /// there or the palette is full.
    pub fn add_entry(&mut self, index: usize, entry: Entry) -> bool {
        match self.list.get_mut(index) {
            Some(p) if p.entries.len() < MAX_ENTRIES && !p.entries.contains(&entry) => {
                p.entries.push(entry);
                true
            }
            _ => false,
        }
    }

    pub fn remove_entry(&mut self, index: usize, entry: usize) {
        if let Some(p) = self.list.get_mut(index) {
            if entry < p.entries.len() {
                p.entries.remove(entry);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("depthmap-pal-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn hex_parsing_accepts_common_spellings_and_rejects_junk() {
        assert_eq!(parse_hex("#ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("FF8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("  #f80 "), Some([255, 136, 0]));
        assert_eq!(parse_hex("#ff80"), None);
        assert_eq!(parse_hex("#gg0000"), None);
        assert_eq!(parse_hex("é12345"), None);
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  Dungeon \n"), Some("Dungeon".into()));
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name(&"x".repeat(100)).unwrap().chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn creating_renaming_and_deleting() {
        let mut p = Palettes::in_memory();
        assert_eq!(p.list().len(), 1);
        let a = p.create().unwrap();
        let b = p.create().unwrap();
        assert_eq!(p.current_index(), b);
        assert_ne!(p.list()[a].name, p.list()[b].name, "names stay distinct");
        assert!(p.rename(b, "Forest"));
        assert!(!p.rename(b, "  "));
        assert_eq!(p.current().name, "Forest");
        p.delete(a);
        assert_eq!(p.list().len(), 2);
        assert_eq!(p.current().name, "Forest", "the current palette follows its index");
        p.delete(1);
        p.delete(0);
        assert_eq!(p.list().len(), 1, "the last palette is emptied, never removed");
        assert!(p.current().entries.is_empty());
    }

    #[test]
    fn entries_keep_colour_and_texture_and_skip_duplicates() {
        let mut p = Palettes::in_memory();
        let i = p.create().unwrap();
        let e = Entry { rgb: [1, 2, 3], texture: 4 };
        assert!(p.add_entry(i, e));
        assert!(!p.add_entry(i, e));
        assert!(p.add_entry(i, Entry { rgb: [1, 2, 3], texture: 5 }), "same colour, other tile");
        p.remove_entry(i, 0);
        assert_eq!(p.list()[i].entries, vec![Entry { rgb: [1, 2, 3], texture: 5 }]);
    }

    #[test]
    fn palettes_survive_a_restart_and_garbage_is_ignored() {
        let dir = temp("persist");
        let file = dir.join("palettes.txt");
        let mut p = Palettes::load_from(Some(file.clone()));
        let i = p.create().unwrap();
        p.rename(i, "My tiles").then_some(()).unwrap();
        p.add_entry(i, Entry { rgb: [10, 20, 30], texture: 7 });
        p.save().unwrap();

        let again = Palettes::load_from(Some(file.clone()));
        assert_eq!(again.list(), p.list());

        fs::write(&file, "nonsense\npalette Ok\nzzzzzz 3\n112233 99\n112233 3\n445566\n").unwrap();
        let g = Palettes::load_from(Some(file));
        assert_eq!(g.list().len(), 1);
        assert_eq!(g.list()[0].entries, vec![Entry { rgb: [0x11, 0x22, 0x33], texture: 3 }]);

        let missing = Palettes::load_from(Some(dir.join("none.txt")));
        assert_eq!(missing.current().name, "Basics");
        let _ = fs::remove_dir_all(dir);
    }
}
