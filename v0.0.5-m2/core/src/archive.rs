//! Arşiv işlemleri: liste, çıkarma, test ve oluşturma.
//! Biçimler: ZIP (AES-256), 7z (AES-256), TAR, TAR.GZ, TAR.BZ2, TAR.XZ, GZ, BZ2, XZ.
//! Hepsi saf Rust (C bağımlılığı yok), bu yüzden Android'de sorunsuz derlenir.
//! Henüz yok: RAR, LZ4, ZSTD, bölünmüş (volume) arşiv.

use crate::error::{Error, Result};
use crate::fsops::{self, Conflict};
use crate::job::{copy_stream, CountingReader, Ctx};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    Gz,
    Bz2,
    Xz,
}

impl ArchiveFormat {
    pub const ALL: [ArchiveFormat; 9] = [
        ArchiveFormat::SevenZ,
        ArchiveFormat::Zip,
        ArchiveFormat::Tar,
        ArchiveFormat::TarGz,
        ArchiveFormat::TarBz2,
        ArchiveFormat::TarXz,
        ArchiveFormat::Gz,
        ArchiveFormat::Bz2,
        ArchiveFormat::Xz,
    ];
    pub fn extension(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::SevenZ => "7z",
            ArchiveFormat::Tar => "tar",
            ArchiveFormat::TarGz => "tar.gz",
            ArchiveFormat::TarBz2 => "tar.bz2",
            ArchiveFormat::TarXz => "tar.xz",
            ArchiveFormat::Gz => "gz",
            ArchiveFormat::Bz2 => "bz2",
            ArchiveFormat::Xz => "xz",
        }
    }
    pub fn supports_password(self) -> bool {
        matches!(self, ArchiveFormat::Zip | ArchiveFormat::SevenZ)
    }
    /// Yalnızca tek dosya sıkıştırır (klasör/çoklu dosya olmaz).
    pub fn is_single_file(self) -> bool {
        matches!(self, ArchiveFormat::Gz | ArchiveFormat::Bz2 | ArchiveFormat::Xz)
    }
    pub fn has_level(self) -> bool {
        self != ArchiveFormat::Tar
    }
}

pub fn detect_by_name(name: &str) -> Option<ArchiveFormat> {
    let n = name.to_lowercase();
    let ends = |s: &[&str]| s.iter().any(|x| n.ends_with(x));
    if ends(&[".tar.gz", ".tgz"]) {
        Some(ArchiveFormat::TarGz)
    } else if ends(&[".tar.bz2", ".tbz2", ".tbz"]) {
        Some(ArchiveFormat::TarBz2)
    } else if ends(&[".tar.xz", ".txz"]) {
        Some(ArchiveFormat::TarXz)
    } else if ends(&[".tar"]) {
        Some(ArchiveFormat::Tar)
    } else if ends(&[".zip", ".jar", ".apk", ".xapk", ".apks", ".aar"]) {
        Some(ArchiveFormat::Zip)
    } else if ends(&[".7z"]) {
        Some(ArchiveFormat::SevenZ)
    } else if ends(&[".gz"]) {
        Some(ArchiveFormat::Gz)
    } else if ends(&[".bz2"]) {
        Some(ArchiveFormat::Bz2)
    } else if ends(&[".xz"]) {
        Some(ArchiveFormat::Xz)
    } else {
        None
    }
}

/// Dosya içeriğinden ("Arşiv olarak aç") biçim tahmini.
pub fn detect_by_magic(path: &Path) -> Option<ArchiveFormat> {
    let mut buf = [0u8; 512];
    let n = File::open(path).ok()?.read(&mut buf).ok()?;
    let b = &buf[..n];
    if b.starts_with(b"PK\x03\x04") || b.starts_with(b"PK\x05\x06") {
        Some(ArchiveFormat::Zip)
    } else if b.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        Some(ArchiveFormat::SevenZ)
    } else if b.starts_with(&[0x1F, 0x8B]) {
        Some(ArchiveFormat::Gz)
    } else if b.starts_with(b"BZh") {
        Some(ArchiveFormat::Bz2)
    } else if b.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]) {
        Some(ArchiveFormat::Xz)
    } else if n >= 262 && &b[257..262] == b"ustar" {
        Some(ArchiveFormat::Tar)
    } else {
        None
    }
}

pub fn detect(path: &Path) -> Option<ArchiveFormat> {
    let name = path.file_name()?.to_string_lossy();
    detect_by_name(&name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// '/' ayırıcılı, başta/sonda '/' olmayan yol
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<i64>,
    pub encrypted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TestReport {
    pub checked: usize,
    pub errors: Vec<String>,
}

fn zname(n: std::result::Result<std::borrow::Cow<'_, str>, zip::result::ZipError>) -> String {
    n.map(|c| c.into_owned()).unwrap_or_default()
}

fn norm_path(raw: &str) -> String {
    let s = raw.replace('\\', "/");
    let s = s.trim_start_matches("./").trim_start_matches('/');
    s.trim_end_matches('/').to_string()
}

/// Arşiv yolunu hedef klasörün altında güvenli bir yola çevirir (zip-slip koruması).
pub fn safe_join(dest: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = dest.to_path_buf();
    let mut any = false;
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(p) => {
                out.push(p);
                any = true;
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    any.then_some(out)
}

fn selected(path: &str, sel: &[String]) -> bool {
    sel.is_empty() || sel.iter().any(|s| path == s || path.starts_with(&format!("{s}/")))
}

/// `base` (arşiv içindeki gezilen klasör) önekini çıkarır.
fn rel_path(path: &str, base: &str) -> Option<String> {
    if base.is_empty() {
        return Some(path.to_string());
    }
    let pref = format!("{base}/");
    path.strip_prefix(&pref).map(|s| s.to_string())
}

// ---------- hata eşleme ----------

fn map_zip(e: zip::result::ZipError) -> Error {
    use zip::result::ZipError as Z;
    match e {
        Z::Io(e) => e.into(),
        Z::InvalidPassword => Error::WrongPassword,
        Z::InvalidArchive(s) => Error::Corrupt(s.to_string()),
        Z::UnsupportedArchive(s) => Error::Unsupported(s.to_string()),
        other => Error::Corrupt(other.to_string()),
    }
}

fn map_7z(e: sevenz_rust2::Error) -> Error {
    use sevenz_rust2::Error as S;
    match e {
        S::PasswordRequired => Error::NeedPassword,
        S::MaybeBadPassword(_) => Error::WrongPassword,
        S::Io(e, _) | S::FileOpen(e, _) => e.into(),
        other => Error::Corrupt(other.to_string()),
    }
}

// ---------- zaman yardımcıları ----------

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn zip_dt_to_unix(dt: zip::DateTime) -> i64 {
    days_from_civil(dt.year() as i64, dt.month() as i64, dt.day() as i64) * 86_400
        + dt.hour() as i64 * 3600
        + dt.minute() as i64 * 60
        + dt.second() as i64
}

fn unix_to_zip_dt(secs: i64) -> Option<zip::DateTime> {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let y = (yoe + era * 400 + (m <= 2) as i64) as u16;
    zip::DateTime::from_date_and_time(y, m, d, (rem / 3600) as u8, (rem % 3600 / 60) as u8, (rem % 60) as u8).ok()
}

fn set_mtime(path: &Path, secs: Option<i64>) {
    if let Some(s) = secs.filter(|s| *s > 0) {
        if let Ok(f) = fs::OpenOptions::new().write(true).open(path) {
            let _ = f.set_modified(UNIX_EPOCH + Duration::from_secs(s as u64));
        }
    }
}

// ---------- okuyucular ----------

fn open_decoder(path: &Path, fmt: ArchiveFormat) -> Result<Box<dyn Read>> {
    let f = BufReader::new(File::open(path)?);
    Ok(match fmt {
        ArchiveFormat::Tar => Box::new(f),
        ArchiveFormat::TarGz | ArchiveFormat::Gz => Box::new(flate2::read::MultiGzDecoder::new(f)),
        ArchiveFormat::TarBz2 | ArchiveFormat::Bz2 => Box::new(bzip2::read::MultiBzDecoder::new(f)),
        ArchiveFormat::TarXz | ArchiveFormat::Xz => Box::new(lzma_rust2::XzReader::new(f, true)),
        _ => return Err(Error::Unsupported("bu biçim akış olarak açılamaz".into())),
    })
}

fn single_name(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => format!("{name}.out"),
    }
}

fn require_format(path: &Path) -> Result<ArchiveFormat> {
    detect(path)
        .or_else(|| detect_by_magic(path))
        .ok_or_else(|| Error::Unsupported("arşiv biçimi tanınmadı".into()))
}

// ---------- liste ----------

pub fn list(path: &Path, password: Option<&str>) -> Result<Vec<ArchiveEntry>> {
    match require_format(path)? {
        ArchiveFormat::Zip => list_zip(path),
        ArchiveFormat::SevenZ => list_7z(path, password),
        f @ (ArchiveFormat::Tar | ArchiveFormat::TarGz | ArchiveFormat::TarBz2 | ArchiveFormat::TarXz) => list_tar(path, f),
        ArchiveFormat::Gz | ArchiveFormat::Bz2 | ArchiveFormat::Xz => Ok(vec![ArchiveEntry {
            path: single_name(path),
            is_dir: false,
            size: 0,
            modified: None,
            encrypted: false,
        }]),
    }
}

fn list_zip(path: &Path) -> Result<Vec<ArchiveEntry>> {
    let mut z = zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(map_zip)?;
    let mut out = Vec::with_capacity(z.len());
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(map_zip)?;
        let p = norm_path(&zname(f.name()));
        if p.is_empty() {
            continue;
        }
        out.push(ArchiveEntry {
            path: p,
            is_dir: f.is_dir(),
            size: f.size(),
            modified: f.last_modified().map(zip_dt_to_unix),
            encrypted: f.encrypted(),
        });
    }
    Ok(out)
}

fn password_7z(pw: Option<&str>) -> sevenz_rust2::Password {
    match pw {
        Some(p) => sevenz_rust2::Password::new(p),
        None => sevenz_rust2::Password::empty(),
    }
}

fn list_7z(path: &Path, password: Option<&str>) -> Result<Vec<ArchiveEntry>> {
    let r = sevenz_rust2::ArchiveReader::open(path, password_7z(password)).map_err(map_7z)?;
    let mut out = Vec::new();
    for e in &r.archive().files {
        if e.is_anti_item() {
            continue;
        }
        let p = norm_path(e.name());
        if p.is_empty() {
            continue;
        }
        let modified = if e.has_last_modified_date {
            let st: std::time::SystemTime = e.last_modified_date().into();
            st.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
        } else {
            None
        };
        out.push(ArchiveEntry { path: p, is_dir: e.is_directory(), size: e.size(), modified, encrypted: false });
    }
    Ok(out)
}

fn list_tar(path: &Path, fmt: ArchiveFormat) -> Result<Vec<ArchiveEntry>> {
    let mut ar = tar::Archive::new(open_decoder(path, fmt)?);
    let mut out = Vec::new();
    for e in ar.entries().map_err(Error::from)? {
        let e = e?;
        let p = norm_path(&e.path()?.to_string_lossy());
        if p.is_empty() {
            continue;
        }
        let t = e.header().entry_type();
        if !(t.is_dir() || t.is_file()) {
            continue;
        }
        out.push(ArchiveEntry {
            path: p,
            is_dir: t.is_dir(),
            size: e.size(),
            modified: e.header().mtime().ok().map(|m| m as i64),
            encrypted: false,
        });
    }
    Ok(out)
}

// ---------- çıkarma ----------

/// Çıkış dosyası yolunu hazırlar. None = atla.
fn prepare_output(dest: &Path, rel: &str, policy: Conflict) -> Result<Option<PathBuf>> {
    let out = safe_join(dest, rel).ok_or_else(|| Error::Invalid(format!("güvensiz yol: {rel}")))?;
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::symlink_metadata(&out).is_ok() {
        match policy {
            Conflict::Skip => return Ok(None),
            Conflict::Overwrite => {
                if out.is_dir() {
                    fs::remove_dir_all(&out)?;
                } else {
                    fs::remove_file(&out)?;
                }
            }
            Conflict::KeepBoth => {
                let dir = out.parent().unwrap_or(dest).to_path_buf();
                return Ok(Some(fsops::unique_path(&dir, &out.file_name().unwrap_or_default().to_string_lossy())));
            }
        }
    }
    Ok(Some(out))
}

fn make_dir(dest: &Path, rel: &str) -> Result<()> {
    let p = safe_join(dest, rel).ok_or_else(|| Error::Invalid(format!("güvensiz yol: {rel}")))?;
    fs::create_dir_all(p)?;
    Ok(())
}

fn write_out(
    reader: &mut dyn Read,
    out: &Path,
    modified: Option<i64>,
    ctx: &Ctx,
) -> Result<()> {
    let mut o = BufWriter::new(File::create(out)?);
    let res = copy_stream(reader, &mut o, ctx).and_then(|_| o.flush().map_err(Error::from));
    drop(o);
    match res {
        Ok(()) => {
            set_mtime(out, modified);
            ctx.progress.file_done();
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(out);
            Err(e)
        }
    }
}

/// `sel`: çıkarılacak arşiv yolları (boş = hepsi). `base`: arşiv içinde gezilen klasör
/// (seçimler ona göre yerleştirilir). Güvensiz yollar hata verir.
pub fn extract(
    path: &Path,
    dest: &Path,
    sel: &[String],
    base: &str,
    password: Option<&str>,
    policy: Conflict,
    ctx: &Ctx,
) -> Result<()> {
    let fmt = require_format(path)?;
    fs::create_dir_all(dest)?;
    match fmt {
        ArchiveFormat::Zip => extract_zip(path, dest, sel, base, password, policy, ctx),
        ArchiveFormat::SevenZ => extract_7z(path, dest, sel, base, password, policy, ctx),
        ArchiveFormat::Gz | ArchiveFormat::Bz2 | ArchiveFormat::Xz => {
            ctx.progress.set_totals(0, 1);
            let name = single_name(path);
            let Some(out) = prepare_output(dest, &name, policy)? else { return Ok(()) };
            ctx.progress.set_current(&name);
            let mut r = open_decoder(path, fmt)?;
            write_out(&mut r, &out, None, ctx)
        }
        _ => extract_tar(path, fmt, dest, sel, base, policy, ctx),
    }
}

fn totals(entries: &[ArchiveEntry], sel: &[String]) -> (u64, u64) {
    entries.iter().filter(|e| selected(&e.path, sel)).fold((0, 0), |(b, n), e| (b + e.size, n + 1))
}

fn extract_zip(path: &Path, dest: &Path, sel: &[String], base: &str, pw: Option<&str>, policy: Conflict, ctx: &Ctx) -> Result<()> {
    let mut z = zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(map_zip)?;
    let mut idx = Vec::new();
    let (mut bytes, mut files) = (0u64, 0u64);
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(map_zip)?;
        if selected(&norm_path(&zname(f.name())), sel) {
            idx.push(i);
            bytes += f.size();
            files += 1;
        }
    }
    ctx.progress.set_totals(bytes, files);
    for i in idx {
        ctx.check()?;
        let (p, is_dir, enc, mtime) = {
            let f = z.by_index_raw(i).map_err(map_zip)?;
            (norm_path(&zname(f.name())), f.is_dir(), f.encrypted(), f.last_modified().map(zip_dt_to_unix))
        };
        let Some(rel) = rel_path(&p, base) else { continue };
        if is_dir {
            make_dir(dest, &rel)?;
            ctx.progress.file_done();
            continue;
        }
        let Some(out) = prepare_output(dest, &rel, policy)? else {
            ctx.progress.add_bytes(z.by_index_raw(i).map_err(map_zip)?.size());
            ctx.progress.file_done();
            continue;
        };
        ctx.progress.set_current(&rel);
        let mut f = if enc {
            let pw = pw.ok_or(Error::NeedPassword)?;
            z.by_index_decrypt(i, pw.as_bytes()).map_err(map_zip)?
        } else {
            z.by_index(i).map_err(map_zip)?
        };
        match write_out(&mut f, &out, mtime, ctx) {
            Err(Error::Io(_)) if enc => return Err(Error::WrongPassword),
            r => r?,
        }
    }
    Ok(())
}

fn extract_7z(path: &Path, dest: &Path, sel: &[String], base: &str, pw: Option<&str>, policy: Conflict, ctx: &Ctx) -> Result<()> {
    let entries = list_7z(path, pw)?;
    let (b, n) = totals(&entries, sel);
    ctx.progress.set_totals(b, n);
    let mut r = sevenz_rust2::ArchiveReader::open(path, password_7z(pw)).map_err(map_7z)?;
    let mut failure: Option<Error> = None;
    let res = r.for_each_entries(|entry, reader| {
        if ctx.cancel.is_cancelled() {
            failure = Some(Error::Cancelled);
            return Ok(false);
        }
        let p = norm_path(entry.name());
        let rel = if selected(&p, sel) { rel_path(&p, base) } else { None };
        let Some(rel) = rel else {
            io::copy(reader, &mut io::sink())?;
            return Ok(true);
        };
        let step = (|| -> Result<()> {
            if entry.is_directory() {
                make_dir(dest, &rel)?;
                ctx.progress.file_done();
                return Ok(());
            }
            let Some(out) = prepare_output(dest, &rel, policy)? else {
                io::copy(reader, &mut io::sink())?;
                ctx.progress.add_bytes(entry.size());
                ctx.progress.file_done();
                return Ok(());
            };
            ctx.progress.set_current(&rel);
            write_out(reader, &out, None, ctx)
        })();
        match step {
            Ok(()) => Ok(true),
            Err(e) => {
                failure = Some(e);
                Ok(false)
            }
        }
    });
    if let Some(e) = failure {
        return Err(e);
    }
    res.map_err(map_7z)
}

fn extract_tar(path: &Path, fmt: ArchiveFormat, dest: &Path, sel: &[String], base: &str, policy: Conflict, ctx: &Ctx) -> Result<()> {
    let entries = list_tar(path, fmt)?;
    let (b, n) = totals(&entries, sel);
    ctx.progress.set_totals(b, n);
    let mut ar = tar::Archive::new(open_decoder(path, fmt)?);
    for e in ar.entries()? {
        ctx.check()?;
        let mut e = e?;
        let p = norm_path(&e.path()?.to_string_lossy());
        let t = e.header().entry_type();
        if p.is_empty() || !(t.is_dir() || t.is_file()) || !selected(&p, sel) {
            continue;
        }
        let Some(rel) = rel_path(&p, base) else { continue };
        if t.is_dir() {
            make_dir(dest, &rel)?;
            ctx.progress.file_done();
            continue;
        }
        let mtime = e.header().mtime().ok().map(|m| m as i64);
        let size = e.size();
        let Some(out) = prepare_output(dest, &rel, policy)? else {
            ctx.progress.add_bytes(size);
            ctx.progress.file_done();
            continue;
        };
        ctx.progress.set_current(&rel);
        write_out(&mut e, &out, mtime, ctx)?;
    }
    Ok(())
}

// ---------- test ----------

pub fn test(path: &Path, password: Option<&str>, ctx: &Ctx) -> Result<TestReport> {
    let fmt = require_format(path)?;
    let mut rep = TestReport::default();
    match fmt {
        ArchiveFormat::Zip => {
            let mut z = zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(map_zip)?;
            ctx.progress.set_totals(0, z.len() as u64);
            for i in 0..z.len() {
                ctx.check()?;
                let (name, enc, dir) = {
                    let f = z.by_index_raw(i).map_err(map_zip)?;
                    (norm_path(&zname(f.name())), f.encrypted(), f.is_dir())
                };
                if dir {
                    ctx.progress.file_done();
                    continue;
                }
                ctx.progress.set_current(&name);
                let opened = if enc {
                    match password {
                        Some(p) => z.by_index_decrypt(i, p.as_bytes()),
                        None => return Err(Error::NeedPassword),
                    }
                } else {
                    z.by_index(i)
                };
                match opened {
                    Ok(mut f) => {
                        if let Err(e) = io::copy(&mut f, &mut io::sink()) {
                            if enc {
                                return Err(Error::WrongPassword);
                            }
                            rep.errors.push(format!("{name}: {e}"));
                        }
                    }
                    Err(e) => match map_zip(e) {
                        Error::WrongPassword => return Err(Error::WrongPassword),
                        e => rep.errors.push(format!("{name}: {e}")),
                    },
                }
                rep.checked += 1;
                ctx.progress.file_done();
            }
        }
        ArchiveFormat::SevenZ => {
            let entries = list_7z(path, password)?;
            ctx.progress.set_totals(0, entries.len() as u64);
            let mut r = sevenz_rust2::ArchiveReader::open(path, password_7z(password)).map_err(map_7z)?;
            let mut checked = 0usize;
            let mut cancelled = false;
            let res = r.for_each_entries(|entry, reader| {
                if ctx.cancel.is_cancelled() {
                    cancelled = true;
                    return Ok(false);
                }
                ctx.progress.set_current(entry.name());
                io::copy(reader, &mut io::sink())?;
                checked += 1;
                ctx.progress.file_done();
                Ok(true)
            });
            if cancelled {
                return Err(Error::Cancelled);
            }
            rep.checked = checked;
            if let Err(e) = res {
                match map_7z(e) {
                    Error::NeedPassword => return Err(Error::NeedPassword),
                    Error::WrongPassword => return Err(Error::WrongPassword),
                    e => rep.errors.push(e.to_string()),
                }
            }
        }
        ArchiveFormat::Gz | ArchiveFormat::Bz2 | ArchiveFormat::Xz => {
            let mut r = open_decoder(path, fmt)?;
            match io::copy(&mut r, &mut io::sink()) {
                Ok(_) => rep.checked = 1,
                Err(e) => rep.errors.push(e.to_string()),
            }
        }
        _ => {
            let mut ar = tar::Archive::new(open_decoder(path, fmt)?);
            let res: Result<()> = (|| {
                for e in ar.entries()? {
                    ctx.check()?;
                    let mut e = e?;
                    io::copy(&mut e, &mut io::sink())?;
                    rep.checked += 1;
                }
                Ok(())
            })();
            match res {
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => rep.errors.push(e.to_string()),
                Ok(()) => {}
            }
        }
    }
    Ok(rep)
}

// ---------- oluşturma ----------

#[derive(Debug, Clone)]
pub struct CompressOptions {
    pub format: ArchiveFormat,
    /// 0 = sıkıştırma yok (depola) ... 9 = en yüksek
    pub level: u32,
    pub password: Option<String>,
    /// Yalnızca 7z: dosya adlarını da şifrele
    pub encrypt_names: bool,
    pub delete_sources: bool,
}

impl Default for CompressOptions {
    fn default() -> Self {
        Self { format: ArchiveFormat::Zip, level: 5, password: None, encrypt_names: false, delete_sources: false }
    }
}

struct Item {
    abs: PathBuf,
    rel: String,
    is_dir: bool,
    size: u64,
    mtime: Option<i64>,
}

fn collect_items(sources: &[PathBuf], ctx: &Ctx) -> Result<Vec<Item>> {
    fn walk(p: &Path, rel: String, out: &mut Vec<Item>, ctx: &Ctx) -> Result<()> {
        ctx.check()?;
        let md = fs::symlink_metadata(p)?;
        let mtime = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
        if md.is_symlink() {
            // dosya bağlantısını içeriğiyle al, klasör bağlantısını atla
            match fs::metadata(p) {
                Ok(m) if m.is_file() => {
                    out.push(Item { abs: p.to_path_buf(), rel, is_dir: false, size: m.len(), mtime });
                }
                _ => {}
            }
            return Ok(());
        }
        if md.is_dir() {
            out.push(Item { abs: p.to_path_buf(), rel: rel.clone(), is_dir: true, size: 0, mtime });
            let mut kids: Vec<_> = fs::read_dir(p)?.flatten().collect();
            kids.sort_by_key(|e| e.file_name());
            for k in kids {
                walk(&k.path(), format!("{rel}/{}", k.file_name().to_string_lossy()), out, ctx)?;
            }
        } else {
            out.push(Item { abs: p.to_path_buf(), rel, is_dir: false, size: md.len(), mtime });
        }
        Ok(())
    }
    let mut out = Vec::new();
    for s in sources {
        let name = s.file_name().ok_or_else(|| Error::Invalid("geçersiz kaynak".into()))?.to_string_lossy().into_owned();
        walk(s, name, &mut out, ctx)?;
    }
    Ok(out)
}

/// Kaynaklardan varsayılan arşiv adı: tek kaynak adı (uzantısız dosya) ya da üst klasör adı.
pub fn default_archive_name(sources: &[PathBuf], fmt: ArchiveFormat) -> String {
    let base = if sources.len() == 1 {
        let n = sources[0].file_name().unwrap_or_default().to_string_lossy().into_owned();
        if sources[0].is_dir() {
            n
        } else {
            match n.rsplit_once('.') {
                Some((s, _)) if !s.is_empty() => s.to_string(),
                _ => n,
            }
        }
    } else {
        sources
            .first()
            .and_then(|s| s.parent())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Arsiv".into())
    };
    format!("{base}.{}", fmt.extension())
}

fn write_tar<W: Write>(w: W, items: &[Item], ctx: &Ctx) -> Result<W> {
    let mut b = tar::Builder::new(w);
    for it in items {
        ctx.check()?;
        if it.is_dir {
            b.append_dir(&it.rel, &it.abs)?;
        } else {
            ctx.progress.set_current(&it.rel);
            let f = File::open(&it.abs)?;
            let md = f.metadata()?;
            let mut h = tar::Header::new_gnu();
            h.set_metadata(&md);
            h.set_size(md.len());
            b.append_data(&mut h, &it.rel, CountingReader { inner: f, ctx })?;
            ctx.progress.file_done();
        }
    }
    Ok(b.into_inner()?)
}

fn write_zip(out: &Path, items: &[Item], o: &CompressOptions, ctx: &Ctx) -> Result<()> {
    let mut zw = zip::ZipWriter::new(BufWriter::new(File::create(out)?));
    for it in items {
        ctx.check()?;
        let mut opts = zip::write::SimpleFileOptions::default()
            .compression_method(if o.level == 0 { zip::CompressionMethod::Stored } else { zip::CompressionMethod::Deflated })
            .compression_level(Some(o.level.clamp(1, 9) as i64))
            .large_file(it.size > 0xFFFF_FFFF);
        if let Some(dt) = it.mtime.and_then(unix_to_zip_dt) {
            opts = opts.last_modified_time(dt);
        }
        if it.is_dir {
            zw.add_directory(&it.rel, opts).map_err(map_zip)?;
            continue;
        }
        ctx.progress.set_current(&it.rel);
        match o.password.as_deref() {
            Some(pw) => zw.start_file(&it.rel, opts.with_aes_encryption(zip::AesMode::Aes256, pw)),
            None => zw.start_file(&it.rel, opts),
        }
        .map_err(map_zip)?;
        let mut r = CountingReader { inner: File::open(&it.abs)?, ctx };
        io::copy(&mut r, &mut zw)?;
        ctx.progress.file_done();
    }
    zw.finish().map_err(map_zip)?;
    Ok(())
}

fn write_7z(out: &Path, items: &[Item], o: &CompressOptions, ctx: &Ctx) -> Result<()> {
    use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
    let mut w = sevenz_rust2::ArchiveWriter::create(out).map_err(map_7z)?;
    let mut methods: Vec<sevenz_rust2::EncoderConfiguration> = Vec::new();
    if let Some(pw) = o.password.as_deref() {
        methods.push(AesEncoderOptions::new(sevenz_rust2::Password::new(pw)).into());
    }
    if o.level == 0 {
        methods.push(sevenz_rust2::EncoderMethod::COPY.into());
    } else {
        methods.push(Lzma2Options::from_level(o.level.clamp(1, 9)).into());
    }
    w.set_content_methods(methods);
    if o.encrypt_names && o.password.is_some() {
        w.set_encrypt_header(true);
    }
    for it in items {
        ctx.check()?;
        let entry = sevenz_rust2::ArchiveEntry::from_path(&it.abs, it.rel.clone());
        if it.is_dir {
            w.push_archive_entry::<&[u8]>(entry, None).map_err(map_7z)?;
        } else {
            ctx.progress.set_current(&it.rel);
            let r = CountingReader { inner: File::open(&it.abs)?, ctx };
            w.push_archive_entry(entry, Some(r)).map_err(map_7z)?;
            ctx.progress.file_done();
        }
    }
    w.finish()?;
    Ok(())
}

fn write_single(out: &Path, item: &Item, fmt: ArchiveFormat, level: u32, ctx: &Ctx) -> Result<()> {
    let mut src = File::open(&item.abs)?;
    let bw = BufWriter::new(File::create(out)?);
    ctx.progress.set_current(&item.rel);
    match fmt {
        ArchiveFormat::Gz => {
            let mut e = flate2::write::GzEncoder::new(bw, flate2::Compression::new(level.min(9)));
            copy_stream(&mut src, &mut e, ctx)?;
            e.finish()?.flush()?;
        }
        ArchiveFormat::Bz2 => {
            let mut e = bzip2::write::BzEncoder::new(bw, bzip2::Compression::new(level.clamp(1, 9)));
            copy_stream(&mut src, &mut e, ctx)?;
            e.finish()?.flush()?;
        }
        ArchiveFormat::Xz => {
            let mut e = lzma_rust2::XzWriter::new(bw, lzma_rust2::XzOptions::with_preset(level.min(9)))?;
            copy_stream(&mut src, &mut e, ctx)?;
            e.finish()?.flush()?;
        }
        _ => unreachable!(),
    }
    ctx.progress.file_done();
    Ok(())
}

/// `sources`'u tek bir arşive sıkıştırır. Hedef varsa Error::Exists.
pub fn compress(sources: &[PathBuf], out: &Path, o: &CompressOptions, ctx: &Ctx) -> Result<()> {
    if sources.is_empty() {
        return Err(Error::Invalid("kaynak yok".into()));
    }
    if out.exists() {
        return Err(Error::Exists(out.file_name().unwrap_or_default().to_string_lossy().into_owned()));
    }
    for s in sources {
        if out.starts_with(s) {
            return Err(Error::Invalid("Arşiv kendi içine yazılamaz".into()));
        }
    }
    if let Some(p) = o.password.as_deref() {
        if p.is_empty() {
            return Err(Error::Invalid("parola boş olamaz".into()));
        }
        if !o.format.supports_password() {
            return Err(Error::Unsupported("bu biçim parolayı desteklemiyor".into()));
        }
    }
    let items = collect_items(sources, ctx)?;
    if o.format.is_single_file() && (items.len() != 1 || items[0].is_dir) {
        return Err(Error::Invalid("Bu biçim yalnızca tek bir dosyayı sıkıştırır".into()));
    }
    let (bytes, files) = items.iter().fold((0u64, 0u64), |(b, n), i| (b + i.size, n + !i.is_dir as u64));
    ctx.progress.set_totals(bytes, files);

    let res: Result<()> = match o.format {
        ArchiveFormat::Zip => write_zip(out, &items, o, ctx),
        ArchiveFormat::SevenZ => write_7z(out, &items, o, ctx),
        ArchiveFormat::Tar => (|| {
            let w = write_tar(BufWriter::new(File::create(out)?), &items, ctx)?;
            w.into_inner().map_err(|e| Error::Io(e.to_string()))?.flush()?;
            Ok(())
        })(),
        ArchiveFormat::TarGz => (|| {
            let enc = flate2::write::GzEncoder::new(BufWriter::new(File::create(out)?), flate2::Compression::new(o.level.min(9)));
            write_tar(enc, &items, ctx)?.finish()?.flush()?;
            Ok(())
        })(),
        ArchiveFormat::TarBz2 => (|| {
            let enc = bzip2::write::BzEncoder::new(BufWriter::new(File::create(out)?), bzip2::Compression::new(o.level.clamp(1, 9)));
            write_tar(enc, &items, ctx)?.finish()?.flush()?;
            Ok(())
        })(),
        ArchiveFormat::TarXz => (|| {
            let enc = lzma_rust2::XzWriter::new(BufWriter::new(File::create(out)?), lzma_rust2::XzOptions::with_preset(o.level.min(9)))?;
            write_tar(enc, &items, ctx)?.finish()?.flush()?;
            Ok(())
        })(),
        ArchiveFormat::Gz | ArchiveFormat::Bz2 | ArchiveFormat::Xz => write_single(out, &items[0], o.format, o.level, ctx),
    };
    if let Err(e) = res {
        let _ = fs::remove_file(out); // yarım kalan arşivi bırakma
        return Err(e);
    }
    if o.delete_sources {
        fsops::delete(sources, ctx)?;
    }
    Ok(())
}

/// Her kaynak için ayrı arşiv ("Ayrı arşiv oluştur"). Çıktı adları benzersizleştirilir.
pub fn compress_separate(sources: &[PathBuf], out_dir: &Path, o: &CompressOptions, ctx: &Ctx) -> Result<Vec<PathBuf>> {
    let mut outs = Vec::new();
    let mut opts = o.clone();
    opts.delete_sources = false;
    for s in sources {
        ctx.check()?;
        let name = default_archive_name(std::slice::from_ref(s), o.format);
        let out = fsops::unique_path(out_dir, &name);
        compress(std::slice::from_ref(s), &out, &opts, ctx)?;
        outs.push(out);
    }
    if o.delete_sources {
        fsops::delete(sources, ctx)?;
    }
    Ok(outs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, read, write};

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("rarch_arc_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        create_dir_all(&p).unwrap();
        p
    }

    fn make_tree(d: &Path) -> PathBuf {
        let src = d.join("Proje");
        create_dir_all(src.join("alt/derin")).unwrap();
        write(src.join("a.txt"), b"merhaba dunya").unwrap();
        write(src.join("alt/b.bin"), vec![7u8; 100_000]).unwrap();
        write(src.join("alt/derin/c.txt"), "ÇĞİÖŞÜ unicode".as_bytes()).unwrap();
        create_dir_all(src.join("bos")).unwrap();
        src
    }

    fn assert_tree(root: &Path) {
        assert_eq!(read(root.join("Proje/a.txt")).unwrap(), b"merhaba dunya");
        assert_eq!(read(root.join("Proje/alt/b.bin")).unwrap(), vec![7u8; 100_000]);
        assert_eq!(read(root.join("Proje/alt/derin/c.txt")).unwrap(), "ÇĞİÖŞÜ unicode".as_bytes());
        assert!(root.join("Proje/bos").is_dir());
    }

    #[test]
    fn roundtrip_all_multi_formats() {
        for fmt in [
            ArchiveFormat::Zip,
            ArchiveFormat::SevenZ,
            ArchiveFormat::Tar,
            ArchiveFormat::TarGz,
            ArchiveFormat::TarBz2,
            ArchiveFormat::TarXz,
        ] {
            let d = tmp(&format!("rt_{}", fmt.extension().replace('.', "_")));
            let src = make_tree(&d);
            let out = d.join(format!("x.{}", fmt.extension()));
            let o = CompressOptions { format: fmt, level: 5, ..Default::default() };
            compress(&[src.clone()], &out, &o, &Ctx::new()).unwrap_or_else(|e| panic!("{fmt:?} compress: {e}"));
            assert!(out.exists());
            assert_eq!(detect(&out), Some(fmt));

            let entries = list(&out, None).unwrap_or_else(|e| panic!("{fmt:?} list: {e}"));
            let names: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
            assert!(names.contains(&"Proje/a.txt"), "{fmt:?}: {names:?}");
            assert!(names.contains(&"Proje/alt/derin/c.txt"), "{fmt:?}: {names:?}");
            let b = entries.iter().find(|e| e.path == "Proje/alt/b.bin").unwrap();
            assert_eq!(b.size, 100_000);

            let dest = d.join("cikti");
            let ctx = Ctx::new();
            extract(&out, &dest, &[], "", None, Conflict::Overwrite, &ctx).unwrap_or_else(|e| panic!("{fmt:?} extract: {e}"));
            assert_tree(&dest);
            let snap = ctx.progress.snapshot();
            assert_eq!(snap.done_bytes, snap.total_bytes, "{fmt:?} ilerleme");

            let rep = test(&out, None, &Ctx::new()).unwrap();
            assert!(rep.errors.is_empty(), "{fmt:?}: {:?}", rep.errors);
            let _ = fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn single_file_formats() {
        for fmt in [ArchiveFormat::Gz, ArchiveFormat::Bz2, ArchiveFormat::Xz] {
            let d = tmp(&format!("single_{}", fmt.extension()));
            let f = d.join("veri.txt");
            write(&f, "tek dosya içeriği ".repeat(1000)).unwrap();
            let out = d.join(format!("veri.txt.{}", fmt.extension()));
            let o = CompressOptions { format: fmt, ..Default::default() };
            compress(&[f.clone()], &out, &o, &Ctx::new()).unwrap_or_else(|e| panic!("{fmt:?}: {e}"));
            let dest = d.join("o");
            extract(&out, &dest, &[], "", None, Conflict::Overwrite, &Ctx::new()).unwrap();
            assert_eq!(read(dest.join("veri.txt")).unwrap(), read(&f).unwrap());
            assert!(test(&out, None, &Ctx::new()).unwrap().errors.is_empty());
            // klasör verilirse hata
            assert!(compress(&[d.clone()], &d.join("k.gz"), &o, &Ctx::new()).is_err());
            let _ = fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn passwords_zip_and_7z() {
        for fmt in [ArchiveFormat::Zip, ArchiveFormat::SevenZ] {
            let d = tmp(&format!("pw_{}", fmt.extension()));
            let src = make_tree(&d);
            let out = d.join(format!("s.{}", fmt.extension()));
            let o = CompressOptions { format: fmt, password: Some("gizli123".into()), encrypt_names: fmt == ArchiveFormat::SevenZ, ..Default::default() };
            compress(&[src], &out, &o, &Ctx::new()).unwrap_or_else(|e| panic!("{fmt:?}: {e}"));

            let dest = d.join("o");
            if fmt == ArchiveFormat::SevenZ {
                // şifreli başlık: parolasız liste bile açılmamalı
                assert_eq!(list(&out, None), Err(Error::NeedPassword).map(|_: ()| vec![]));
            } else {
                assert!(list(&out, None).unwrap().iter().any(|e| e.encrypted));
            }
            let r = extract(&out, &dest, &[], "", None, Conflict::Overwrite, &Ctx::new());
            assert!(matches!(r, Err(Error::NeedPassword)), "{fmt:?}: {r:?}");
            let r = extract(&out, &dest, &[], "", Some("yanlis"), Conflict::Overwrite, &Ctx::new());
            assert!(matches!(r, Err(Error::WrongPassword)), "{fmt:?}: {r:?}");
            let _ = fs::remove_dir_all(&dest);
            extract(&out, &dest, &[], "", Some("gizli123"), Conflict::Overwrite, &Ctx::new()).unwrap_or_else(|e| panic!("{fmt:?}: {e}"));
            assert_tree(&dest);
            assert!(test(&out, Some("gizli123"), &Ctx::new()).unwrap().errors.is_empty());
            let _ = fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn selective_extract_with_base() {
        let d = tmp("sel");
        let src = make_tree(&d);
        let out = d.join("s.zip");
        compress(&[src], &out, &CompressOptions::default(), &Ctx::new()).unwrap();
        // "Proje/alt" klasöründeyken "derin" seçildi
        let dest = d.join("o");
        extract(&out, &dest, &["Proje/alt/derin".to_string()], "Proje/alt", None, Conflict::Skip, &Ctx::new()).unwrap();
        assert!(dest.join("derin/c.txt").exists());
        assert!(!dest.join("derin/../b.bin").exists());
        assert!(!dest.join("a.txt").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn conflict_policies_on_extract() {
        let d = tmp("conf");
        let src = make_tree(&d);
        let out = d.join("s.tar.gz");
        let o = CompressOptions { format: ArchiveFormat::TarGz, ..Default::default() };
        compress(&[src], &out, &o, &Ctx::new()).unwrap();
        let dest = d.join("o");
        create_dir_all(dest.join("Proje")).unwrap();
        write(dest.join("Proje/a.txt"), b"ESKI").unwrap();
        extract(&out, &dest, &[], "", None, Conflict::Skip, &Ctx::new()).unwrap();
        assert_eq!(read(dest.join("Proje/a.txt")).unwrap(), b"ESKI");
        extract(&out, &dest, &[], "", None, Conflict::KeepBoth, &Ctx::new()).unwrap();
        assert_eq!(read(dest.join("Proje/a (1).txt")).unwrap(), b"merhaba dunya");
        extract(&out, &dest, &[], "", None, Conflict::Overwrite, &Ctx::new()).unwrap();
        assert_eq!(read(dest.join("Proje/a.txt")).unwrap(), b"merhaba dunya");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn safe_join_blocks_traversal() {
        let d = Path::new("/data/out");
        assert_eq!(safe_join(d, "a/b.txt"), Some(PathBuf::from("/data/out/a/b.txt")));
        assert_eq!(safe_join(d, "../evil"), None);
        assert_eq!(safe_join(d, "a/../../evil"), None);
        assert_eq!(safe_join(d, "/etc/passwd"), None); // mutlak yol reddedilir
        assert_eq!(safe_join(d, ""), None);
        assert_eq!(norm_path("/abs/x/"), "abs/x");
        assert_eq!(norm_path("a\\b\\c"), "a/b/c");
    }

    #[test]
    fn cancel_and_cleanup() {
        let d = tmp("cancel");
        let src = make_tree(&d);
        let out = d.join("c.zip");
        let ctx = Ctx::new();
        ctx.cancel.cancel();
        assert_eq!(compress(&[src.clone()], &out, &CompressOptions::default(), &ctx), Err(Error::Cancelled));
        assert!(!out.exists());
        // var olan hedef
        write(&out, b"x").unwrap();
        assert!(matches!(compress(&[src.clone()], &out, &CompressOptions::default(), &Ctx::new()), Err(Error::Exists(_))));
        // kendi içine
        assert!(compress(&[src.clone()], &src.join("i.zip"), &CompressOptions::default(), &Ctx::new()).is_err());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn separate_and_delete_sources() {
        let d = tmp("sep");
        write(d.join("a.txt"), b"1").unwrap();
        write(d.join("b.txt"), b"2").unwrap();
        let o = CompressOptions { format: ArchiveFormat::Zip, delete_sources: true, ..Default::default() };
        let outs = compress_separate(&[d.join("a.txt"), d.join("b.txt")], &d, &o, &Ctx::new()).unwrap();
        assert_eq!(outs.len(), 2);
        assert!(outs[0].ends_with("a.zip") && outs[1].ends_with("b.zip"));
        assert!(!d.join("a.txt").exists() && !d.join("b.txt").exists());
        assert_eq!(default_archive_name(&[d.join("a.zip")], ArchiveFormat::SevenZ), "a.7z");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn magic_detection() {
        let d = tmp("magic");
        let src = make_tree(&d);
        let out = d.join("s.zip");
        compress(&[src], &out, &CompressOptions::default(), &Ctx::new()).unwrap();
        let renamed = d.join("gizli.dat");
        fs::copy(&out, &renamed).unwrap();
        assert_eq!(detect(&renamed), None);
        assert_eq!(detect_by_magic(&renamed), Some(ArchiveFormat::Zip));
        assert!(list(&renamed, None).is_ok()); // "Arşiv olarak aç"
        let _ = fs::remove_dir_all(&d);
    }
}
