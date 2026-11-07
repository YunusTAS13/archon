//! Sözde uçbirim (pty): `/dev/ptmx` (ana taraf) ve `/dev/pts/0` (köle taraf).
//!
//! Uçbirim öykünücüsü ana taraftan yazar, kabuk köle taraftan okur. Satır
//! disiplini asgari düzeyde: `ECHO` ve `ICRNL` bayraklarına uyulur, kanonik
//! kipte satır tamponlanır.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{FileType, Inode, InodeNumber, Metadata, Result, VfsError};
use crate::sync::SpinLock;

const CAPACITY: usize = 64 * 1024;

const ICRNL: u32 = 0x0100;
const ECHO: u32 = 0x0008;
const ICANON: u32 = 0x0002;
const ISIG: u32 = 0x0001;

const VERASE: u8 = 0x7f;
const VEOF: u8 = 0x04;
const VINTR: u8 = 0x03;
const VQUIT: u8 = 0x1c;
const VSUSP: u8 = 0x1a;

/// Varsayılan `termios`: ICANON|ECHO|ISIG, ICRNL, OPOST|ONLCR.
const DEFAULT_TERMIOS: [u8; 44] = {
    let mut termios = [0u8; 44];
    termios[0] = 0x00;
    termios[1] = 0x05; // ICRNL | IXON
    termios[4] = 0x05; // OPOST | ONLCR
    termios[8] = 0xbf; // CS8 | CREAD | ...
    termios[12] = 0x0b; // ISIG | ICANON | ECHO
    termios[36] = 0x00;
    termios[37] = 0x96; // 38400 baud
    termios[40] = 0x00;
    termios[41] = 0x96;
    termios
};

struct PtyState {
    /// Köleden anaya (program çıktısı).
    to_master: SpinLock<VecDeque<u8>>,
    /// Anadan köleye (kullanıcı girdisi).
    to_slave: SpinLock<VecDeque<u8>>,
    /// Kanonik kip satır tamponu.
    line: SpinLock<VecDeque<u8>>,
    termios: SpinLock<[u8; 44]>,
    master_open: core::sync::atomic::AtomicBool,
    slave_open: core::sync::atomic::AtomicBool,
    /// Ön plandaki süreç grubu (TIOCSPGRP ile ayarlanır).
    fg_pgrp: core::sync::atomic::AtomicU64,
}

impl PtyState {
    fn flag(&self, offset: usize, mask: u32) -> bool {
        let termios = self.termios.lock();
        let value = u32::from_le_bytes([
            termios[offset],
            termios[offset + 1],
            termios[offset + 2],
            termios[offset + 3],
        ]);
        value & mask != 0
    }
}

pub struct Pty {
    state: Arc<PtyState>,
}

pub struct PtyMaster {
    state: Arc<PtyState>,
}

pub struct PtySlave {
    state: Arc<PtyState>,
}

impl Pty {
    pub fn new() -> Self {
        Self {
            state: Arc::new(PtyState {
                to_master: SpinLock::new(VecDeque::new()),
                to_slave: SpinLock::new(VecDeque::new()),
                line: SpinLock::new(VecDeque::new()),
                termios: SpinLock::new(DEFAULT_TERMIOS),
                master_open: core::sync::atomic::AtomicBool::new(false),
                slave_open: core::sync::atomic::AtomicBool::new(false),
                fg_pgrp: core::sync::atomic::AtomicU64::new(0),
            }),
        }
    }

    pub fn master(&self) -> Arc<PtyMaster> {
        self.state
            .master_open
            .store(true, core::sync::atomic::Ordering::Release);
        Arc::new(PtyMaster {
            state: self.state.clone(),
        })
    }

    pub fn slave(&self) -> Arc<PtySlave> {
        self.state
            .slave_open
            .store(true, core::sync::atomic::Ordering::Release);
        Arc::new(PtySlave {
            state: self.state.clone(),
        })
    }
}

impl Default for Pty {
    fn default() -> Self {
        Self::new()
    }
}

/// Ana tarafa yazılan girdiyi köleye (ve gerekirse yankı olarak geri) aktarır.
fn feed_input(state: &PtyState, bytes: &[u8]) {
    let canonical = state.flag(12, ICANON);
    let echo = state.flag(12, ECHO);
    let signals = state.flag(12, ISIG);
    let convert_newline = state.flag(0, ICRNL);

    for byte in bytes {
        let mut value = *byte;
        if convert_newline && value == b'\r' {
            value = b'\n';
        }

        if signals && (value == VINTR || value == VQUIT || value == VSUSP) {
            let signal = if value == VINTR {
                2
            } else if value == VQUIT {
                3
            } else {
                20
            };
            if echo {
                state.to_master.lock().push_back(b'^');
                let letter = match value {
                    VINTR => b'C',
                    VQUIT => b'\\',
                    _ => b'Z',
                };
                state.to_master.lock().push_back(letter);
                state.to_master.lock().push_back(b'\n');
            }
            // Satır tamponunu at, ön plandaki gruba sinyal gönder.
            state.line.lock().clear();
            let group = state
                .fg_pgrp
                .load(core::sync::atomic::Ordering::Relaxed);
            crate::task::signal::broadcast(group, signal);
            continue;
        }

        if canonical {
            if value == VERASE {
                let mut line = state.line.lock();
                line.pop_back();
                drop(line);
                if echo {
                    state.to_master.lock().push_back(VERASE);
                }
                continue;
            }
            if value == VEOF {
                // EOF: biriken satırı hemen teslim et.
                let mut line = state.line.lock();
                let mut target = state.to_slave.lock();
                target.extend(line.drain(..));
                continue;
            }
            let mut line = state.line.lock();
            line.push_back(value);
            let complete = value == b'\n';
            drop(line);
            if echo {
                state.to_master.lock().push_back(value);
            }
            if complete {
                let mut line = state.line.lock();
                let mut target = state.to_slave.lock();
                target.extend(line.drain(..));
            }
        } else {
            if echo {
                state.to_master.lock().push_back(value);
            }
            state.to_slave.lock().push_back(value);
        }
    }
}

fn master_metadata() -> Metadata {
    let mut metadata = Metadata::regular(20 as InodeNumber, 0);
    metadata.file_type = FileType::CharDevice;
    metadata.mode = 0o666;
    metadata
}

fn slave_metadata() -> Metadata {
    let mut metadata = Metadata::regular(21 as InodeNumber, 0);
    metadata.file_type = FileType::CharDevice;
    metadata.mode = 0o666;
    metadata
}

impl Inode for PtyMaster {
    fn metadata(&self) -> Metadata {
        master_metadata()
    }

    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            {
                let mut queue = self.state.to_master.lock();
                if !queue.is_empty() {
                    let count = buffer.len().min(queue.len());
                    for slot in buffer.iter_mut().take(count) {
                        *slot = queue.pop_front().unwrap();
                    }
                    return Ok(count);
                }
            }
            if !self
                .state
                .slave_open
                .load(core::sync::atomic::Ordering::Acquire)
            {
                return Ok(0);
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
        if !self
            .state
            .slave_open
            .load(core::sync::atomic::Ordering::Acquire)
        {
            return Err(VfsError::Io);
        }
        feed_input(&self.state, buffer);
        Ok(buffer.len())
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
        if write {
            self.state
                .slave_open
                .load(core::sync::atomic::Ordering::Acquire)
        } else {
            !self.state.to_master.lock().is_empty()
                || !self
                    .state
                    .slave_open
                    .load(core::sync::atomic::Ordering::Acquire)
        }
    }
}

impl Inode for PtySlave {
    fn metadata(&self) -> Metadata {
        slave_metadata()
    }

    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            {
                let mut queue = self.state.to_slave.lock();
                if !queue.is_empty() {
                    let count = buffer.len().min(queue.len());
                    for slot in buffer.iter_mut().take(count) {
                        *slot = queue.pop_front().unwrap();
                    }
                    return Ok(count);
                }
            }
            if !self
                .state
                .master_open
                .load(core::sync::atomic::Ordering::Acquire)
            {
                return Ok(0);
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
        let mut written = 0;
        while written < buffer.len() {
            {
                let mut queue = self.state.to_master.lock();
                let space = CAPACITY.saturating_sub(queue.len());
                if space > 0 {
                    let count = space.min(buffer.len() - written);
                    queue.extend(&buffer[written..written + count]);
                    written += count;
                    continue;
                }
            }
            if !self
                .state
                .master_open
                .load(core::sync::atomic::Ordering::Acquire)
            {
                return Err(VfsError::Io);
            }
            if crate::task::signal::interrupted() {
                return Err(VfsError::Interrupted);
            }
            crate::task::schedule();
        }
        Ok(written)
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
        if write {
            self.state
                .master_open
                .load(core::sync::atomic::Ordering::Acquire)
        } else {
            !self.state.to_slave.lock().is_empty()
                || !self
                    .state
                    .master_open
                    .load(core::sync::atomic::Ordering::Acquire)
        }
    }

    fn is_terminal(&self) -> bool {
        true
    }

    fn ioctl(&self, request: u64, argument: u64) -> Option<i64> {
        use crate::syscall::copy_to_user;

        match request {
            0x5401 => {
                let termios = *self.state.termios.lock();
                match copy_to_user(argument, &termios[..36]) {
                    Ok(()) => Some(0),
                    Err(code) => Some(code),
                }
            }
            0x802c_542a => {
                let termios = *self.state.termios.lock();
                match copy_to_user(argument, &termios) {
                    Ok(()) => Some(0),
                    Err(code) => Some(code),
                }
            }
            0x5402 | 0x5403 | 0x5404 => {
                let Ok(data) = crate::syscall::copy_from_user(argument, 36) else {
                    return Some(crate::syscall::EFAULT);
                };
                let mut termios = self.state.termios.lock();
                termios[..36].copy_from_slice(&data);
                Some(0)
            }
            0x402c_542b | 0x402c_542c | 0x402c_542d => {
                let Ok(data) = crate::syscall::copy_from_user(argument, 44) else {
                    return Some(crate::syscall::EFAULT);
                };
                let mut termios = self.state.termios.lock();
                termios.copy_from_slice(&data);
                Some(0)
            }
            0x5413 => {
                let mut buffer = [0u8; 8];
                buffer[0..2].copy_from_slice(&24u16.to_le_bytes());
                buffer[2..4].copy_from_slice(&80u16.to_le_bytes());
                match copy_to_user(argument, &buffer) {
                    Ok(()) => Some(0),
                    Err(code) => Some(code),
                }
            }
            0x5414 => Some(0),
            0x540f => {
                // Ön plan grubu ayarlanmadıysa çağıranın kendi grubunu bildir:
                // aksi hâlde bash kendini art plan sanıp SIGTTIN ile durur.
                let stored = self
                    .state
                    .fg_pgrp
                    .load(core::sync::atomic::Ordering::Relaxed);
                let group = if stored != 0 {
                    stored as i32
                } else {
                    crate::task::getpgrp() as i32
                };
                match copy_to_user(argument, &group.to_le_bytes()) {
                    Ok(()) => Some(0),
                    Err(code) => Some(code),
                }
            }
            0x5410 => {
                let Ok(data) = crate::syscall::copy_from_user(argument, 4) else {
                    return Some(crate::syscall::EFAULT);
                };
                let group = i32::from_le_bytes(data[..4].try_into().unwrap());
                self.state
                    .fg_pgrp
                    .store(group as u64, core::sync::atomic::Ordering::Relaxed);
                Some(0)
            }
            0x540e | 0x5419 => Some(0),
            0x5421 => Some(0),
            0x541b => {
                let available = self.state.to_slave.lock().len() as i32;
                match copy_to_user(argument, &available.to_le_bytes()) {
                    Ok(()) => Some(0),
                    Err(code) => Some(code),
                }
            }
            _ => None,
        }
    }
}
