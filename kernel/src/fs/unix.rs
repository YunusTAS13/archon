//! `AF_UNIX` soketleri.
//!
//! Linux'un yol adlı (pathname) ve soyut (abstract) soketlerini destekler:
//! `socket`/`bind`/`connect`/`listen`/`accept`/`sendmsg`/`recvmsg`.
//! `SCM_RIGHTS` ile dosya tanımlayıcı geçişi Wayland ve D-Bus için zorunlu.
//!
//! Bağlantı modeli: iki uç tek bir `Connection` durumunu paylaşır. Her yön
//! ayrı bir mesaj kuyruğudur; bir mesaj veri ve (varsa) geçirilen fd'leri
//! birlikte taşır. Böylece `sendmsg` ile gönderilen fd, aynı mesajın baytları
//! okunurken teslim edilir.

use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::fs::{FileType, Inode, Metadata, OpenFile, Result, VfsError};
use crate::sync::SpinLock;

pub const AF_UNIX: u32 = 1;
pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;

/// Yön başına tampon sınırı.
const CAPACITY: usize = 64 * 1024;

struct Message {
    data: VecDeque<u8>,
    files: Vec<Arc<OpenFile>>,
}

impl Message {
    fn new(data: &[u8], files: &[Arc<OpenFile>]) -> Self {
        Self {
            data: data.iter().copied().collect(),
            files: files.to_vec(),
        }
    }
}

struct Connection {
    /// `queues[i]`: `i` numaralı ucun okuyacağı mesajlar.
    queues: [VecDeque<Message>; 2],
    queued: [usize; 2],
    open: [bool; 2],
    read_shutdown: [bool; 2],
    write_shutdown: [bool; 2],
}

impl Connection {
    fn new() -> Self {
        Self {
            queues: [VecDeque::new(), VecDeque::new()],
            queued: [0, 0],
            open: [true, true],
            read_shutdown: [false, false],
            write_shutdown: [false, false],
        }
    }
}

struct Listener {
    pending: SpinLock<VecDeque<Arc<UnixSocketInode>>>,
    backlog: usize,
    open: AtomicBool,
}

impl Listener {
    fn new(backlog: usize) -> Self {
        Self {
            pending: SpinLock::new(VecDeque::new()),
            backlog: backlog.clamp(1, 128),
            open: AtomicBool::new(true),
        }
    }
}

enum State {
    Idle,
    Connected {
        connection: Arc<SpinLock<Connection>>,
        side: usize,
    },
    Listening(Arc<Listener>),
}

pub struct UnixSocketInode {
    state: SpinLock<State>,
    kind: u32,
    bound: SpinLock<Option<Vec<u8>>>,
}

/// `unlink` sonrası yol kaydını düşürür.
pub fn forget_path(path: &str) {
    let mut registry = BOUND.lock();
    registry.retain(|(key, _)| key.as_slice() != path.as_bytes());
}

/// Yol → dinleyici kaydı. `bind` buraya yazar, `connect` buradan bulur.
/// Zayıf referans: fd kapandığında kayıt kendiliğinden ölür.
static BOUND: SpinLock<Vec<(Vec<u8>, Weak<dyn Inode>)>> = SpinLock::new(Vec::new());

impl UnixSocketInode {
    /// `socketpair`: aynı bağlantının iki ucu.
    pub fn pair(kind: u32) -> (Arc<UnixSocketInode>, Arc<UnixSocketInode>) {
        let connection = Arc::new(SpinLock::new(Connection::new()));
        let first = Arc::new(Self {
            state: SpinLock::new(State::Connected {
                connection: connection.clone(),
                side: 0,
            }),
            kind,
            bound: SpinLock::new(None),
        });
        let second = Arc::new(Self {
            state: SpinLock::new(State::Connected {
                connection,
                side: 1,
            }),
            kind,
            bound: SpinLock::new(None),
        });
        (first, second)
    }

    /// `socket(AF_UNIX, ...)`: henüz bağlanmamış soket.
    pub fn create(kind: u32) -> Arc<UnixSocketInode> {
        Arc::new(Self {
            state: SpinLock::new(State::Idle),
            kind,
            bound: SpinLock::new(None),
        })
    }

    fn connection(&self) -> Result<(Arc<SpinLock<Connection>>, usize)> {
        let state = self.state.lock();
        match &*state {
            State::Connected { connection, side } => Ok((connection.clone(), *side)),
            State::Idle => Err(VfsError::NotConnected),
            State::Listening(_) => Err(VfsError::InvalidArgument),
        }
    }

    // --- bağlama / dinleme / bağlanma -------------------------------------

    pub fn bind(&self, key: Vec<u8>, handle: Weak<dyn Inode>) -> Result<()> {
        {
            let mut bound = self.bound.lock();
            if bound.is_some() {
                return Err(VfsError::InvalidArgument);
            }
        }
        let mut registry = BOUND.lock();
        registry.retain(|(_, existing)| existing.upgrade().is_some());
        if registry.iter().any(|(existing, _)| *existing == key) {
            return Err(VfsError::AddressInUse);
        }
        registry.push((key.clone(), handle));
        drop(registry);
        *self.bound.lock() = Some(key);
        Ok(())
    }

    pub fn listen(&self, backlog: usize) -> Result<()> {
        if self.bound.lock().is_none() {
            return Err(VfsError::InvalidArgument);
        }
        let listener = Arc::new(Listener::new(backlog));
        let mut state = self.state.lock();
        match &*state {
            State::Idle => {
                *state = State::Listening(listener);
                Ok(())
            }
            _ => Err(VfsError::InvalidArgument),
        }
    }

    fn lookup_bound(key: &[u8]) -> Option<Arc<dyn Inode>> {
        let registry = BOUND.lock();
        registry
            .iter()
            .find(|(existing, _)| existing.as_slice() == key)
            .and_then(|(_, inode)| inode.upgrade())
    }

    pub fn connect(&self, key: &[u8]) -> Result<()> {
        {
            let state = self.state.lock();
            if !matches!(&*state, State::Idle) {
                return Err(VfsError::AlreadyConnected);
            }
        }
        let listener_inode = Self::lookup_bound(key).ok_or(VfsError::NotFound)?;
        let listener = match listener_inode.as_unix() {
            Some(inode) => {
                let state = inode.state.lock();
                match &*state {
                    State::Listening(listener) => listener.clone(),
                    _ => return Err(VfsError::ConnectionRefused),
                }
            }
            None => return Err(VfsError::ConnectionRefused),
        };

        let connection = Arc::new(SpinLock::new(Connection::new()));
        let server = Arc::new(Self {
            state: SpinLock::new(State::Connected {
                connection: connection.clone(),
                side: 1,
            }),
            kind: self.kind,
            bound: SpinLock::new(None),
        });

        {
            let mut pending = listener.pending.lock();
            if pending.len() >= listener.backlog {
                return Err(VfsError::ConnectionRefused);
            }
            pending.push_back(server);
        }

        let mut state = self.state.lock();
        *state = State::Connected {
            connection,
            side: 0,
        };
        Ok(())
    }

    /// Bağlantıyı kabul eder; çağıran yeni soketi fd olarak kurar.
    pub fn accept(&self) -> Result<Arc<UnixSocketInode>> {
        let listener = {
            let state = self.state.lock();
            match &*state {
                State::Listening(listener) => listener.clone(),
                _ => return Err(VfsError::InvalidArgument),
            }
        };
        loop {
            {
                let mut pending = listener.pending.lock();
                if let Some(server) = pending.pop_front() {
                    return Ok(server);
                }
                if !listener.open.load(Ordering::Acquire) {
                    return Err(VfsError::ConnectionAborted);
                }
            }
            if crate::task::signal::interrupted() {
                return Err(VfsError::Interrupted);
            }
            crate::task::schedule();
        }
    }

    // --- veri -------------------------------------------------------------

    pub fn send(&self, data: &[u8], files: &[Arc<OpenFile>]) -> Result<usize> {
        let (connection, side) = self.connection()?;
        let peer = 1 - side;
        let mut spins = 0u32;
        loop {
            {
                let mut shared = connection.lock();
                if !shared.open[peer] || shared.write_shutdown[side] {
                    return Err(VfsError::BrokenPipe);
                }
                let space = CAPACITY.saturating_sub(shared.queued[peer]);
                if space > 0 || (data.is_empty() && !files.is_empty()) {
                    let count = space.min(data.len());
                    shared.queues[peer].push_back(Message::new(&data[..count], files));
                    shared.queued[peer] += count;
                    return Ok(count);
                }
            }
            if crate::task::signal::interrupted() {
                return Err(VfsError::Interrupted);
            }
            spins = spins.wrapping_add(1);
            if spins % 64 == 0 {
                crate::task::schedule();
            } else {
                core::hint::spin_loop();
            }
        }
    }

    pub fn receive(
        &self,
        buffer: &mut [u8],
        max_files: usize,
    ) -> Result<(usize, Vec<Arc<OpenFile>>)> {
        let (connection, side) = self.connection()?;
        let peer = 1 - side;
        let mut spins = 0u32;
        loop {
            let mut delivered: Vec<Arc<OpenFile>> = Vec::new();
            {
                let mut shared = connection.lock();
                if let Some(front) = shared.queues[side].front_mut() {
                    if !front.files.is_empty() && front.files.len() <= max_files {
                        delivered = core::mem::take(&mut front.files);
                    }
                    let count = buffer.len().min(front.data.len());
                    for slot in buffer[..count].iter_mut() {
                        if let Some(byte) = front.data.pop_front() {
                            *slot = byte;
                        }
                    }
                    shared.queued[side] -= count;
                    if shared.queues[side].front().map(|m| m.data.is_empty()).unwrap_or(false) {
                        shared.queues[side].pop_front();
                    }
                    return Ok((count, delivered));
                }
                if !shared.open[peer] || shared.read_shutdown[side] || shared.write_shutdown[peer] {
                    return Ok((0, Vec::new()));
                }
            }
            if crate::task::signal::interrupted() {
                return Err(VfsError::Interrupted);
            }
            spins = spins.wrapping_add(1);
            if spins % 64 == 0 {
                crate::task::schedule();
            } else {
                core::hint::spin_loop();
            }
        }
    }

    pub fn shutdown(&self, how: u32) -> Result<()> {
        let (connection, side) = self.connection()?;
        let mut shared = connection.lock();
        match how {
            0 => shared.read_shutdown[side] = true,
            1 => shared.write_shutdown[side] = true,
            2 => {
                shared.read_shutdown[side] = true;
                shared.write_shutdown[side] = true;
            }
            _ => return Err(VfsError::InvalidArgument),
        }
        Ok(())
    }

    pub fn bound_name(&self) -> Option<Vec<u8>> {
        self.bound.lock().clone()
    }

    pub fn kind(&self) -> u32 {
        self.kind
    }

    pub fn is_listening(&self) -> bool {
        matches!(&*self.state.lock(), State::Listening(_))
    }
}

impl Drop for UnixSocketInode {
    fn drop(&mut self) {
        let state = self.state.lock();
        match &*state {
            State::Connected { connection, side } => {
                connection.lock().open[*side] = false;
            }
            State::Listening(listener) => {
                listener.open.store(false, Ordering::Release);
            }
            State::Idle => {}
        }
    }
}

impl Inode for UnixSocketInode {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::regular(0, 0);
        metadata.file_type = FileType::Socket;
        metadata.mode = 0o600;
        metadata
    }

    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.receive(buffer, 0).map(|(count, _)| count)
    }

    fn write_at(&self, _offset: u64, buffer: &[u8]) -> Result<usize> {
        self.send(buffer, &[])
    }

    fn send_message(&self, data: &[u8], files: &[Arc<OpenFile>]) -> Result<usize> {
        self.send(data, files)
    }

    fn recv_message(
        &self,
        buffer: &mut [u8],
        max_files: usize,
    ) -> Result<(usize, Vec<Arc<OpenFile>>)> {
        self.receive(buffer, max_files)
    }

    fn ready(&self, write: bool) -> bool {
        let state = self.state.lock();
        match &*state {
            State::Listening(listener) => {
                if write {
                    return false;
                }
                !listener.pending.lock().is_empty()
            }
            State::Connected { connection, side } => {
                let shared = connection.lock();
                let peer = 1 - *side;
                if write {
                    shared.open[peer]
                        && !shared.write_shutdown[*side]
                        && shared.queued[peer] < CAPACITY
                } else {
                    !shared.queues[*side].is_empty()
                        || !shared.open[peer]
                        || shared.read_shutdown[*side]
                        || shared.write_shutdown[peer]
                }
            }
            State::Idle => false,
        }
    }

    fn as_unix(&self) -> Option<&UnixSocketInode> {
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
    fn readdir(&self) -> Result<Vec<(alloc::string::String, Arc<dyn Inode>)>> {
        Err(VfsError::NotADirectory)
    }
    fn truncate(&self, _size: u64) -> Result<()> {
        Err(VfsError::InvalidArgument)
    }
}
