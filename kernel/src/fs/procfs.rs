//! `/proc`: süreç ve çekirdek bilgileri.
//!
//! Dosyalar okuma anında üretilir; `self` o anki sürece çözülür.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{FileSystem, FileType, Inode, InodeNumber, Metadata, Result, VfsError};

pub struct ProcFs {
    root: Arc<ProcRoot>,
}

impl ProcFs {
    pub fn new() -> Self {
        Self {
            root: Arc::new(ProcRoot),
        }
    }
}

impl Default for ProcFs {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for ProcFs {
    fn name(&self) -> &'static str {
        "procfs"
    }

    fn root(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }
}

/// İçeriği önceden üretilmiş salt okunur dosya.
struct ProcFile {
    number: InodeNumber,
    content: Vec<u8>,
    symlink: bool,
}

impl ProcFile {
    fn new(number: InodeNumber, content: String) -> Arc<dyn Inode> {
        Arc::new(ProcFile {
            number,
            content: content.into_bytes(),
            symlink: false,
        })
    }

    fn new_symlink(number: InodeNumber, target: String) -> Arc<dyn Inode> {
        Arc::new(ProcFile {
            number,
            content: target.into_bytes(),
            symlink: true,
        })
    }
}

impl Inode for ProcFile {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::regular(self.number, self.content.len() as u64);
        metadata.mode = 0o444;
        if self.symlink {
            metadata.file_type = FileType::Symlink;
        }
        metadata
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let start = offset as usize;
        if start >= self.content.len() {
            return Ok(0);
        }
        let count = buffer.len().min(self.content.len() - start);
        buffer[..count].copy_from_slice(&self.content[start..start + count]);
        Ok(count)
    }

    fn write_at(&self, _offset: u64, _buffer: &[u8]) -> Result<usize> {
        Err(VfsError::ReadOnly)
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>> {
        Err(VfsError::NotADirectory)
    }

    fn create(&self, _name: &str, _file_type: FileType, _mode: u32) -> Result<Arc<dyn Inode>> {
        Err(VfsError::ReadOnly)
    }

    fn unlink(&self, _name: &str) -> Result<()> {
        Err(VfsError::ReadOnly)
    }

    fn readdir(&self) -> Result<Vec<(String, Arc<dyn Inode>)>> {
        Err(VfsError::NotADirectory)
    }

    fn truncate(&self, _size: u64) -> Result<()> {
        Err(VfsError::ReadOnly)
    }
}

/// `/proc/<pid>` dizini.
struct ProcDirectory {
    pid: u64,
}

impl Inode for ProcDirectory {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::directory(1000 + self.pid as InodeNumber);
        metadata.mode = 0o555;
        metadata
    }

    fn read_at(&self, _offset: u64, _buffer: &mut [u8]) -> Result<usize> {
        Err(VfsError::IsADirectory)
    }

    fn write_at(&self, _offset: u64, _buffer: &[u8]) -> Result<usize> {
        Err(VfsError::ReadOnly)
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        let Some(info) = crate::task::snapshot_of(self.pid) else {
            return Err(VfsError::NotFound);
        };
        match name {
            "stat" => Ok(ProcFile::new(1, stat_text(&info))),
            "status" => Ok(ProcFile::new(2, status_text(&info))),
            "cmdline" => Ok(ProcFile::new(3, format!("{}\0", info.name))),
            "comm" => Ok(ProcFile::new(4, format!("{}\n", info.name))),
            "exe" => Ok(ProcFile::new_symlink(5, info.path)),
            "maps" => Ok(ProcFile::new(6, String::new())),
            _ => Err(VfsError::NotFound),
        }
    }

    fn create(&self, _name: &str, _file_type: FileType, _mode: u32) -> Result<Arc<dyn Inode>> {
        Err(VfsError::ReadOnly)
    }

    fn unlink(&self, _name: &str) -> Result<()> {
        Err(VfsError::ReadOnly)
    }

    fn readdir(&self) -> Result<Vec<(String, Arc<dyn Inode>)>> {
        let names = ["stat", "status", "cmdline", "comm", "exe", "maps"];
        let mut entries = Vec::new();
        for name in names {
            if let Ok(inode) = self.lookup(name) {
                entries.push((name.to_string(), inode));
            }
        }
        Ok(entries)
    }

    fn truncate(&self, _size: u64) -> Result<()> {
        Err(VfsError::ReadOnly)
    }
}

fn state_letter(state: u8) -> char {
    match state {
        crate::task::STATE_RUNNING => 'R',
        crate::task::STATE_READY => 'R',
        crate::task::STATE_ZOMBIE => 'Z',
        _ => 'S',
    }
}

fn stat_text(info: &crate::task::ProcessSnapshot) -> String {
    // Linux `stat` alan sırası: pid, comm, state, ppid, ...
    format!(
        "{} ({}) {} {} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 {} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
        info.pid,
        info.name,
        state_letter(info.state),
        info.ppid,
        info.user_stack,
    )
}

fn status_text(info: &crate::task::ProcessSnapshot) -> String {
    format!(
        "Name:\t{}\nState:\t{}\nTgid:\t{}\nPid:\t{}\nPPid:\t{}\nUid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\nThreads:\t1\n",
        info.name,
        match state_letter(info.state) {
            'R' => "R (running)",
            'Z' => "Z (zombie)",
            _ => "S (sleeping)",
        },
        info.pid,
        info.pid,
        info.ppid,
    )
}

fn memory_text() -> String {
    let page = 4096u64;
    let total = crate::mem::pmm::total_frames() as u64 * page;
    let free = crate::mem::pmm::free_frames() as u64 * page;
    format!(
        "MemTotal:       {} kB\nMemFree:        {} kB\nMemAvailable:   {} kB\nBuffers:        0 kB\nCached:         0 kB\nSwapTotal:      0 kB\nSwapFree:       0 kB\n",
        total / 1024,
        free / 1024,
        free / 1024,
    )
}

fn cpu_text() -> String {
    format!(
        "processor\t: 0\nvendor_id\t: Archon\nmodel name\t: {}\ncpu MHz\t\t: {}\ncache size\t: 0 KB\nflags\t\t: fpu tsc msr apic\n",
        crate::arch::cpu::brand_string(),
        crate::time::tsc_frequency() / 1_000_000,
    )
}

fn uptime_text() -> String {
    let millis = crate::time::uptime_ms();
    format!("{}.{:02} 0.00\n", millis / 1000, (millis % 1000) / 10)
}

fn mounts_text() -> String {
    let mut text = String::new();
    for (prefix, name) in crate::fs::mount_list() {
        text.push_str(&format!("{} {} {} rw 0 0\n", name, prefix, name));
    }
    text
}

struct ProcRoot;

impl Inode for ProcRoot {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::directory(1);
        metadata.mode = 0o555;
        metadata
    }

    fn read_at(&self, _offset: u64, _buffer: &mut [u8]) -> Result<usize> {
        Err(VfsError::IsADirectory)
    }

    fn write_at(&self, _offset: u64, _buffer: &[u8]) -> Result<usize> {
        Err(VfsError::ReadOnly)
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        match name {
            "self" => {
                let pid = crate::task::current_pid();
                Ok(Arc::new(ProcDirectory { pid }))
            }
            "version" => Ok(ProcFile::new(
                10,
                "Linux version 6.6.0-archon (archon@localhost) #1 SMP PREEMPT x86_64\n".to_string(),
            )),
            "uptime" => Ok(ProcFile::new(11, uptime_text())),
            "meminfo" => Ok(ProcFile::new(12, memory_text())),
            "cpuinfo" => Ok(ProcFile::new(13, cpu_text())),
            "mounts" => Ok(ProcFile::new(14, mounts_text())),
            "filesystems" => Ok(ProcFile::new(
                15,
                "nodev\text4\nnodev\tramfs\nnodev\tdevfs\nnodev\tproc\nnodev\tpipefs\n".to_string(),
            )),
            "loadavg" => Ok(ProcFile::new(16, "0.00 0.00 0.00 1/1 1\n".to_string())),
            "cmdline" => Ok(ProcFile::new(17, String::new())),
            "sys" => Ok(ProcFile::new(18, String::new())),
            other => match other.parse::<u64>() {
                Ok(pid) if crate::task::snapshot_of(pid).is_some() => {
                    Ok(Arc::new(ProcDirectory { pid }))
                }
                _ => Err(VfsError::NotFound),
            },
        }
    }

    fn create(&self, _name: &str, _file_type: FileType, _mode: u32) -> Result<Arc<dyn Inode>> {
        Err(VfsError::ReadOnly)
    }

    fn unlink(&self, _name: &str) -> Result<()> {
        Err(VfsError::ReadOnly)
    }

    fn readdir(&self) -> Result<Vec<(String, Arc<dyn Inode>)>> {
        let mut entries = Vec::new();
        for name in [
            "self",
            "version",
            "uptime",
            "meminfo",
            "cpuinfo",
            "mounts",
            "filesystems",
            "loadavg",
        ] {
            if let Ok(inode) = self.lookup(name) {
                entries.push((name.to_string(), inode));
            }
        }
        for info in crate::task::snapshot() {
            entries.push((format!("{}", info.pid), Arc::new(ProcDirectory { pid: info.pid })));
        }
        Ok(entries)
    }

    fn truncate(&self, _size: u64) -> Result<()> {
        Err(VfsError::ReadOnly)
    }
}
