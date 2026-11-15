//! RArchiver çekirdeği: Android'den bağımsız, saf Rust.
//! M1 kapsamı: dizin listeleme, sıralama, biçimlendirme.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Unix zamanı (saniye). Okunamazsa None.
    pub modified: Option<i64>,
    /// Yalnızca klasörler için doğrudan çocuk sayısı. Okunamazsa None.
    pub children: Option<usize>,
    pub hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Modified,
    Size,
}

#[derive(Debug, Clone, Copy)]
pub struct ListOptions {
    pub show_hidden: bool,
    pub sort: SortKey,
    pub descending: bool,
    pub folders_first: bool,
}

impl Default for ListOptions {
    fn default() -> Self {
        // Karar: gizli (noktalı) klasörler varsayılan olarak görünür.
        Self { show_hidden: true, sort: SortKey::Name, descending: false, folders_first: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListError {
    NotFound,
    PermissionDenied,
    NotADirectory,
    Other(String),
}

impl From<std::io::Error> for ListError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            ErrorKind::NotFound => ListError::NotFound,
            ErrorKind::PermissionDenied => ListError::PermissionDenied,
            _ => ListError::Other(e.to_string()),
        }
    }
}

/// Klasördeki doğrudan çocuk sayısı (okunamazsa None).
pub fn child_count(path: &Path) -> Option<usize> {
    fs::read_dir(path).ok().map(|rd| rd.filter(|e| e.is_ok()).count())
}

fn to_unix(t: std::time::SystemTime) -> Option<i64> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => Some(d.as_secs() as i64),
        Err(e) => Some(-(e.duration().as_secs() as i64)),
    }
}

/// Bir klasörü listeler.
pub fn list_dir(path: &Path, opts: &ListOptions) -> Result<Vec<Entry>, ListError> {
    let md = fs::metadata(path)?;
    if !md.is_dir() {
        return Err(ListError::NotADirectory);
    }
    let mut out = Vec::new();
    for item in fs::read_dir(path)? {
        let item = match item {
            Ok(i) => i,
            Err(_) => continue,
        };
        let name = item.file_name().to_string_lossy().into_owned();
        let hidden = name.starts_with('.');
        if hidden && !opts.show_hidden {
            continue;
        }
        let full = item.path();
        let meta = fs::metadata(&full).or_else(|_| fs::symlink_metadata(&full));
        let (kind, size, modified) = match &meta {
            Ok(m) => (
                if m.is_dir() { EntryKind::Dir } else { EntryKind::File },
                if m.is_dir() { 0 } else { m.len() },
                m.modified().ok().and_then(to_unix),
            ),
            Err(_) => (EntryKind::File, 0, None),
        };
        let children = if kind == EntryKind::Dir { child_count(&full) } else { None };
        out.push(Entry { name, kind, size, modified, children, hidden });
    }
    sort_entries(&mut out, opts);
    Ok(out)
}

pub fn sort_entries(entries: &mut [Entry], opts: &ListOptions) {
    entries.sort_by(|a, b| {
        use std::cmp::Ordering;
        if opts.folders_first {
            let ad = a.kind == EntryKind::Dir;
            let bd = b.kind == EntryKind::Dir;
            if ad != bd {
                return if ad { Ordering::Less } else { Ordering::Greater };
            }
        }
        let ord = match opts.sort {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Size => a.size.cmp(&b.size),
        }
        .then_with(|| a.name.cmp(&b.name));
        if opts.descending { ord.reverse() } else { ord }
    });
}

/// `cur` kökün (`root`) altındaysa ve köke eşit değilse üst klasörü verir.
pub fn parent_within(cur: &Path, root: &Path) -> Option<PathBuf> {
    if cur == root || !cur.starts_with(root) {
        return None;
    }
    cur.parent().map(|p| p.to_path_buf())
}

/// 1024 tabanlı okunur boyut: "512 B", "1.5 KB", "3.0 MB" ...
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1} {}", v, UNITS[i])
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + (m <= 2) as i64, m, d)
}

/// Unix saniyesini "gg.aa.yy" (UTC) biçimine çevirir. Yerel saat dilimi M2'de.
pub fn format_date(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{:02}.{:02}.{:02}", d, m, y.rem_euclid(100))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, write};

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("rarchiver_test_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn size_format() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn date_format() {
        assert_eq!(format_date(0), "01.01.70");
        assert_eq!(format_date(86_400 * 365), "01.01.71");
        assert_eq!(format_date(951_782_400), "29.02.00");
        assert_eq!(format_date(1_794_528_000), "13.11.26");
    }

    #[test]
    fn list_sorts_folders_first_and_shows_hidden() {
        let d = tmp("list");
        create_dir_all(d.join("Download")).unwrap();
        create_dir_all(d.join(".dlprovider")).unwrap();
        create_dir_all(d.join("alarms")).unwrap();
        write(d.join("b.txt"), b"hello").unwrap();
        write(d.join("A.txt"), b"x").unwrap();
        write(d.join("Download").join("f"), b"1").unwrap();

        let v = list_dir(&d, &ListOptions::default()).unwrap();
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, [".dlprovider", "alarms", "Download", "A.txt", "b.txt"]);
        assert!(v[0].hidden);
        assert_eq!(v[0].children, Some(0));
        assert_eq!(v[2].children, Some(1));
        assert_eq!(v[4].size, 5);

        let opts = ListOptions { show_hidden: false, ..Default::default() };
        let v = list_dir(&d, &opts).unwrap();
        assert!(v.iter().all(|e| !e.hidden));
        assert_eq!(v.len(), 4);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn sort_by_size_descending() {
        let d = tmp("size");
        write(d.join("a"), b"1").unwrap();
        write(d.join("b"), b"123456").unwrap();
        write(d.join("c"), b"123").unwrap();
        let opts = ListOptions { sort: SortKey::Size, descending: true, ..Default::default() };
        let v = list_dir(&d, &opts).unwrap();
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["b", "c", "a"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn errors() {
        let d = tmp("err");
        assert_eq!(list_dir(&d.join("yok"), &ListOptions::default()), Err(ListError::NotFound));
        write(d.join("f"), b"x").unwrap();
        assert_eq!(list_dir(&d.join("f"), &ListOptions::default()), Err(ListError::NotADirectory));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn parent_rules() {
        let root = Path::new("/storage/emulated/0");
        assert_eq!(parent_within(root, root), None);
        assert_eq!(
            parent_within(Path::new("/storage/emulated/0/DCIM"), root),
            Some(PathBuf::from("/storage/emulated/0"))
        );
        assert_eq!(parent_within(Path::new("/sdcard2"), root), None);
    }
}
