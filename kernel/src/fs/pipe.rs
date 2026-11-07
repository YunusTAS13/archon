//! Borular (`pipe`).
//!
//! Boru, VFS'te bir `Inode` olarak temsil edilir: okuma/yazma uçları aynı
//! halka tamponu paylaşır. Böylece mevcut `read`/`write`/`dup2` yolları
//! değişmeden çalışır.

use alloc::collections::VecDeque;
use alloc::sync::Arc;

use super::{FileType, Inode, InodeNumber, Metadata, Result, VfsError};
use crate::sync::SpinLock;

const CAPACITY: usize = 64 * 1024;

struct PipeBuffer {
    data: VecDeque<u8>,
    readers: u32,
    writers: u32,
}

/// Borunun bir ucu. Aynı tamponu paylaşan iki örnek oluşturulur.
pub struct PipeInode {
    buffer: Arc<SpinLock<PipeBuffer>>,
    read_end: bool,
}

impl PipeInode {
    /// `(okuma ucu, yazma ucu)` döndürür.
    pub fn pair() -> (Arc<PipeInode>, Arc<PipeInode>) {
        let buffer = Arc::new(SpinLock::new(PipeBuffer {
            data: VecDeque::with_capacity(4096),
            readers: 1,
            writers: 1,
        }));
        (
            Arc::new(PipeInode {
                buffer: buffer.clone(),
                read_end: true,
            }),
            Arc::new(PipeInode {
                buffer,
                read_end: false,
            }),
        )
    }

    fn current_length(&self) -> u64 {
        self.buffer.lock().data.len() as u64
    }
}

impl Drop for PipeInode {
    fn drop(&mut self) {
        let mut buffer = self.buffer.lock();
        if self.read_end {
            buffer.readers = buffer.readers.saturating_sub(1);
        } else {
            buffer.writers = buffer.writers.saturating_sub(1);
        }
    }
}

impl Inode for PipeInode {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::regular(0 as InodeNumber, self.current_length());
        metadata.file_type = FileType::Fifo;
        metadata
    }

    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            {
                let mut pipe = self.buffer.lock();
                if !pipe.data.is_empty() {
                    let count = buffer.len().min(pipe.data.len());
                    for slot in buffer.iter_mut().take(count) {
                        *slot = pipe.data.pop_front().unwrap();
                    }
                    return Ok(count);
                }
                if pipe.writers == 0 {
                    return Ok(0); // EOF: yazan kalmadı
                }
            }
            if crate::task::signal::interrupted() {
                return Err(VfsError::Interrupted);
            }
            crate::task::schedule();
        }
    }

    fn write_at(&self, _offset: u64, buffer: &[u8]) -> Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            {
                let mut pipe = self.buffer.lock();
                if pipe.readers == 0 {
                    return Err(VfsError::Io); // EPIPE
                }
                let space = CAPACITY.saturating_sub(pipe.data.len());
                if space > 0 {
                    let count = space.min(buffer.len());
                    pipe.data.extend(&buffer[..count]);
                    return Ok(count);
                }
            }
            if crate::task::signal::interrupted() {
                return Err(VfsError::Interrupted);
            }
            crate::task::schedule();
        }
    }

    fn ready(&self, write: bool) -> bool {
        let pipe = self.buffer.lock();
        if write {
            pipe.readers == 0 || pipe.data.len() < CAPACITY
        } else {
            !pipe.data.is_empty() || pipe.writers == 0
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

    fn readdir(&self) -> Result<alloc::vec::Vec<(alloc::string::String, Arc<dyn Inode>)>> {
        Err(VfsError::NotADirectory)
    }

    fn truncate(&self, _size: u64) -> Result<()> {
        self.buffer.lock().data.clear();
        Ok(())
    }
}
