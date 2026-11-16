//! Gezgin durumu: konum, öğe listesi, seçim, pano (yapıştırma modu), arşiv içinde gezinme,
//! arama sonuçları ve bağlamsal menüler. Arayüzden bağımsızdır, birim testlidir.

use crate::archive::{self, ArchiveEntry};
use crate::error::{Error, Result};
use crate::filetype::{classify, FileClass};
use crate::listing::{list_dir, ListOptions};
use crate::search::SearchHit;
use crate::settings::Settings;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Up,
    Folder,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub kind: ItemKind,
    pub class: FileClass,
    pub size: u64,
    pub modified: Option<i64>,
    pub children: Option<usize>,
    pub hidden: bool,
    pub selected: bool,
    /// Gerçek dosya yolu (Dir ve Search konumlarında)
    pub full: Option<PathBuf>,
    /// Arşiv içi tam yol (Archive konumunda)
    pub inner: Option<String>,
    /// Arama sonuçlarında üst klasör yolu
    pub subtitle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Dir(PathBuf),
    Archive { file: PathBuf, inner: String },
    Search { query: String, hits: Vec<SearchHit>, origin: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipMode {
    Copy,
    Cut,
    Extract { archive: PathBuf, entries: Vec<String>, base: String, password: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clipboard {
    pub mode: ClipMode,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    View,
    Open,
    Install,
    ExtractHere,
    ExtractToFolder,
    ExtractTo,
    CompressZip,
    Compress7z,
    CompressDialog,
    Test,
    Copy,
    Cut,
    Delete,
    Rename,
    Info,
    AddFavorite,
    RemoveFavorite,
    Share,
    OpenAsArchive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tap {
    Navigated,
    /// Dosyaya dokunuldu: ana bağlamsal menüyü göster
    ShowMenu(usize),
    SelectionChanged,
    Nothing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LongPress {
    /// ".." öğesine uzun basış: tümünü seç/bırak
    ToggleAll,
    /// Seçili öğeden bu öğeye aralık seçildi
    Range,
    /// Ek bağlamsal menüyü göster
    ShowMenu(usize),
}

struct ArchiveState {
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
}

pub struct Browser {
    pub start: PathBuf,
    pub location: Location,
    pub items: Vec<Item>,
    pub settings: Settings,
    pub multi_select: bool,
    pub clipboard: Option<Clipboard>,
    anchor: Option<usize>,
    archive: Option<ArchiveState>,
}

fn up_item() -> Item {
    Item {
        name: "..".into(),
        kind: ItemKind::Up,
        class: FileClass::Folder,
        size: 0,
        modified: None,
        children: None,
        hidden: false,
        selected: false,
        full: None,
        inner: None,
        subtitle: None,
    }
}

impl Browser {
    pub fn new(start: PathBuf, settings: Settings) -> Self {
        Self {
            location: Location::Dir(start.clone()),
            start,
            items: Vec::new(),
            settings,
            multi_select: false,
            clipboard: None,
            anchor: None,
            archive: None,
        }
    }

    // ---------- başlık ----------

    /// Araç çubuğu başlığı: klasör adı ("0"), kökte "/".
    pub fn title(&self) -> String {
        match &self.location {
            Location::Dir(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()),
            Location::Archive { file, inner } => {
                if inner.is_empty() {
                    file.file_name().unwrap_or_default().to_string_lossy().into_owned()
                } else {
                    inner.rsplit('/').next().unwrap_or("").to_string()
                }
            }
            Location::Search { query, .. } => format!("Ara: {query}"),
        }
    }

    /// Araç çubuğu alt satırı: üst klasörün yolu ("/storage/emulated").
    pub fn subtitle(&self) -> String {
        match &self.location {
            Location::Dir(p) => p.parent().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default(),
            Location::Archive { file, inner } => {
                let f = file.to_string_lossy();
                match inner.rsplit_once('/') {
                    Some((parent, _)) => format!("{f}/{parent}"),
                    None => if inner.is_empty() { file.parent().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default() } else { f.into_owned() },
                }
            }
            Location::Search { origin, .. } => origin.to_string_lossy().into_owned(),
        }
    }

    pub fn current_dir(&self) -> Option<PathBuf> {
        match &self.location {
            Location::Dir(p) => Some(p.clone()),
            _ => None,
        }
    }

    pub fn in_archive(&self) -> bool {
        matches!(self.location, Location::Archive { .. })
    }

    pub fn is_paste_mode(&self) -> bool {
        self.clipboard.is_some()
    }

    // ---------- yenileme ----------

    pub fn refresh(&mut self) -> Result<()> {
        self.anchor = None;
        let prev_sel: Vec<String> = self.items.iter().filter(|i| i.selected).map(|i| i.name.clone()).collect();
        let loc = self.location.clone();
        let result = match loc {
            Location::Dir(p) => self.build_dir(&p),
            Location::Archive { inner, .. } => self.build_archive(&inner),
            Location::Search { hits, .. } => {
                self.build_search(&hits);
                Ok(())
            }
        };
        for it in &mut self.items {
            it.selected = prev_sel.contains(&it.name) && it.kind != ItemKind::Up;
        }
        result
    }

    fn build_dir(&mut self, p: &Path) -> Result<()> {
        let opts = ListOptions {
            show_hidden: self.settings.show_hidden,
            sort: self.settings.sort,
            descending: self.settings.descending,
            folders_first: self.settings.folders_first,
        };
        self.items.clear();
        let entries = list_dir(p, &opts).map_err(|e| match e {
            crate::listing::ListError::NotFound => Error::NotFound(p.to_string_lossy().into_owned()),
            crate::listing::ListError::PermissionDenied => Error::PermissionDenied(p.to_string_lossy().into_owned()),
            crate::listing::ListError::NotADirectory => Error::Invalid("Bu bir klasör değil".into()),
            crate::listing::ListError::Other(s) => Error::Io(s),
        });
        if p.parent().is_some() {
            self.items.push(up_item());
        }
        for e in entries? {
            let is_dir = e.kind == crate::listing::EntryKind::Dir;
            self.items.push(Item {
                class: if is_dir { FileClass::Folder } else { classify(&e.name) },
                kind: if is_dir { ItemKind::Folder } else { ItemKind::File },
                size: e.size,
                modified: e.modified,
                children: e.children,
                hidden: e.hidden,
                selected: false,
                full: Some(p.join(&e.name)),
                inner: None,
                subtitle: None,
                name: e.name,
            });
        }
        Ok(())
    }

    fn build_archive(&mut self, inner: &str) -> Result<()> {
        self.items.clear();
        self.items.push(up_item());
        let Some(st) = &self.archive else { return Err(Error::Invalid("arşiv açık değil".into())) };
        let prefix = if inner.is_empty() { String::new() } else { format!("{inner}/") };
        // ad -> (klasör mü, boyut, tarih, çocuk sayısı, tam yol)
        let mut map: BTreeMap<String, (bool, u64, Option<i64>, usize, String)> = BTreeMap::new();
        for e in &st.entries {
            let Some(rest) = e.path.strip_prefix(&prefix) else { continue };
            if rest.is_empty() {
                continue;
            }
            let (first, more) = match rest.split_once('/') {
                Some((f, _)) => (f, true),
                None => (rest, false),
            };
            let full = format!("{prefix}{first}");
            let ent = map.entry(first.to_string()).or_insert((e.is_dir || more, 0, e.modified, 0, full));
            if more || e.is_dir {
                ent.0 = true;
            }
            if more {
                ent.3 += 1;
                ent.1 += e.size;
            } else if !e.is_dir {
                ent.1 = e.size;
                ent.2 = e.modified;
            } else {
                ent.2 = e.modified.or(ent.2);
            }
        }
        let mut list: Vec<_> = map.into_iter().collect();
        list.sort_by(|a, b| (!a.1 .0).cmp(&!b.1 .0).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        for (name, (is_dir, size, modified, kids, full)) in list {
            self.items.push(Item {
                class: if is_dir { FileClass::Folder } else { classify(&name) },
                kind: if is_dir { ItemKind::Folder } else { ItemKind::File },
                size,
                modified,
                children: if is_dir { Some(kids) } else { None },
                hidden: name.starts_with('.'),
                selected: false,
                full: None,
                inner: Some(full),
                subtitle: None,
                name,
            });
        }
        Ok(())
    }

    fn build_search(&mut self, hits: &[SearchHit]) {
        self.items.clear();
        self.items.push(up_item());
        for h in hits {
            let name = h.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            self.items.push(Item {
                class: if h.is_dir { FileClass::Folder } else { classify(&name) },
                kind: if h.is_dir { ItemKind::Folder } else { ItemKind::File },
                size: h.size,
                modified: h.modified,
                children: None,
                hidden: name.starts_with('.'),
                selected: false,
                full: Some(h.path.clone()),
                inner: None,
                subtitle: h.path.parent().map(|p| p.to_string_lossy().into_owned()),
                name,
            });
        }
    }

    // ---------- gezinme ----------

    fn clear_selection_state(&mut self) {
        self.multi_select = false;
        self.anchor = None;
        for i in &mut self.items {
            i.selected = false;
        }
    }

    pub fn go_to(&mut self, dir: PathBuf) -> Result<()> {
        let old = std::mem::replace(&mut self.location, Location::Dir(dir));
        self.archive = None;
        self.clear_selection_state();
        let r = self.refresh();
        if r.is_err() {
            // gidilemediyse eski konuma dön (menüsüz hata göster)
            self.location = old;
            let _ = self.refresh();
        }
        r
    }

    /// ".." / geri düğmesi. true = bir şey değişti.
    pub fn go_up(&mut self) -> Result<bool> {
        match self.location.clone() {
            Location::Dir(p) => match p.parent() {
                Some(parent) => {
                    self.go_to(parent.to_path_buf())?;
                    Ok(true)
                }
                None => Ok(false),
            },
            Location::Archive { file, inner } => {
                self.clear_selection_state();
                if inner.is_empty() {
                    self.archive = None;
                    self.location = Location::Dir(file.parent().map(|p| p.to_path_buf()).unwrap_or(self.start.clone()));
                } else {
                    let up = inner.rsplit_once('/').map(|(a, _)| a.to_string()).unwrap_or_default();
                    self.location = Location::Archive { file, inner: up };
                }
                self.refresh()?;
                Ok(true)
            }
            Location::Search { origin, .. } => {
                self.go_to(origin)?;
                Ok(true)
            }
        }
    }

    /// Bir arşivi sanal klasör olarak açar.
    pub fn open_archive(&mut self, file: &Path, password: Option<String>) -> Result<()> {
        let entries = archive::list(file, password.as_deref())?;
        self.archive = Some(ArchiveState { entries, password });
        self.clear_selection_state();
        self.location = Location::Archive { file: file.to_path_buf(), inner: String::new() };
        self.refresh()
    }

    pub fn archive_password(&self) -> Option<String> {
        self.archive.as_ref().and_then(|a| a.password.clone())
    }

    pub fn archive_file(&self) -> Option<PathBuf> {
        match &self.location {
            Location::Archive { file, .. } => Some(file.clone()),
            _ => None,
        }
    }

    pub fn archive_inner(&self) -> String {
        match &self.location {
            Location::Archive { inner, .. } => inner.clone(),
            _ => String::new(),
        }
    }

    pub fn show_search_results(&mut self, query: &str, hits: Vec<SearchHit>) {
        let origin = match &self.location {
            Location::Dir(p) => p.clone(),
            Location::Search { origin, .. } => origin.clone(),
            Location::Archive { file, .. } => file.parent().map(|p| p.to_path_buf()).unwrap_or(self.start.clone()),
        };
        self.archive = None;
        self.clear_selection_state();
        self.location = Location::Search { query: query.to_string(), hits, origin };
        let _ = self.refresh();
    }

    // ---------- dokunma / seçim ----------

    pub fn selection_count(&self) -> usize {
        self.items.iter().filter(|i| i.selected).count()
    }

    pub fn has_selection(&self) -> bool {
        self.selection_count() > 0
    }

    pub fn selection_active(&self) -> bool {
        self.multi_select || self.has_selection()
    }

    pub fn toggle_select(&mut self, i: usize) {
        if self.is_paste_mode() {
            return; // yapıştırma modunda yeni seçim yapılamaz
        }
        if let Some(it) = self.items.get_mut(i) {
            if it.kind != ItemKind::Up {
                it.selected = !it.selected;
                self.anchor = Some(i);
            }
        }
        if !self.has_selection() {
            self.multi_select = false;
        }
    }

    /// Satıra (ikon dışına) dokunma.
    pub fn tap(&mut self, i: usize) -> Result<Tap> {
        let Some(it) = self.items.get(i).cloned() else { return Ok(Tap::Nothing) };
        match it.kind {
            ItemKind::Up => {
                if self.go_up()? {
                    Ok(Tap::Navigated)
                } else {
                    Ok(Tap::Nothing)
                }
            }
            _ if self.selection_active() && !self.is_paste_mode() => {
                self.toggle_select(i);
                Ok(Tap::SelectionChanged)
            }
            ItemKind::Folder => {
                match self.location.clone() {
                    Location::Dir(p) => self.go_to(p.join(&it.name))?,
                    Location::Archive { file, .. } => {
                        self.location = Location::Archive { file, inner: it.inner.clone().unwrap_or_default() };
                        self.clear_selection_state();
                        self.refresh()?;
                    }
                    Location::Search { .. } => self.go_to(it.full.clone().unwrap_or_default())?,
                }
                Ok(Tap::Navigated)
            }
            ItemKind::File => {
                if self.is_paste_mode() {
                    Ok(Tap::Nothing)
                } else {
                    Ok(Tap::ShowMenu(i))
                }
            }
        }
    }

    /// İkona dokunma = seç/bırak (ızgara modunda çalışmaz, arayüz karar verir).
    pub fn tap_icon(&mut self, i: usize) {
        self.toggle_select(i);
    }

    pub fn long_press(&mut self, i: usize) -> LongPress {
        let Some(it) = self.items.get(i) else { return LongPress::ShowMenu(i) };
        if it.kind == ItemKind::Up {
            if !self.is_paste_mode() {
                self.select_all_toggle();
            }
            return LongPress::ToggleAll;
        }
        if self.has_selection() && !it.selected && !self.is_paste_mode() {
            if let Some(a) = self.anchor {
                let (lo, hi) = if a <= i { (a, i) } else { (i, a) };
                for k in lo..=hi {
                    if self.items[k].kind != ItemKind::Up {
                        self.items[k].selected = true;
                    }
                }
                self.anchor = Some(i);
                return LongPress::Range;
            }
        }
        LongPress::ShowMenu(i)
    }

    pub fn select_all(&mut self) {
        if self.is_paste_mode() {
            return;
        }
        for it in &mut self.items {
            it.selected = it.kind != ItemKind::Up;
        }
    }

    pub fn select_none(&mut self) {
        for it in &mut self.items {
            it.selected = false;
        }
        self.multi_select = false;
        self.anchor = None;
    }

    pub fn select_all_toggle(&mut self) {
        let all = self.items.iter().filter(|i| i.kind != ItemKind::Up).all(|i| i.selected);
        if all {
            self.select_none();
        } else {
            self.select_all();
        }
    }

    pub fn invert_selection(&mut self) {
        if self.is_paste_mode() {
            return;
        }
        for it in &mut self.items {
            if it.kind != ItemKind::Up {
                it.selected = !it.selected;
            }
        }
    }

    pub fn set_multi_select(&mut self, on: bool) {
        if self.is_paste_mode() {
            return;
        }
        self.multi_select = on;
        if !on {
            self.select_none();
        }
    }

    // ---------- hedefler ----------

    /// İşlem hedefleri: seçim varsa seçilenler, yoksa dokunulan öğe.
    pub fn target_indices(&self, fallback: Option<usize>) -> Vec<usize> {
        let sel: Vec<usize> = self.items.iter().enumerate().filter(|(_, i)| i.selected).map(|(k, _)| k).collect();
        if !sel.is_empty() {
            sel
        } else {
            fallback.into_iter().filter(|k| self.items.get(*k).map(|i| i.kind != ItemKind::Up).unwrap_or(false)).collect()
        }
    }

    pub fn target_paths(&self, fallback: Option<usize>) -> Vec<PathBuf> {
        self.target_indices(fallback).into_iter().filter_map(|k| self.items[k].full.clone()).collect()
    }

    pub fn target_inner_paths(&self, fallback: Option<usize>) -> Vec<String> {
        self.target_indices(fallback).into_iter().filter_map(|k| self.items[k].inner.clone()).collect()
    }

    // ---------- pano ----------

    pub fn begin_copy(&mut self, fallback: Option<usize>) -> bool {
        let paths = self.target_paths(fallback);
        if paths.is_empty() {
            return false;
        }
        self.clipboard = Some(Clipboard { mode: ClipMode::Copy, paths });
        self.select_none();
        true
    }

    pub fn begin_cut(&mut self, fallback: Option<usize>) -> bool {
        let paths = self.target_paths(fallback);
        if paths.is_empty() {
            return false;
        }
        self.clipboard = Some(Clipboard { mode: ClipMode::Cut, paths });
        self.select_none();
        true
    }

    /// "Çıkar…": arşivden seçilenleri başka bir konuma çıkarmak için yapıştırma moduna geç.
    pub fn begin_extract_to(&mut self, archive_file: PathBuf, entries: Vec<String>, base: String, password: Option<String>) {
        self.clipboard = Some(Clipboard { mode: ClipMode::Extract { archive: archive_file.clone(), entries, base, password }, paths: vec![archive_file] });
        self.select_none();
    }

    pub fn cancel_paste(&mut self) {
        self.clipboard = None;
    }

    // ---------- menüler ----------

    fn is_archive_item(&self, it: &Item) -> bool {
        it.kind == ItemKind::File && (it.class == FileClass::Archive || archive::detect_by_name(&it.name).is_some()) && it.class != FileClass::Apk
    }

    /// Dosyaya dokununca açılan ana menü (ZArchiver: Görüntüle, Aç, Çıkar..., Sıkıştır...).
    pub fn main_menu(&self, i: usize) -> Vec<Action> {
        let Some(it) = self.items.get(i) else { return vec![] };
        if self.in_archive() {
            return vec![Action::ExtractHere, Action::ExtractTo, Action::Open];
        }
        if self.is_archive_item(it) {
            vec![
                Action::View,
                Action::Open,
                Action::ExtractHere,
                Action::ExtractToFolder,
                Action::ExtractTo,
                Action::Test,
                Action::CompressDialog,
            ]
        } else if it.class == FileClass::Apk {
            vec![Action::Install, Action::View, Action::Open, Action::CompressZip, Action::Compress7z, Action::CompressDialog]
        } else {
            vec![Action::Open, Action::CompressZip, Action::Compress7z, Action::CompressDialog]
        }
    }

    /// Uzun basınca açılan ek menü (kopyala, kes, sil, yeniden adlandır...).
    pub fn extra_menu(&self, i: usize) -> Vec<Action> {
        let Some(it) = self.items.get(i) else { return vec![] };
        if self.in_archive() {
            return vec![Action::ExtractHere, Action::ExtractTo];
        }
        let group = self.selection_count() > 1 || (self.has_selection() && !it.selected);
        let mut v = vec![Action::Copy, Action::Cut, Action::Delete];
        if !group {
            v.push(Action::Rename);
        }
        v.push(Action::Info);
        if !group && it.kind == ItemKind::Folder {
            let fav = it.full.as_ref().map(|p| self.settings.is_favorite(p)).unwrap_or(false);
            v.push(if fav { Action::RemoveFavorite } else { Action::AddFavorite });
        }
        if !group && it.kind == ItemKind::File {
            v.push(Action::Share);
            v.push(Action::OpenAsArchive);
        }
        v.extend([Action::CompressZip, Action::Compress7z, Action::CompressDialog]);
        v
    }

    /// Çoklu seçim varken üst çubuktaki işlem menüsü.
    pub fn selection_menu(&self) -> Vec<Action> {
        if self.in_archive() {
            return vec![Action::ExtractHere, Action::ExtractTo];
        }
        let mut v = vec![Action::Copy, Action::Cut, Action::Delete, Action::Info];
        let all_archives = self
            .items
            .iter()
            .filter(|i| i.selected)
            .all(|i| self.is_archive_item(i));
        if all_archives && self.has_selection() {
            v.extend([Action::ExtractHere, Action::ExtractToFolder, Action::ExtractTo, Action::Test]);
        }
        v.extend([Action::CompressZip, Action::Compress7z, Action::CompressDialog]);
        v
    }

    // ---------- ayarlar ----------

    pub fn set_sort(&mut self, key: crate::listing::SortKey, descending: bool) -> Result<()> {
        self.settings.sort = key;
        self.settings.descending = descending;
        self.refresh()
    }

    pub fn toggle_hidden(&mut self) -> Result<()> {
        self.settings.show_hidden = !self.settings.show_hidden;
        self.refresh()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{compress, CompressOptions};
    use crate::job::Ctx;
    use std::fs::{self, create_dir_all, write};

    fn setup(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rarch_br_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        create_dir_all(d.join("Download/Alt")).unwrap();
        create_dir_all(d.join(".gizli")).unwrap();
        create_dir_all(d.join("Bos")).unwrap();
        write(d.join("a.txt"), b"a").unwrap();
        write(d.join("b.mp3"), b"b").unwrap();
        write(d.join("Download/x.zip.txt"), b"x").unwrap();
        d
    }

    #[test]
    fn navigation_and_titles() {
        let d = setup("nav");
        let mut b = Browser::new(d.clone(), Settings::default());
        b.refresh().unwrap();
        // ".." her zaman ilk, gizli klasör varsayılan görünür
        assert_eq!(b.items[0].kind, ItemKind::Up);
        let names: Vec<_> = b.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["..", ".gizli", "Bos", "Download", "a.txt", "b.mp3"]);
        assert_eq!(b.title(), d.file_name().unwrap().to_string_lossy());
        assert_eq!(b.subtitle(), d.parent().unwrap().to_string_lossy());

        let idx = b.items.iter().position(|i| i.name == "Download").unwrap();
        assert_eq!(b.tap(idx).unwrap(), Tap::Navigated);
        assert_eq!(b.title(), "Download");
        assert!(b.go_up().unwrap());
        assert_eq!(b.current_dir().unwrap(), d);

        // dosyaya dokunma -> ana menü
        let f = b.items.iter().position(|i| i.name == "a.txt").unwrap();
        assert_eq!(b.tap(f).unwrap(), Tap::ShowMenu(f));
        assert!(b.main_menu(f).contains(&Action::Open));

        // olmayan klasöre gidilemez, eski konumda kalır
        assert!(b.go_to(d.join("yok")).is_err());
        assert_eq!(b.current_dir().unwrap(), d);

        b.toggle_hidden().unwrap();
        assert!(b.items.iter().all(|i| i.name != ".gizli"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn selection_rules() {
        let d = setup("sel");
        let mut b = Browser::new(d.clone(), Settings::default());
        b.refresh().unwrap();
        // ikon: seç
        b.tap_icon(2);
        assert_eq!(b.selection_count(), 1);
        // seçim varken satıra dokunma = seç/bırak
        assert_eq!(b.tap(3).unwrap(), Tap::SelectionChanged);
        assert_eq!(b.selection_count(), 2);
        // ilk seçiliden son öğeye uzun bas = aralık
        b.select_none();
        b.tap_icon(1);
        assert_eq!(b.long_press(4), LongPress::Range);
        assert_eq!(b.selection_count(), 4);
        // ".." uzun basış: tümünü seç/bırak
        assert_eq!(b.long_press(0), LongPress::ToggleAll); // kısmi seçim -> hepsini seç
        assert_eq!(b.selection_count(), b.items.len() - 1);
        assert_eq!(b.long_press(0), LongPress::ToggleAll); // hepsi seçili -> bırak
        assert_eq!(b.selection_count(), 0);
        b.select_all();
        b.invert_selection();
        assert_eq!(b.selection_count(), 0);
        // seçim yokken uzun bas: ek menü
        assert_eq!(b.long_press(2), LongPress::ShowMenu(2));
        assert!(b.extra_menu(2).contains(&Action::Rename));
        b.select_all();
        assert!(!b.extra_menu(2).contains(&Action::Rename)); // grup: yeniden adlandırma yok
        // ".." seçilemez
        b.select_none();
        b.tap_icon(0);
        assert_eq!(b.selection_count(), 0);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn paste_mode() {
        let d = setup("paste");
        let mut b = Browser::new(d.clone(), Settings::default());
        b.refresh().unwrap();
        let f = b.items.iter().position(|i| i.name == "a.txt").unwrap();
        b.tap_icon(f);
        assert!(b.begin_copy(None));
        assert!(b.is_paste_mode());
        assert_eq!(b.clipboard.as_ref().unwrap().paths, vec![d.join("a.txt")]);
        assert_eq!(b.selection_count(), 0);
        // yapıştırma modunda seçim yapılamaz ama gezinilebilir
        b.tap_icon(f);
        assert_eq!(b.selection_count(), 0);
        let dl = b.items.iter().position(|i| i.name == "Download").unwrap();
        assert_eq!(b.tap(dl).unwrap(), Tap::Navigated);
        assert!(b.is_paste_mode());
        b.cancel_paste();
        assert!(!b.is_paste_mode());
        // seçim yokken uzun basılan öğe hedef olur
        assert!(b.begin_cut(Some(1)));
        assert!(matches!(b.clipboard.as_ref().unwrap().mode, ClipMode::Cut));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn archive_browsing() {
        let d = setup("arc");
        let out = d.join("paket.zip");
        compress(&[d.join("Download")], &out, &CompressOptions::default(), &Ctx::new()).unwrap();
        let mut b = Browser::new(d.clone(), Settings::default());
        b.refresh().unwrap();
        let z = b.items.iter().position(|i| i.name == "paket.zip").unwrap();
        assert!(b.main_menu(z).contains(&Action::ExtractToFolder));
        b.open_archive(&out, None).unwrap();
        assert!(b.in_archive());
        assert_eq!(b.title(), "paket.zip");
        let names: Vec<_> = b.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["..", "Download"]);
        assert_eq!(b.tap(1).unwrap(), Tap::Navigated);
        assert_eq!(b.archive_inner(), "Download");
        let names: Vec<_> = b.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["..", "Alt", "x.zip.txt"]);
        // arşiv içi menü
        assert!(b.main_menu(2).contains(&Action::ExtractHere));
        assert_eq!(b.target_inner_paths(Some(2)), vec!["Download/x.zip.txt".to_string()]);
        // ".." ile yukarı, sonra arşivi kapat
        assert!(b.go_up().unwrap());
        assert_eq!(b.archive_inner(), "");
        assert!(b.go_up().unwrap());
        assert_eq!(b.current_dir().unwrap(), d);
        assert!(!b.in_archive());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn search_results_location() {
        let d = setup("search");
        let mut b = Browser::new(d.clone(), Settings::default());
        b.refresh().unwrap();
        let hit = SearchHit { path: d.join("Download/x.zip.txt"), is_dir: false, size: 1, modified: None };
        b.show_search_results("*.txt", vec![hit]);
        assert_eq!(b.title(), "Ara: *.txt");
        assert_eq!(b.items.len(), 2);
        assert_eq!(b.items[1].subtitle.as_deref(), Some(d.join("Download").to_string_lossy().as_ref()));
        assert!(b.go_up().unwrap());
        assert_eq!(b.current_dir().unwrap(), d);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn menus_for_types() {
        let d = setup("menu");
        write(d.join("k.apk"), b"PK").unwrap();
        let mut b = Browser::new(d.clone(), Settings::default());
        b.refresh().unwrap();
        let apk = b.items.iter().position(|i| i.name == "k.apk").unwrap();
        assert!(b.main_menu(apk).contains(&Action::Install));
        let folder = b.items.iter().position(|i| i.name == "Download").unwrap();
        assert!(b.extra_menu(folder).contains(&Action::AddFavorite));
        b.settings.add_favorite(&d.join("Download"));
        assert!(b.extra_menu(folder).contains(&Action::RemoveFavorite));
        let file = b.items.iter().position(|i| i.name == "a.txt").unwrap();
        assert!(b.extra_menu(file).contains(&Action::OpenAsArchive));
        b.tap_icon(folder);
        assert!(b.selection_menu().contains(&Action::Delete));
        let _ = fs::remove_dir_all(&d);
    }
}
