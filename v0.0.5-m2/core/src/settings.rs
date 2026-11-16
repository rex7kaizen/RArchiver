//! Kalıcı ayarlar ve sık kullanılanlar (basit anahtar=değer metin dosyası).

use crate::listing::SortKey;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    List,
    Grid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub show_hidden: bool,
    pub sort: SortKey,
    pub descending: bool,
    pub folders_first: bool,
    pub view: ViewMode,
    /// 0 RArchiver Koyu Mor, 1 RArchiver Açık Lavanta, 2 Klasik (ZArchiver renkleri)
    pub theme: u8,
    /// 0 küçük, 1 normal, 2 büyük
    pub row_size: u8,
    pub show_time: bool,
    pub favorites: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_hidden: true, // Karar: gizli klasörler varsayılan görünür
            sort: SortKey::Name,
            descending: false,
            folders_first: true,
            view: ViewMode::List,
            theme: 0,
            row_size: 1,
            show_time: true,
            favorites: Vec::new(),
        }
    }
}

fn sort_name(k: SortKey) -> &'static str {
    match k {
        SortKey::Name => "name",
        SortKey::Modified => "modified",
        SortKey::Size => "size",
    }
}

impl Settings {
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let b = |v: bool| if v { "1" } else { "0" };
        s += &format!("show_hidden={}\n", b(self.show_hidden));
        s += &format!("sort={}\n", sort_name(self.sort));
        s += &format!("descending={}\n", b(self.descending));
        s += &format!("folders_first={}\n", b(self.folders_first));
        s += &format!("view={}\n", if self.view == ViewMode::Grid { "grid" } else { "list" });
        s += &format!("theme={}\n", self.theme);
        s += &format!("row_size={}\n", self.row_size);
        s += &format!("show_time={}\n", b(self.show_time));
        for f in &self.favorites {
            s += &format!("fav={}\n", f.to_string_lossy());
        }
        s
    }

    pub fn from_text(text: &str) -> Self {
        let mut st = Settings::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let on = v == "1";
            match k {
                "show_hidden" => st.show_hidden = on,
                "sort" => {
                    st.sort = match v {
                        "modified" => SortKey::Modified,
                        "size" => SortKey::Size,
                        _ => SortKey::Name,
                    }
                }
                "descending" => st.descending = on,
                "folders_first" => st.folders_first = on,
                "view" => st.view = if v == "grid" { ViewMode::Grid } else { ViewMode::List },
                "theme" => st.theme = v.parse::<u8>().unwrap_or(0).min(2),
                "row_size" => st.row_size = v.parse::<u8>().unwrap_or(1).min(2),
                "show_time" => st.show_time = on,
                "fav" if !v.is_empty() => st.favorites.push(PathBuf::from(v)),
                _ => {}
            }
        }
        st
    }

    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path).map(|t| Self::from_text(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(path, self.to_text())
    }

    pub fn is_favorite(&self, p: &Path) -> bool {
        self.favorites.iter().any(|f| f == p)
    }

    pub fn add_favorite(&mut self, p: &Path) {
        if !self.is_favorite(p) {
            self.favorites.push(p.to_path_buf());
        }
    }

    pub fn remove_favorite(&mut self, p: &Path) {
        self.favorites.retain(|f| f != p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut s = Settings::default();
        s.theme = 2;
        s.sort = SortKey::Size;
        s.descending = true;
        s.view = ViewMode::Grid;
        s.row_size = 2;
        s.add_favorite(Path::new("/storage/emulated/0/DCIM"));
        s.add_favorite(Path::new("/storage/emulated/0/DCIM"));
        assert_eq!(s.favorites.len(), 1);
        let t = s.to_text();
        assert_eq!(Settings::from_text(&t), s);
        s.remove_favorite(Path::new("/storage/emulated/0/DCIM"));
        assert!(s.favorites.is_empty());
        assert_eq!(Settings::from_text(""), Settings::default());
    }
}
