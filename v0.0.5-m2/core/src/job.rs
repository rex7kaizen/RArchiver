//! Uzun işler için iptal ve ilerleme altyapısı (iş parçacıkları arası güvenli).

use crate::error::{Error, Result};
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct Progress {
    total_bytes: AtomicU64,
    done_bytes: AtomicU64,
    total_files: AtomicU64,
    done_files: AtomicU64,
    current: Mutex<String>,
    title: Mutex<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_files: u64,
    pub done_files: u64,
    pub current: String,
    pub title: String,
}

impl ProgressSnapshot {
    /// 0.0..=1.0 (bilinmiyorsa dosya sayısına bakar)
    pub fn fraction(&self) -> f32 {
        if self.total_bytes > 0 {
            (self.done_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
        } else if self.total_files > 0 {
            (self.done_files as f64 / self.total_files as f64).clamp(0.0, 1.0) as f32
        } else {
            0.0
        }
    }
}

impl Progress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn set_totals(&self, bytes: u64, files: u64) {
        self.total_bytes.store(bytes, Ordering::Relaxed);
        self.total_files.store(files, Ordering::Relaxed);
    }
    pub fn add_totals(&self, bytes: u64, files: u64) {
        self.total_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.total_files.fetch_add(files, Ordering::Relaxed);
    }
    pub fn add_bytes(&self, n: u64) {
        self.done_bytes.fetch_add(n, Ordering::Relaxed);
    }
    pub fn file_done(&self) {
        self.done_files.fetch_add(1, Ordering::Relaxed);
    }
    pub fn set_current(&self, s: &str) {
        *self.current.lock().unwrap() = s.to_string();
    }
    pub fn set_title(&self, s: &str) {
        *self.title.lock().unwrap() = s.to_string();
    }
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            done_bytes: self.done_bytes.load(Ordering::Relaxed),
            total_files: self.total_files.load(Ordering::Relaxed),
            done_files: self.done_files.load(Ordering::Relaxed),
            current: self.current.lock().unwrap().clone(),
            title: self.title.lock().unwrap().clone(),
        }
    }
}

#[derive(Clone)]
pub struct Ctx {
    pub progress: Arc<Progress>,
    pub cancel: CancelToken,
}

impl Ctx {
    pub fn new() -> Self {
        Self { progress: Progress::new(), cancel: CancelToken::new() }
    }
    pub fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl Default for Ctx {
    fn default() -> Self {
        Self::new()
    }
}

/// Okunan baytları sayan ve iptali denetleyen sarmalayıcı.
pub struct CountingReader<'a, R> {
    pub inner: R,
    pub ctx: &'a Ctx,
}

impl<R: Read> Read for CountingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.ctx.cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = self.inner.read(buf)?;
        self.ctx.progress.add_bytes(n as u64);
        Ok(n)
    }
}

/// `src`'den `dst`'ye kopyalar; ilerlemeyi sayar, iptali denetler.
pub fn copy_stream<R: Read + ?Sized, W: io::Write>(src: &mut R, dst: &mut W, ctx: &Ctx) -> Result<u64> {
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        ctx.check()?;
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
        ctx.progress.add_bytes(n as u64);
        total += n as u64;
    }
    Ok(total)
}
