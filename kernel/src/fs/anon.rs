//! Anonim `Inode` türleri: `eventfd`, `timerfd`, `memfd` ve `socketpair`.
//!
//! Hepsi VFS düğümü olarak yaşar; böylece mevcut `read`/`write`/`poll`/`mmap`
//! yolları değişmeden çalışır. Hazırlık durumu `Inode::ready` üzerinden
//! bildirilir, engelleme syscall katmanında `schedule()` ile yapılır.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::{FileType, Inode, InodeNumber, Metadata, Result, VfsError};
use crate::sync::SpinLock;

/// `eventfd` sayacı: 8 baytlık okuma/yazma.
pub struct EventFdInode {
    counter: AtomicU64,
    semaphore: bool,
}

impl EventFdInode {
    pub fn new(initial: u64, semaphore: bool) -> Self {
        Self {
            counter: AtomicU64::new(initial),
            semaphore,
        }
    }
}

impl Inode for EventFdInode {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::regular(0 as InodeNumber, 8);
        metadata.mode = 0o600;
        metadata
    }

    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if buffer.len() < 8 {
            return Err(VfsError::InvalidArgument);
        }
        loop {
            let current = self.counter.load(Ordering::Relaxed);
            if current > 0 {
                let value = if self.semaphore {
                    self.counter.fetch_sub(1, Ordering::Relaxed);
                    1
                } else {
                    self.counter.store(0, Ordering::Relaxed);
                    current
                };
                buffer[..8].copy_from_slice(&value.to_le_bytes());
                return Ok(8);
            }
            if crate::task::signal::interrupted() {
                return Err(VfsError::Interrupted);
            }
            crate::task::schedule();
        }
    }

    fn write_at(&self, _offset: u64, buffer: &[u8]) -> Result<usize> {
        if buffer.len() < 8 {
            return Err(VfsError::InvalidArgument);
        }
        let value = u64::from_le_bytes([
            buffer[0], buffer[1], buffer[2], buffer[3], buffer[4], buffer[5], buffer[6], buffer[7],
        ]);
        if value == u64::MAX {
            return Err(VfsError::InvalidArgument);
        }
        self.counter.fetch_add(value, Ordering::Relaxed);
        Ok(8)
    }

    fn ready(&self, write: bool) -> bool {
        if write {
            self.counter.load(Ordering::Relaxed) < u64::MAX - 1
        } else {
            self.counter.load(Ordering::Relaxed) > 0
        }
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>> {
        Err(VfsError::NotADirectory)
    }
    fn create(&self, _name: &str, _file_type: FileType, _mode: u32) -> Result<Arc<dyn Inode>> {
        Err(VfsError::NotADirectory)
    }
    fn unlink(&self, _name: &str) -> Result<()> {
        Err(VfsError::NotADirectory)
    }
    fn readdir(&self) -> Result<Vec<(String, Arc<dyn Inode>)>> {
        Err(VfsError::NotADirectory)
    }
    fn truncate(&self, _size: u64) -> Result<()> {
        Err(VfsError::InvalidArgument)
    }
}

/// `timerfd`: `settime` ile kurulan tek atımlık/aralıklı zamanlayıcı.
pub struct TimerFdInode {
    /// Bitiş anı (milisaniye, monotonik). 0 = kurulu değil.
    deadline: AtomicU64,
    /// Aralık (milisaniye). 0 = tek atım.
    interval: AtomicU64,
    /// Okunmayı bekleyen tetiklenme sayısı.
    expirations: AtomicU64,
}

impl TimerFdInode {
    pub fn new() -> Self {
        Self {
            deadline: AtomicU64::new(0),
            interval: AtomicU64::new(0),
            expirations: AtomicU64::new(0),
        }
    }

    pub fn arm(&self, value_ms: u64, interval_ms: u64) {
        self.interval.store(interval_ms, Ordering::Relaxed);
        self.expirations.store(0, Ordering::Relaxed);
        if value_ms == 0 {
            self.deadline.store(0, Ordering::Relaxed);
        } else {
            self.deadline
                .store(crate::time::uptime_ms() + value_ms, Ordering::Relaxed);
        }
    }

    pub fn values(&self) -> (u64, u64) {
        let deadline = self.deadline.load(Ordering::Relaxed);
        let now = crate::time::uptime_ms();
        let remaining = deadline.saturating_sub(now);
        (remaining, self.interval.load(Ordering::Relaxed))
    }

    /// Süresi dolduysa sayacı bir artırır.
    fn tick(&self) {
        let deadline = self.deadline.load(Ordering::Relaxed);
        if deadline == 0 {
            return;
        }
        let now = crate::time::uptime_ms();
        if now < deadline {
            return;
        }
        let interval = self.interval.load(Ordering::Relaxed);
        if interval == 0 {
            self.deadline.store(0, Ordering::Relaxed);
            self.expirations.fetch_add(1, Ordering::Relaxed);
        } else {
            let late = (now - deadline) / interval;
            self.expirations.fetch_add(late + 1, Ordering::Relaxed);
            self.deadline
                .store(deadline + (late + 1) * interval, Ordering::Relaxed);
        }
    }
}

impl Default for TimerFdInode {
    fn default() -> Self {
        Self::new()
    }
}

impl Inode for TimerFdInode {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::regular(0 as InodeNumber, 8);
        metadata.mode = 0o600;
        metadata
    }

    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if buffer.len() < 8 {
            return Err(VfsError::InvalidArgument);
        }
        loop {
            self.tick();
            let pending = self.expirations.swap(0, Ordering::Relaxed);
            if pending > 0 {
                buffer[..8].copy_from_slice(&pending.to_le_bytes());
                return Ok(8);
            }
            if crate::task::signal::interrupted() {
                return Err(VfsError::Interrupted);
            }
            crate::task::schedule();
        }
    }

    fn write_at(&self, _offset: u64, _buffer: &[u8]) -> Result<usize> {
        Err(VfsError::InvalidArgument)
    }

    fn ready(&self, write: bool) -> bool {
        if write {
            return false;
        }
        self.tick();
        self.expirations.load(Ordering::Relaxed) > 0
    }

    fn as_timerfd(&self) -> Option<&TimerFdInode> {
        Some(self)
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>> {
        Err(VfsError::NotADirectory)
    }
    fn create(&self, _name: &str, _file_type: FileType, _mode: u32) -> Result<Arc<dyn Inode>> {
        Err(VfsError::NotADirectory)
    }
    fn unlink(&self, _name: &str) -> Result<()> {
        Err(VfsError::NotADirectory)
    }
    fn readdir(&self) -> Result<Vec<(String, Arc<dyn Inode>)>> {
        Err(VfsError::NotADirectory)
    }
    fn truncate(&self, _size: u64) -> Result<()> {
        Err(VfsError::InvalidArgument)
    }
}

/// `memfd`/`shm` sayfaları: fiziksel çerçeveler inode'a aittir ve `MAP_SHARED`
/// eşlemelerinde birden çok süreç aynı çerçeveleri görür.
pub struct MemFdInode {
    name: SpinLock<String>,
    size: AtomicU64,
    frames: SpinLock<Vec<Option<u64>>>,
}

impl MemFdInode {
    pub fn new(name: &str) -> Self {
        Self {
            name: SpinLock::new(String::from(name)),
            size: AtomicU64::new(0),
            frames: SpinLock::new(Vec::new()),
        }
    }

    pub fn name(&self) -> String {
        self.name.lock().clone()
    }

    /// Sayfanın fiziksel çerçevesini döndürür; yoksa ayırır.
    pub fn frame(&self, index: u64) -> Option<u64> {
        let mut frames = self.frames.lock();
        let index = index as usize;
        while frames.len() <= index {
            frames.push(None);
        }
        if let Some(frame) = frames[index] {
            return Some(frame);
        }
        let frame = crate::mem::pmm::alloc_frame()?;
        unsafe { crate::mem::hhdm::zero_physical(frame, 4096) };
        frames[index] = Some(frame);
        register_shared(frame);
        Some(frame)
    }
}

/// Paylaşımlı çerçeveler: `munmap` bunları serbest bırakmamalı.
static SHARED_FRAMES: SpinLock<Vec<u64>> = SpinLock::new(Vec::new());

fn register_shared(frame: u64) {
    SHARED_FRAMES.lock().push(frame);
}

pub fn is_shared_frame(frame: u64) -> bool {
    SHARED_FRAMES.lock().contains(&frame)
}

impl Inode for MemFdInode {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::regular(0 as InodeNumber, self.size.load(Ordering::Relaxed));
        metadata.mode = 0o600;
        metadata
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let size = self.size.load(Ordering::Relaxed);
        if offset >= size {
            return Ok(0);
        }
        let count = ((size - offset) as usize).min(buffer.len());
        let mut done = 0;
        while done < count {
            let position = offset + done as u64;
            let index = position / 4096;
            let within = (position % 4096) as usize;
            let frame = match self.frame(index) {
                Some(frame) => frame,
                None => return Err(VfsError::NoSpace),
            };
            let chunk = (4096 - within).min(count - done);
            let source = unsafe {
                core::slice::from_raw_parts(
                    crate::mem::hhdm::phys_to_ptr::<u8>(frame).add(within),
                    chunk,
                )
            };
            buffer[done..done + chunk].copy_from_slice(source);
            done += chunk;
        }
        Ok(count)
    }

    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<usize> {
        let needed = offset + buffer.len() as u64;
        if needed > self.size.load(Ordering::Relaxed) {
            self.size.store(needed, Ordering::Relaxed);
        }
        let mut done = 0;
        while done < buffer.len() {
            let position = offset + done as u64;
            let index = position / 4096;
            let within = (position % 4096) as usize;
            let frame = match self.frame(index) {
                Some(frame) => frame,
                None => return Err(VfsError::NoSpace),
            };
            let chunk = (4096 - within).min(buffer.len() - done);
            let destination = unsafe {
                core::slice::from_raw_parts_mut(
                    crate::mem::hhdm::phys_to_ptr::<u8>(frame).add(within),
                    chunk,
                )
            };
            destination.copy_from_slice(&buffer[done..done + chunk]);
            done += chunk;
        }
        Ok(buffer.len())
    }

    fn truncate(&self, size: u64) -> Result<()> {
        self.size.store(size, Ordering::Relaxed);
        let pages = (size + 4095) / 4096;
        let mut frames = self.frames.lock();
        while (frames.len() as u64) < pages {
            frames.push(None);
        }
        Ok(())
    }

    fn shared_frame(&self, index: u64) -> Option<u64> {
        self.frame(index)
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>> {
        Err(VfsError::NotADirectory)
    }
    fn create(&self, _name: &str, _file_type: FileType, _mode: u32) -> Result<Arc<dyn Inode>> {
        Err(VfsError::NotADirectory)
    }
    fn unlink(&self, _name: &str) -> Result<()> {
        Err(VfsError::NotADirectory)
    }
    fn readdir(&self) -> Result<Vec<(String, Arc<dyn Inode>)>> {
        Err(VfsError::NotADirectory)
    }
}
