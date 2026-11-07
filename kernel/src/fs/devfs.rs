//! `/dev` düğümleri: null, zero, full, random, urandom, console, tty.
//!
//! Gerçek aygıt düğümleri yerine VFS üzerinde sanal `Inode`'lar sunulur;
//! böylece `open`/`read`/`write`/`stat` yolları değişmeden çalışır.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{FileSystem, FileType, Inode, InodeNumber, Metadata, Result, VfsError};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Null,
    Zero,
    Full,
    Random,
    Console,
    Framebuffer,
    Keyboard,
    Input,
}

pub struct DevFs {
    root: Arc<DeviceDirectory>,
}

impl DevFs {
    pub fn new() -> Self {
        let mut children = Vec::new();
        let pty = super::pty::Pty::new();
        for (name, kind) in [
            ("null", Kind::Null),
            ("zero", Kind::Zero),
            ("full", Kind::Full),
            ("random", Kind::Random),
            ("urandom", Kind::Random),
            ("console", Kind::Console),
            ("fb0", Kind::Framebuffer),
            ("kbd", Kind::Keyboard),
        ] {
            children.push((
                String::from(name),
                Arc::new(Device { kind }) as Arc<dyn Inode>,
            ));
        }
        // `/dev/tty`: kontrol uçbirimi olarak pty kölesine bağlanır.
        children.push((String::from("tty"), pty.slave() as Arc<dyn Inode>));
        // Sözde uçbirim: `/dev/ptmx` ana taraf, `/dev/pts/0` köle taraf.
        children.push((
            String::from("ptmx"),
            pty.master() as Arc<dyn Inode>,
        ));
        children.push((
            String::from("pts"),
            Arc::new(DeviceDirectory {
                children: alloc::vec![(String::from("0"), pty.slave() as Arc<dyn Inode>)],
            }) as Arc<dyn Inode>,
        ));
        // `/dev/input/event0`: SDL ve diğer araçların beklediği evdev arayüzü.
        children.push((
            String::from("input"),
            Arc::new(DeviceDirectory {
                children: alloc::vec![(
                    String::from("event0"),
                    Arc::new(Device { kind: Kind::Input }) as Arc<dyn Inode>,
                )],
            }) as Arc<dyn Inode>,
        ));
        Self {
            root: Arc::new(DeviceDirectory { children }),
        }
    }
}

impl Default for DevFs {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for DevFs {
    fn name(&self) -> &'static str {
        "devfs"
    }

    fn root(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }
}

struct DeviceDirectory {
    children: Vec<(String, Arc<dyn Inode>)>,
}

impl Inode for DeviceDirectory {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::directory(1 as InodeNumber);
        metadata.mode = 0o755;
        metadata
    }

    fn read_at(&self, _offset: u64, _buffer: &mut [u8]) -> Result<usize> {
        Err(VfsError::NotADirectory)
    }

    fn write_at(&self, _offset: u64, _buffer: &[u8]) -> Result<usize> {
        Err(VfsError::NotADirectory)
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        self.children
            .iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, inode)| inode.clone())
            .ok_or(VfsError::NotFound)
    }

    fn create(&self, _name: &str, _file_type: FileType, _mode: u32) -> Result<Arc<dyn Inode>> {
        Err(VfsError::ReadOnly)
    }

    fn unlink(&self, _name: &str) -> Result<()> {
        Err(VfsError::ReadOnly)
    }

    fn readdir(&self) -> Result<Vec<(String, Arc<dyn Inode>)>> {
        Ok(self.children.clone())
    }

    fn truncate(&self, _size: u64) -> Result<()> {
        Err(VfsError::ReadOnly)
    }
}

struct Device {
    kind: Kind,
}

impl Inode for Device {
    fn metadata(&self) -> Metadata {
        let number = match self.kind {
            Kind::Null => 2,
            Kind::Zero => 3,
            Kind::Full => 4,
            Kind::Random => 5,
            Kind::Console => 6,
            Kind::Framebuffer => 7,
            Kind::Keyboard => 8,
            Kind::Input => 9,
        };
        let mut metadata = Metadata::regular(number as InodeNumber, 0);
        metadata.file_type = FileType::CharDevice;
        metadata.mode = 0o666;
        metadata
    }

    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize> {
        match self.kind {
            Kind::Null => Ok(0),
            Kind::Full => Ok(0),
            Kind::Zero => {
                for byte in buffer.iter_mut() {
                    *byte = 0;
                }
                Ok(buffer.len())
            }
            Kind::Random => {
                let mut state = crate::arch::cpu::rdtsc();
                for byte in buffer.iter_mut() {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    *byte = state as u8;
                }
                Ok(buffer.len())
            }
            Kind::Console => crate::fs::console_read(buffer),
            Kind::Keyboard => {
                loop {
                    if let Some(value) = crate::drivers::ps2::read_char() {
                        buffer[0] = value;
                        return Ok(1);
                    }
                    crate::task::schedule();
                }
            }
            Kind::Input => {
                if buffer.len() < 24 {
                    return Err(VfsError::InvalidArgument);
                }
                loop {
                    if let Some(event) = crate::drivers::evdev::pop_event() {
                        buffer[0..8].copy_from_slice(&event.seconds.to_le_bytes());
                        buffer[8..16].copy_from_slice(&event.micros.to_le_bytes());
                        buffer[16..18].copy_from_slice(&event.kind.to_le_bytes());
                        buffer[18..20].copy_from_slice(&event.code.to_le_bytes());
                        buffer[20..24].copy_from_slice(&event.value.to_le_bytes());
                        return Ok(24);
                    }
                    if crate::task::signal::interrupted() {
                        return Err(VfsError::Interrupted);
                    }
                    crate::task::schedule();
                }
            }
            Kind::Framebuffer => Err(VfsError::Unsupported),
        }
    }

    fn write_at(&self, _offset: u64, buffer: &[u8]) -> Result<usize> {
        match self.kind {
            Kind::Null | Kind::Zero => Ok(buffer.len()),
            Kind::Full => Err(VfsError::NoSpace),
            Kind::Random => Ok(buffer.len()),
            Kind::Console => {
                crate::log::write_bytes(buffer);
                Ok(buffer.len())
            }
            Kind::Framebuffer | Kind::Keyboard | Kind::Input => Err(VfsError::Unsupported),
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

    fn ready(&self, write: bool) -> bool {
        match self.kind {
            Kind::Console => write || crate::fs::console_ready(),
            Kind::Keyboard => crate::drivers::ps2::pending() > 0,
            Kind::Input => crate::drivers::evdev::pending() > 0,
            Kind::Full => write,
            _ => true,
        }
    }

    fn mmap_notify(&self, mapped: bool) {
        if !matches!(self.kind, Kind::Framebuffer) {
            return;
        }
        let pid = crate::task::current_pid() as i32;
        if mapped {
            crate::console::acquire_framebuffer(pid);
        } else {
            crate::console::release_framebuffer(pid);
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self.kind, Kind::Console)
    }

    fn physical_region(&self) -> Option<(u64, u64)> {
        match self.kind {
            Kind::Framebuffer => crate::console::framebuffer_region(),
            _ => None,
        }
    }

    fn ioctl(&self, request: u64, argument: u64) -> Option<i64> {
        match self.kind {
            Kind::Framebuffer => crate::console::framebuffer_ioctl(request, argument),
            Kind::Input => crate::drivers::evdev::ioctl(request, argument),
            _ => None,
        }
    }
}
