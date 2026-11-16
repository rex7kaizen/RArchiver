//! Dosya işlemleri: oluştur, yeniden adlandır, sil, kopyala, taşı, bilgi.
//! Silme KALICIDIR (çöp kutusu yok, ZArchiver gibi).

use crate::error::{Error, Result};
use crate::job::Ctx;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    Skip,
    Overwrite,
    KeepBoth,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
}

pub fn validate_name(name: &str) -> Result<()> {
    let n = name.trim();
    if n.is_empty() {
        return Err(Error::InvalidName("ad boş olamaz".into()));
    }
    if n == "." || n == ".." || n.contains('/') || n.contains('\0') {
        return Err(Error::InvalidName(name.into()));
    }
    if n.len() > 255 {
        return Err(Error::InvalidName("ad çok uzun".into()));
    }
    Ok(())
}

pub fn create_dir(parent: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let p = parent.join(name.trim());
    if p.exists() {
        return Err(Error::Exists(name.into()));
    }
    fs::create_dir(&p)?;
    Ok(p)
}

pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let p = parent.join(name.trim());
    fs::OpenOptions::new().write(true).create_new(true).open(&p)?;
    Ok(p)
}

pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    validate_name(new_name)?;
    let parent = path.parent().ok_or_else(|| Error::Invalid("üst klasör yok".into()))?;
    let target = parent.join(new_name.trim());
    if target == path {
        return Ok(target);
    }
    if target.exists() {
        return Err(Error::Exists(new_name.into()));
    }
    fs::rename(path, &target)?;
    Ok(target)
}

/// "ad (1).uzantı" biçiminde var olmayan bir yol üretir.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for i in 1.. {
        let cand = dir.join(format!("{stem} ({i}){ext}"));
        if fs::symlink_metadata(&cand).is_err() {
            return cand;
        }
    }
    unreachable!()
}

/// Hedefte aynı adla var olan öğelerin adları.
pub fn find_conflicts(srcs: &[PathBuf], dest: &Path) -> Vec<String> {
    srcs.iter()
        .filter_map(|s| s.file_name())
        .filter(|n| fs::symlink_metadata(dest.join(n)).is_ok())
        .map(|n| n.to_string_lossy().into_owned())
        .collect()
}

fn walk_stats(p: &Path, st: &mut Stats, ctx: &Ctx) -> Result<()> {
    ctx.check()?;
    let md = match fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    if md.is_dir() {
        st.dirs += 1;
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                walk_stats(&e.path(), st, ctx)?;
            }
        }
    } else if md.is_symlink() {
        if let Ok(m) = fs::metadata(p) {
            if m.is_file() {
                st.files += 1;
                st.bytes += m.len();
            }
        }
    } else {
        st.files += 1;
        st.bytes += md.len();
    }
    Ok(())
}

pub fn stats(paths: &[PathBuf], ctx: &Ctx) -> Result<Stats> {
    let mut st = Stats::default();
    for p in paths {
        walk_stats(p, &mut st, ctx)?;
    }
    Ok(st)
}

fn remove_rec(p: &Path, ctx: &Ctx) -> Result<()> {
    ctx.check()?;
    let md = fs::symlink_metadata(p)?;
    if md.is_dir() {
        for e in fs::read_dir(p)?.flatten() {
            remove_rec(&e.path(), ctx)?;
        }
        fs::remove_dir(p)?;
    } else {
        ctx.progress.set_current(&p.file_name().unwrap_or_default().to_string_lossy());
        fs::remove_file(p)?;
        ctx.progress.add_bytes(md.len());
    }
    ctx.progress.file_done();
    Ok(())
}

pub fn delete(paths: &[PathBuf], ctx: &Ctx) -> Result<()> {
    let st = stats(paths, ctx)?;
    ctx.progress.set_totals(st.bytes, st.files + st.dirs);
    for p in paths {
        remove_rec(p, ctx)?;
    }
    Ok(())
}

fn copy_file(src: &Path, dst: &Path, ctx: &Ctx) -> Result<()> {
    ctx.check()?;
    ctx.progress.set_current(&src.file_name().unwrap_or_default().to_string_lossy());
    let mut input = fs::File::open(src)?;
    let mut output = fs::File::create(dst)?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if ctx.cancel.is_cancelled() {
            drop(output);
            let _ = fs::remove_file(dst);
            return Err(Error::Cancelled);
        }
        let n = match input.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                drop(output);
                let _ = fs::remove_file(dst);
                return Err(e.into());
            }
        };
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n])?;
        ctx.progress.add_bytes(n as u64);
    }
    output.flush()?;
    if let Ok(m) = input.metadata().and_then(|m| m.modified()) {
        let _ = output.set_modified(m);
    }
    ctx.progress.file_done();
    Ok(())
}

fn remove_any(p: &Path) -> Result<()> {
    let md = fs::symlink_metadata(p)?;
    if md.is_dir() {
        fs::remove_dir_all(p)?;
    } else {
        fs::remove_file(p)?;
    }
    Ok(())
}

fn copy_item(src: &Path, target: PathBuf, policy: Conflict, ctx: &Ctx) -> Result<()> {
    ctx.check()?;
    let sm = fs::symlink_metadata(src)?;
    // Klasör sembolik bağlantılarını izleme (döngü riski).
    if sm.is_symlink() && fs::metadata(src).map(|m| m.is_dir()).unwrap_or(true) {
        return Ok(());
    }
    let is_dir = sm.is_dir();
    let mut target = target;
    match fs::symlink_metadata(&target) {
        Ok(tm) => {
            if is_dir && tm.is_dir() {
                // iki klasör: birleştir
            } else {
                match policy {
                    Conflict::Skip => {
                        // atlanan baytları ilerlemeye ekle
                        let mut st = Stats::default();
                        let _ = walk_stats(src, &mut st, ctx);
                        ctx.progress.add_bytes(st.bytes);
                        for _ in 0..(st.files + st.dirs) {
                            ctx.progress.file_done();
                        }
                        return Ok(());
                    }
                    Conflict::Overwrite => remove_any(&target)?,
                    Conflict::KeepBoth => {
                        let dir = target.parent().unwrap_or(Path::new("/")).to_path_buf();
                        let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        target = unique_path(&dir, &name);
                    }
                }
            }
        }
        Err(_) => {}
    }
    if is_dir {
        if !target.exists() {
            fs::create_dir(&target)?;
        }
        for e in fs::read_dir(src)?.flatten() {
            let child_target = target.join(e.file_name());
            copy_item(&e.path(), child_target, policy, ctx)?;
        }
        if let Ok(m) = sm.modified() {
            if let Ok(f) = fs::File::open(&target) {
                let _ = f.set_modified(m);
            }
        }
        ctx.progress.file_done();
        Ok(())
    } else {
        copy_file(src, &target, ctx)
    }
}

fn check_not_into_self(srcs: &[PathBuf], dest: &Path) -> Result<()> {
    for s in srcs {
        if s.is_dir() && dest.starts_with(s) {
            return Err(Error::Invalid("Bir klasör kendi içine kopyalanamaz/taşınamaz".into()));
        }
    }
    Ok(())
}

pub fn copy(srcs: &[PathBuf], dest_dir: &Path, policy: Conflict, ctx: &Ctx) -> Result<()> {
    check_not_into_self(srcs, dest_dir)?;
    let st = stats(srcs, ctx)?;
    ctx.progress.set_totals(st.bytes, st.files + st.dirs);
    for s in srcs {
        let name = s.file_name().ok_or_else(|| Error::Invalid("geçersiz kaynak".into()))?;
        copy_item(s, dest_dir.join(name), policy, ctx)?;
    }
    Ok(())
}

pub fn move_(srcs: &[PathBuf], dest_dir: &Path, policy: Conflict, ctx: &Ctx) -> Result<()> {
    check_not_into_self(srcs, dest_dir)?;
    let st = stats(srcs, ctx)?;
    ctx.progress.set_totals(st.bytes, st.files + st.dirs);
    for s in srcs {
        ctx.check()?;
        let name = s.file_name().ok_or_else(|| Error::Invalid("geçersiz kaynak".into()))?;
        if s.parent() == Some(dest_dir) {
            continue; // aynı klasöre taşıma: işlem yok
        }
        let target = dest_dir.join(name);
        let src_stats = stats(std::slice::from_ref(s), ctx)?;
        let mut target_final = target.clone();
        let exists = fs::symlink_metadata(&target).is_ok();
        let mut can_rename = true;
        if exists {
            match policy {
                Conflict::Skip => {
                    ctx.progress.add_bytes(src_stats.bytes);
                    continue;
                }
                Conflict::Overwrite => {
                    // klasör+klasör birleştirme rename ile olmaz; kopyala+sil yoluna git
                    if s.is_dir() && target.is_dir() {
                        can_rename = false;
                    } else {
                        remove_any(&target)?;
                    }
                }
                Conflict::KeepBoth => {
                    target_final = unique_path(dest_dir, &name.to_string_lossy());
                }
            }
        }
        if can_rename && fs::rename(s, &target_final).is_ok() {
            ctx.progress.add_bytes(src_stats.bytes);
            for _ in 0..(src_stats.files + src_stats.dirs) {
                ctx.progress.file_done();
            }
            continue;
        }
        // Başka bölüme taşıma: kopyala, sonra sil
        copy_item(s, target_final, policy, ctx)?;
        // Kaynak silinirken ilerleme çift sayılmasın
        remove_rec_silent(s, ctx)?;
    }
    Ok(())
}

fn remove_rec_silent(p: &Path, ctx: &Ctx) -> Result<()> {
    ctx.check()?;
    let md = fs::symlink_metadata(p)?;
    if md.is_dir() {
        for e in fs::read_dir(p)?.flatten() {
            remove_rec_silent(&e.path(), ctx)?;
        }
        fs::remove_dir(p)?;
    } else {
        fs::remove_file(p)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoData {
    pub count: usize,
    pub stats: Stats,
    /// Tek öğe seçiliyse: (ad, tam yol, klasör mü, değiştirilme)
    pub single: Option<(String, String, bool, Option<i64>)>,
}

pub fn info(paths: &[PathBuf], ctx: &Ctx) -> Result<InfoData> {
    let st = stats(paths, ctx)?;
    let single = if paths.len() == 1 {
        let p = &paths[0];
        let md = fs::symlink_metadata(p)?;
        Some((
            p.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            p.to_string_lossy().into_owned(),
            md.is_dir(),
            md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64),
        ))
    } else {
        None
    };
    Ok(InfoData { count: paths.len(), stats: st, single })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, read, write};

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("rarch_ops_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn names() {
        assert!(validate_name("ok.txt").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
    }

    #[test]
    fn create_rename_unique() {
        let d = tmp("cr");
        let f = create_dir(&d, "Yeni").unwrap();
        assert!(f.is_dir());
        assert!(matches!(create_dir(&d, "Yeni"), Err(Error::Exists(_))));
        let t = create_file(&d, "a.txt").unwrap();
        assert!(t.is_file());
        let r = rename(&t, "b.txt").unwrap();
        assert!(r.exists() && !t.exists());
        assert!(matches!(rename(&r, "Yeni"), Err(Error::Exists(_))));
        assert_eq!(unique_path(&d, "b.txt"), d.join("b (1).txt"));
        write(d.join("b (1).txt"), b"x").unwrap();
        assert_eq!(unique_path(&d, "b.txt"), d.join("b (2).txt"));
        assert_eq!(unique_path(&d, "yok.bin"), d.join("yok.bin"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn copy_move_delete_tree() {
        let d = tmp("tree");
        let src = d.join("src");
        create_dir_all(src.join("sub")).unwrap();
        write(src.join("a.txt"), b"AAA").unwrap();
        write(src.join("sub/b.txt"), b"BBBBB").unwrap();
        let dst = d.join("dst");
        create_dir_all(&dst).unwrap();
        let ctx = Ctx::new();

        copy(&[src.clone()], &dst, Conflict::Skip, &ctx).unwrap();
        assert_eq!(read(dst.join("src/sub/b.txt")).unwrap(), b"BBBBB");
        let snap = ctx.progress.snapshot();
        assert_eq!(snap.total_bytes, 8);
        assert_eq!(snap.done_bytes, 8);

        // çakışma
        assert_eq!(find_conflicts(&[src.clone()], &dst), vec!["src".to_string()]);
        write(src.join("a.txt"), b"NEW").unwrap();
        copy(&[src.clone()], &dst, Conflict::Skip, &Ctx::new()).unwrap();
        assert_eq!(read(dst.join("src/a.txt")).unwrap(), b"AAA"); // klasör birleşti, dosya zaten vardı ve src klasörü varsayılan birleştirildi
        copy(&[src.clone()], &dst, Conflict::KeepBoth, &Ctx::new()).unwrap();
        assert!(dst.join("src/a (1).txt").exists());

        // taşı
        let mv = d.join("mv");
        create_dir_all(&mv).unwrap();
        move_(&[src.clone()], &mv, Conflict::Overwrite, &Ctx::new()).unwrap();
        assert!(!src.exists());
        assert_eq!(read(mv.join("src/a.txt")).unwrap(), b"NEW");

        // kendi içine
        assert!(copy(&[mv.clone()], &mv.join("src"), Conflict::Skip, &Ctx::new()).is_err());

        // sil
        let c = Ctx::new();
        delete(&[dst.clone(), mv.clone()], &c).unwrap();
        assert!(!dst.exists() && !mv.exists());
        let s = c.progress.snapshot();
        assert_eq!(s.done_files, s.total_files);

        // info
        let i = info(&[d.clone()], &Ctx::new()).unwrap();
        assert_eq!(i.count, 1);
        assert!(i.single.is_some());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn cancel_copy() {
        let d = tmp("cancel");
        write(d.join("a"), vec![1u8; 1024 * 1024]).unwrap();
        let ctx = Ctx::new();
        ctx.cancel.cancel();
        let out = d.join("out");
        create_dir_all(&out).unwrap();
        assert_eq!(copy(&[d.join("a")], &out, Conflict::Skip, &ctx), Err(Error::Cancelled));
        assert!(!out.join("a").exists());
        let _ = fs::remove_dir_all(&d);
    }
}
