//! `epoll`: olay döngüleri için hazırlık toplayıcı.
//!
//! Kayıtlar yalnızca `(fd, events, data)` üçlüsüdür; hazırlık her çağrıda
//! hedef inode'un `ready()` yöntemiyle yeniden sorulur (seviye tetiklemeli).
//! Böylece `epoll` durumu ile gerçek tampon durumu arasında kopya tutulmaz.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{FileType, Inode, InodeNumber, Metadata, Result, VfsError};
use crate::sync::SpinLock;

#[derive(Clone, Copy)]
pub struct EpollEntry {
    pub descriptor: i32,
    pub events: u32,
    pub data: u64,
}

pub struct EpollInode {
    entries: SpinLock<Vec<EpollEntry>>,
}

impl EpollInode {
    pub fn new() -> Self {
        Self {
            entries: SpinLock::new(Vec::new()),
        }
    }

    pub fn add(&self, descriptor: i32, events: u32, data: u64) -> Result<()> {
        let mut entries = self.entries.lock();
        if entries.iter().any(|entry| entry.descriptor == descriptor) {
            return Err(VfsError::AlreadyExists);
        }
        entries.push(EpollEntry {
            descriptor,
            events,
            data,
        });
        Ok(())
    }

    pub fn modify(&self, descriptor: i32, events: u32, data: u64) -> Result<()> {
        let mut entries = self.entries.lock();
        match entries
            .iter_mut()
            .find(|entry| entry.descriptor == descriptor)
        {
            Some(entry) => {
                entry.events = events;
                entry.data = data;
                Ok(())
            }
            None => Err(VfsError::NotFound),
        }
    }

    pub fn remove(&self, descriptor: i32) -> Result<()> {
        let mut entries = self.entries.lock();
        let before = entries.len();
        entries.retain(|entry| entry.descriptor != descriptor);
        if entries.len() == before {
            return Err(VfsError::NotFound);
        }
        Ok(())
    }

    pub fn entries(&self) -> Vec<EpollEntry> {
        self.entries.lock().clone()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }
}

impl Default for EpollInode {
    fn default() -> Self {
        Self::new()
    }
}

impl Inode for EpollInode {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::regular(0 as InodeNumber, 0);
        metadata.mode = 0o600;
        metadata
    }

    fn read_at(&self, _offset: u64, _buffer: &mut [u8]) -> Result<usize> {
        Err(VfsError::InvalidArgument)
    }

    fn write_at(&self, _offset: u64, _buffer: &[u8]) -> Result<usize> {
        Err(VfsError::InvalidArgument)
    }

    fn ready(&self, _write: bool) -> bool {
        !self.is_empty()
    }

    fn as_epoll(&self) -> Option<&EpollInode> {
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
