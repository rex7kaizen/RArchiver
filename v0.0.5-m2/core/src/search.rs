//! Arama: `*` / `?` joker karakterleri ve tür filtreleri (ZArchiver benzeri).

use crate::error::Result;
use crate::filetype::{classify, FileClass};
use crate::job::Ctx;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub pattern: String,
    /// Boşsa tür filtresi yok. Dolu ise yalnızca bu türlerdeki dosyalar eşleşir.
    pub classes: Vec<FileClass>,
    pub max_results: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<i64>,
}

/// Büyük/küçük harf duyarsız joker eşleme. `*` herhangi bir dizi, `?` tek karakter.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn name_matches(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    if pattern.contains('*') || pattern.contains('?') {
        glob_match(pattern, name)
    } else {
        name.to_lowercase().contains(&pattern.to_lowercase())
    }
}

/// `root` altında özyinelemeli arar. Okunamayan klasörler sessizce atlanır.
pub fn search(root: &Path, q: &SearchQuery, ctx: &Ctx, on_hit: &mut dyn FnMut(SearchHit)) -> Result<usize> {
    let max = if q.max_results == 0 { usize::MAX } else { q.max_results };
    let mut found = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        ctx.check()?;
        ctx.progress.set_current(&dir.to_string_lossy());
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut subdirs = Vec::new();
        for item in rd.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            let Ok(ft) = item.file_type() else { continue };
            let is_dir = ft.is_dir();
            if is_dir {
                subdirs.push(item.path());
            }
            // Tür filtresi varsa klasörler eşleşmez.
            if !q.classes.is_empty() && (is_dir || !q.classes.contains(&classify(&name))) {
                continue;
            }
            if !name_matches(&q.pattern, &name) {
                continue;
            }
            let md = item.metadata().ok();
            on_hit(SearchHit {
                path: item.path(),
                is_dir,
                size: md.as_ref().map(|m| if m.is_dir() { 0 } else { m.len() }).unwrap_or(0),
                modified: md
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64),
            });
            found += 1;
            if found >= max {
                return Ok(found);
            }
        }
        // Sıralı gezinti için ters ekle
        subdirs.reverse();
        stack.extend(subdirs);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, write};

    #[test]
    fn globbing() {
        assert!(glob_match("*.zip", "Backup.ZIP"));
        assert!(!glob_match("*.zip", "a.zip.txt"));
        assert!(glob_match("a?c", "abc"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
    }

    #[test]
    fn finds_files() {
        let d = std::env::temp_dir().join(format!("rarch_search_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        create_dir_all(d.join("a/b")).unwrap();
        write(d.join("x.zip"), b"1").unwrap();
        write(d.join("a/y.ZIP"), b"1").unwrap();
        write(d.join("a/b/photo.jpg"), b"1").unwrap();
        write(d.join("a/b/notes.txt"), b"1").unwrap();
        let ctx = Ctx::new();

        let mut hits = Vec::new();
        let q = SearchQuery { pattern: "*.zip".into(), ..Default::default() };
        let n = search(&d, &q, &ctx, &mut |h| hits.push(h)).unwrap();
        assert_eq!(n, 2);

        hits.clear();
        let q = SearchQuery { pattern: "".into(), classes: vec![FileClass::Image], max_results: 0 };
        search(&d, &q, &ctx, &mut |h| hits.push(h)).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("photo.jpg"));

        hits.clear();
        let q = SearchQuery { pattern: "note".into(), ..Default::default() };
        search(&d, &q, &ctx, &mut |h| hits.push(h)).unwrap();
        assert_eq!(hits.len(), 1);

        ctx.cancel.cancel();
        assert!(search(&d, &q, &ctx, &mut |_| {}).is_err());
        let _ = fs::remove_dir_all(&d);
    }
}
