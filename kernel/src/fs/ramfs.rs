//! Bellek içi dosya sistemi: initramfs ve geçici dosyalar.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{FileSystem, FileType, Inode, InodeNumber, Metadata, Result, VfsError};
use crate::sync::SpinLock;

const BLOCK_SIZE: usize = 4096;

static NEXT_INODE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(2);

fn next_inode() -> InodeNumber {
    NEXT_INODE.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

enum Content {
    File(Vec<u8>),
    Directory(BTreeMap<String, Arc<RamInode>>),
    Link(String),
}

pub struct RamInode {
    number: InodeNumber,
    content: SpinLock<Content>,
    metadata: SpinLock<Metadata>,
}

impl RamInode {
    fn new_file(number: InodeNumber) -> Self {
        Self {
            number,
            content: SpinLock::new(Content::File(Vec::new())),
            metadata: SpinLock::new(Metadata::regular(number, 0)),
        }
    }

    fn new_directory(number: InodeNumber) -> Self {
        Self {
            number,
            content: SpinLock::new(Content::Directory(BTreeMap::new())),
            metadata: SpinLock::new(Metadata::directory(number)),
        }
    }

    fn new_symlink(number: InodeNumber, target: String) -> Self {
        let mut metadata = Metadata::regular(number, target.len() as u64);
        metadata.file_type = FileType::Symlink;
        metadata.mode = 0o777;
        Self {
            number,
            content: SpinLock::new(Content::Link(target)),
            metadata: SpinLock::new(metadata),
        }
    }

    fn update_size(&self) {
        let size = match &*self.content.lock() {
            Content::File(data) => data.len() as u64,
            Content::Directory(entries) => entries.len() as u64 * 32,
            Content::Link(target) => target.len() as u64,
        };
        let mut metadata = self.metadata.lock();
        metadata.size = size;
        metadata.blocks = (size + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64;
    }
}

fn read_slice(data: &[u8], offset: u64, buffer: &mut [u8]) -> usize {
    let offset = offset as usize;
    if offset >= data.len() {
        return 0;
    }
    let available = &data[offset..];
    let count = available.len().min(buffer.len());
    buffer[..count].copy_from_slice(&available[..count]);
    count
}

fn write_slice(data: &mut Vec<u8>, offset: u64, buffer: &[u8]) -> usize {
    let offset = offset as usize;
    if offset + buffer.len() > data.len() {
        data.resize(offset + buffer.len(), 0);
    }
    data[offset..offset + buffer.len()].copy_from_slice(buffer);
    buffer.len()
}

impl Inode for RamInode {
    fn metadata(&self) -> Metadata {
        *self.metadata.lock()
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        match &*self.content.lock() {
            Content::File(data) => Ok(read_slice(data, offset, buffer)),
            Content::Link(target) => Ok(read_slice(target.as_bytes(), offset, buffer)),
            Content::Directory(_) => Err(VfsError::IsADirectory),
        }
    }

    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<usize> {
        let written = match &mut *self.content.lock() {
            Content::File(data) => write_slice(data, offset, buffer),
            Content::Link(_) => return Err(VfsError::Unsupported),
            Content::Directory(_) => return Err(VfsError::IsADirectory),
        };
        self.update_size();
        self.metadata.lock().modify_time = 0;
        Ok(written)
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        let content = self.content.lock();
        match &*content {
            Content::Directory(entries) => entries
                .get(name)
                .cloned()
                .map(|inode| inode as Arc<dyn Inode>)
                .ok_or(VfsError::NotFound),
            _ => Err(VfsError::NotADirectory),
        }
    }

    fn create(&self, name: &str, file_type: FileType, mode: u32) -> Result<Arc<dyn Inode>> {
        let mut content = self.content.lock();
        let entries = match &mut *content {
            Content::Directory(entries) => entries,
            _ => return Err(VfsError::NotADirectory),
        };

        if entries.contains_key(name) {
            return Err(VfsError::AlreadyExists);
        }

        let number = next_inode();
        let inode: Arc<RamInode> = match file_type {
            FileType::Directory => Arc::new(RamInode::new_directory(number)),
            FileType::Symlink => Arc::new(RamInode::new_symlink(number, String::new())),
            _ => Arc::new(RamInode::new_file(number)),
        };
        {
            let mut metadata = inode.metadata.lock();
            metadata.mode = mode;
            metadata.file_type = file_type;
        }

        entries.insert(name.to_string(), inode.clone());
        drop(content);
        self.update_size();
        Ok(inode as Arc<dyn Inode>)
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let mut content = self.content.lock();
        let entries = match &mut *content {
            Content::Directory(entries) => entries,
            _ => return Err(VfsError::NotADirectory),
        };
        entries.remove(name).ok_or(VfsError::NotFound)?;
        drop(content);
        self.update_size();
        Ok(())
    }

    fn readdir(&self) -> Result<Vec<(String, Arc<dyn Inode>)>> {
        let content = self.content.lock();
        match &*content {
            Content::Directory(entries) => Ok(entries
                .iter()
                .map(|(name, inode)| (name.clone(), inode.clone() as Arc<dyn Inode>))
                .collect()),
            _ => Err(VfsError::NotADirectory),
        }
    }

    fn truncate(&self, size: u64) -> Result<()> {
        match &mut *self.content.lock() {
            Content::File(data) => {
                data.resize(size as usize, 0);
            }
            Content::Directory(_) | Content::Link(_) => return Err(VfsError::Unsupported),
        }
        self.update_size();
        Ok(())
    }
}

pub struct RamFs {
    root: Arc<RamInode>,
}

impl RamFs {
    pub fn new() -> Self {
        Self {
            root: Arc::new(RamInode::new_directory(1)),
        }
    }

    pub fn root_inode(&self) -> Arc<RamInode> {
        self.root.clone()
    }
}

impl Default for RamFs {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for RamFs {
    fn name(&self) -> &'static str {
        "ramfs"
    }

    fn root(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }
}
