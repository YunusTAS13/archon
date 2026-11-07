//! Sanal dosya sistemi.
//!
//! Tasarım Linux'a yakın: `Inode` içerik ve meta veriyi, `Dentry` isim
//! çözümlemesini, `FileSystem` bir bağlama noktasını temsil eder. Syscall
//! katmanı yalnızca bu trait'leri görür; ext4, ramfs ve devfs aynı arayüzü
//! uygular.

pub mod anon;
pub mod devfs;
pub mod epoll;
pub mod ext4;
pub mod procfs;
pub mod pty;
pub mod initramfs;
pub mod pipe;
pub mod ramfs;
pub mod unix;

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::sync::SpinLock;

pub type InodeNumber = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    NotFound,
    NotADirectory,
    IsADirectory,
    PermissionDenied,
    InvalidArgument,
    NoSpace,
    Io,
    ReadOnly,
    Unsupported,
    AlreadyExists,
    TooManyOpenFiles,
    NotEmpty,
    Busy,
    Interrupted,
    BadDescriptor,
    AddressInUse,
    AlreadyConnected,
    ConnectionRefused,
    ConnectionAborted,
    NotConnected,
    BrokenPipe,
}

pub type Result<T> = core::result::Result<T, VfsError>;

#[derive(Debug, Clone, Copy)]
pub struct Metadata {
    pub inode: InodeNumber,
    pub file_type: FileType,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub links: u32,
    pub blocks: u64,
    pub access_time: i64,
    pub modify_time: i64,
    pub change_time: i64,
}

impl Metadata {
    pub fn directory(inode: InodeNumber) -> Self {
        Self {
            inode,
            file_type: FileType::Directory,
            size: 0,
            mode: 0o755,
            uid: 0,
            gid: 0,
            links: 2,
            blocks: 0,
            access_time: 0,
            modify_time: 0,
            change_time: 0,
        }
    }

    pub fn regular(inode: InodeNumber, size: u64) -> Self {
        Self {
            inode,
            file_type: FileType::Regular,
            size,
            mode: 0o644,
            uid: 0,
            gid: 0,
            links: 1,
            blocks: (size + 511) / 512,
            access_time: 0,
            modify_time: 0,
            change_time: 0,
        }
    }

    pub fn is_directory(&self) -> bool {
        self.file_type == FileType::Directory
    }
}

pub trait Inode: Send + Sync {
    fn metadata(&self) -> Metadata;
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize>;
    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<usize>;
    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>>;
    fn create(&self, name: &str, file_type: FileType, mode: u32) -> Result<Arc<dyn Inode>>;
    fn unlink(&self, name: &str) -> Result<()>;
    fn readdir(&self) -> Result<Vec<(String, Arc<dyn Inode>)>>;
    fn truncate(&self, size: u64) -> Result<()>;
    fn sync(&self) -> Result<()> {
        Ok(())
    }

    /// `select`/`poll` için hazırlık durumu. Varsayılan: her zaman hazır.
    fn ready(&self, _write: bool) -> bool {
        true
    }

    /// `sendmsg`: veriye ek olarak dosya tanımlayıcıları taşır.
    fn send_message(&self, _data: &[u8], _files: &[Arc<OpenFile>]) -> Result<usize> {
        Err(VfsError::Unsupported)
    }

    /// `recvmsg`: mesajla gelen dosya tanımlayıcılarını döndürür.
    fn recv_message(
        &self,
        _buffer: &mut [u8],
        _max_files: usize,
    ) -> Result<(usize, Vec<Arc<OpenFile>>)> {
        Err(VfsError::Unsupported)
    }

    /// `isatty`/`tcgetattr` için: uçbirim (tty) mi?
    fn is_terminal(&self) -> bool {
        false
    }

    /// `mmap` için doğrudan eşlenecek fiziksel bellek (framebuffer gibi).
    fn physical_region(&self) -> Option<(u64, u64)> {
        None
    }

    /// Uçbirime/aygıta özgü `ioctl`. `None` = bu düğüm tanımıyor.
    fn ioctl(&self, _request: u64, _argument: u64) -> Option<i64> {
        None
    }

    /// Aygıt belleği eşlemesi kurulduğunda/kaldırıldığında çağrılır.
    fn mmap_notify(&self, _mapped: bool) {}

    /// `MAP_SHARED` için kalıcı fiziksel çerçeve (`memfd`/`shm` gibi).
    fn shared_frame(&self, _index: u64) -> Option<u64> {
        None
    }

    /// Somut anonim düğüm erişimleri (trait nesnesi aşağı çevrimi için).
    fn as_epoll(&self) -> Option<&epoll::EpollInode> {
        None
    }
    fn as_timerfd(&self) -> Option<&anon::TimerFdInode> {
        None
    }
    fn as_unix(&self) -> Option<&unix::UnixSocketInode> {
        None
    }
}

pub trait FileSystem: Send + Sync {
    fn name(&self) -> &'static str;
    fn root(&self) -> Arc<dyn Inode>;
    fn sync(&self) -> Result<()> {
        Ok(())
    }
}

/// Açık dosya tanımı: inode + paylaşımlı konum.
pub struct OpenFile {
    pub inode: Arc<dyn Inode>,
    pub offset: core::sync::atomic::AtomicU64,
    pub flags: u32,
    pub append: bool,
    /// `O_NONBLOCK`: hazır değilse `EAGAIN` dön.
    pub nonblock: core::sync::atomic::AtomicBool,
}

impl OpenFile {
    pub fn new(inode: Arc<dyn Inode>, flags: u32, append: bool, nonblock: bool) -> Self {
        Self {
            inode,
            offset: core::sync::atomic::AtomicU64::new(0),
            flags,
            append,
            nonblock: core::sync::atomic::AtomicBool::new(nonblock),
        }
    }

    pub fn is_nonblock(&self) -> bool {
        self.nonblock.load(core::sync::atomic::Ordering::Relaxed)
    }
}

pub const O_RDONLY: u32 = 0o0;
pub const O_WRONLY: u32 = 0o1;
pub const O_RDWR: u32 = 0o2;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_NONBLOCK: u32 = 0o4000;
pub const O_CLOEXEC: u32 = 0o2000000;

/// Süreç başına dosya tanımlayıcı tablosu. 0/1/2 console'a bağlanır.
/// Konsoldan okur: önce PS/2 tamponu, sonra seri port. Karakter gelene kadar
/// diğer süreçlere zaman verilir.
/// Konsolda okunmayı bekleyen bayt var mı?
pub fn console_ready() -> bool {
    crate::drivers::ps2::pending() > 0 || crate::arch::serial::Serial::can_read()
}

pub fn console_read(buffer: &mut [u8]) -> Result<usize> {
    if buffer.is_empty() {
        return Ok(0);
    }
    loop {
        if let Some(value) = crate::drivers::ps2::read_char() {
            buffer[0] = value;
            return Ok(1);
        }
        if let Some(value) = crate::arch::serial::Serial::read_byte() {
            buffer[0] = value;
            return Ok(1);
        }
        if crate::task::signal::interrupted() {
            return Err(VfsError::Interrupted);
        }
        crate::task::schedule();
    }
}

/// 0/1/2 için yer tutucu konsol düğümü: Linux'ta bunlar her zaman açıktır.
struct ConsoleInode;

impl Inode for ConsoleInode {
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::regular(1, 0);
        metadata.file_type = FileType::CharDevice;
        metadata.mode = 0o620;
        metadata
    }
    fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<usize> {
        console_read(buffer)
    }
    fn write_at(&self, _offset: u64, buffer: &[u8]) -> Result<usize> {
        crate::log::write_bytes(buffer);
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
        Ok(())
    }
}

pub struct DescriptorTable {
    files: SpinLock<Vec<Option<Arc<OpenFile>>>>,
    /// Tanıtıcı başına `FD_CLOEXEC` bayrağı (`execve` sırasında kapanır).
    cloexec: SpinLock<Vec<bool>>,
    /// 0/1/2 stdin/stdout/stderr için ayrılır; `insert` bunları atlar.
    reserved: usize,
}

impl DescriptorTable {
    pub fn new() -> Self {
        let mut files: Vec<Option<Arc<OpenFile>>> = Vec::new();
        for _ in 0..3 {
            files.push(Some(Arc::new(OpenFile::new(
                Arc::new(ConsoleInode),
                0,
                false,
                false,
            ))));
        }
        Self {
            files: SpinLock::new(files),
            cloexec: SpinLock::new(alloc::vec![false; 3]),
            reserved: 3,
        }
    }

    pub fn insert(&self, file: Arc<OpenFile>) -> Result<i32> {
        let mut files = self.files.lock();
        for (index, slot) in files.iter_mut().enumerate().skip(self.reserved) {
            if slot.is_none() {
                *slot = Some(file);
                let mut cloexec = self.cloexec.lock();
                if cloexec.len() <= index {
                    cloexec.resize(index + 1, false);
                }
                cloexec[index] = false;
                return Ok(index as i32);
            }
        }
        if files.len() >= 1024 {
            return Err(VfsError::TooManyOpenFiles);
        }
        files.push(Some(file));
        self.cloexec.lock().push(false);
        Ok((files.len() - 1) as i32)
    }

    /// Dosya tanımlayıcılarını kopyalar; `inode` paylaşılır, ofset ayrılır.
    pub fn duplicate(&self) -> Self {
        let files = self.files.lock();
        let mut copy = Vec::with_capacity(files.len());
        for slot in files.iter() {
            copy.push(slot.as_ref().map(|file| {
                Arc::new(OpenFile {
                    inode: file.inode.clone(),
                    offset: core::sync::atomic::AtomicU64::new(
                        file.offset.load(core::sync::atomic::Ordering::Relaxed),
                    ),
                    flags: file.flags,
                    append: file.append,
                    nonblock: core::sync::atomic::AtomicBool::new(file.is_nonblock()),
                })
            }));
        }
        Self {
            files: SpinLock::new(copy),
            cloexec: SpinLock::new(self.cloexec.lock().clone()),
            reserved: self.reserved,
        }
    }

    pub fn install(&self, descriptor: i32, file: Arc<OpenFile>) -> Result<()> {
        let mut files = self.files.lock();
        if descriptor < 0 {
            return Err(VfsError::InvalidArgument);
        }
        let index = descriptor as usize;
        while files.len() <= index {
            files.push(None);
        }
        files[index] = Some(file);
        drop(files);
        let mut cloexec = self.cloexec.lock();
        if cloexec.len() <= index {
            cloexec.resize(index + 1, false);
        }
        cloexec[index] = false;
        Ok(())
    }

    pub fn get(&self, descriptor: i32) -> Result<Arc<OpenFile>> {
        let files = self.files.lock();
        if descriptor < 0 {
            return Err(VfsError::BadDescriptor);
        }
        files
            .get(descriptor as usize)
            .and_then(|slot| slot.clone())
            .ok_or(VfsError::BadDescriptor)
    }

    pub fn close(&self, descriptor: i32) -> Result<()> {
        let mut files = self.files.lock();
        if descriptor < 0 {
            return Err(VfsError::BadDescriptor);
        }
        let slot = files
            .get_mut(descriptor as usize)
            .ok_or(VfsError::BadDescriptor)?;
        if slot.take().is_none() {
            return Err(VfsError::BadDescriptor);
        }
        drop(files);
        let mut cloexec = self.cloexec.lock();
        if let Some(flag) = cloexec.get_mut(descriptor as usize) {
            *flag = false;
        }
        Ok(())
    }

    pub fn cloexec(&self, descriptor: i32) -> bool {
        self.cloexec
            .lock()
            .get(descriptor as usize)
            .copied()
            .unwrap_or(false)
    }

    pub fn set_cloexec(&self, descriptor: i32, value: bool) {
        let mut cloexec = self.cloexec.lock();
        if descriptor >= 0 && (descriptor as usize) < cloexec.len() {
            cloexec[descriptor as usize] = value;
        }
    }

    /// `execve` öncesi: `FD_CLOEXEC` işaretli tüm tanıtıcıları kapatır.
    pub fn close_cloexec(&self) {
        let cloexec = self.cloexec.lock().clone();
        for (descriptor, flagged) in cloexec.iter().enumerate() {
            if *flagged {
                let _ = self.close(descriptor as i32);
            }
        }
    }

    pub fn dup(&self, descriptor: i32, minimum: i32) -> Result<i32> {
        let file = self.get(descriptor)?;
        let mut files = self.files.lock();
        const MAX_DESCRIPTORS: usize = 1024;
        let mut index = minimum.max(0) as usize;
        if index >= MAX_DESCRIPTORS {
            return Err(VfsError::InvalidArgument);
        }
        while index < files.len() && files[index].is_some() {
            index += 1;
        }
        if index >= files.len() {
            files.resize(index + 1, None);
        }
        files[index] = Some(file);
        drop(files);
        let mut cloexec = self.cloexec.lock();
        if cloexec.len() <= index {
            cloexec.resize(index + 1, false);
        }
        cloexec[index] = false;
        Ok(index as i32)
    }
}

impl Default for DescriptorTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Global kök. `mount` gelene kadar ramfs.
static ROOT: SpinLock<Option<Arc<dyn FileSystem>>> = SpinLock::new(None);

pub fn mount_root(filesystem: Arc<dyn FileSystem>) {
    *ROOT.lock() = Some(filesystem);
}

pub fn root() -> Option<Arc<dyn FileSystem>> {
    ROOT.lock().clone()
}

/// Süreç başına çalışma dizini (şimdilik tek, çekirdek geneli).
static CWD: SpinLock<alloc::string::String> = SpinLock::new(alloc::string::String::new());

pub fn getcwd() -> alloc::string::String {
    let cwd = CWD.lock();
    if cwd.is_empty() {
        alloc::string::String::from("/")
    } else {
        cwd.clone()
    }
}

pub fn chdir(path: &str) -> Result<()> {
    let inode = lookup_path(path)?;
    if !inode.metadata().is_directory() {
        return Err(VfsError::NotADirectory);
    }
    let absolute = absolute_path(path);
    *CWD.lock() = absolute;
    Ok(())
}

/// Göreli yolu çalışma dizinine göre mutlaklaştırır ve normalleştirir.
pub fn absolute_path(path: &str) -> alloc::string::String {
    let joined = if path.starts_with('/') {
        alloc::string::String::from(path)
    } else {
        alloc::format!("{}/{}", getcwd(), path)
    };
    let mut result = alloc::string::String::from("/");
    for component in normalize(&joined) {
        if result.len() > 1 {
            result.push('/');
        }
        result.push_str(component);
    }
    result
}

/// Yolu üst dizin ve son bileşene ayırır (`unlink`, `mkdir`, `rename` için).
pub fn parent_and_name(path: &str) -> Result<(Arc<dyn Inode>, alloc::string::String)> {
    let full = absolute_path(path);
    let trimmed = full.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(VfsError::InvalidArgument);
    }
    let (parent_path, name) = match trimmed.rfind('/') {
        Some(0) => ("/", &trimmed[1..]),
        Some(index) => (&trimmed[..index], &trimmed[index + 1..]),
        None => ("/", trimmed),
    };
    if name.is_empty() {
        return Err(VfsError::InvalidArgument);
    }
    let parent = lookup_path(parent_path)?;
    if !parent.metadata().is_directory() {
        return Err(VfsError::NotADirectory);
    }
    Ok((parent, name.to_string()))
}

/// Mutlak yolu bileşenlere ayırır, `.` ve `..` çözer.
pub fn normalize(path: &str) -> Vec<&str> {
    let mut components: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            other => components.push(other),
        }
    }
    components
}

/// Bağlama noktaları: (yol bileşenleri, dosya sistemi).
static MOUNTS: SpinLock<Vec<(Vec<String>, Arc<dyn FileSystem>)>> = SpinLock::new(Vec::new());

/// Bağlı dosya sistemleri: `(bağlama noktası, dosya sistemi adı)`.
pub fn mount_list() -> Vec<(alloc::string::String, &'static str)> {
    let mut list = Vec::new();
    if let Some(filesystem) = root() {
        list.push((alloc::string::String::from("/"), filesystem.name()));
    }
    for (components, filesystem) in MOUNTS.lock().iter() {
        list.push((
            alloc::format!("/{}", components.join("/")),
            filesystem.name(),
        ));
    }
    list
}

/// Tanımlayıcının `select`/`poll` hazırlığı; tanımsız fd için `None`.
pub fn descriptor_ready(descriptor: i32, write: bool) -> Option<bool> {
    let process = crate::task::current()?;
    let file = process.fd_table.get(descriptor).ok()?;
    Some(file.inode.ready(write))
}

/// `prefix` altına bir dosya sistemi bağlar. En uzun önek kazanır.
pub fn mount(prefix: &str, filesystem: Arc<dyn FileSystem>) {
    let components: Vec<String> = normalize(prefix)
        .into_iter()
        .map(|part| part.to_string())
        .collect();
    MOUNTS.lock().push((components, filesystem));
}

pub fn lookup_path(path: &str) -> Result<Arc<dyn Inode>> {
    let full = absolute_path(path);
    let components = normalize(&full);

    let mounted = {
        let mounts = MOUNTS.lock();
        let mut best: Option<(usize, Arc<dyn FileSystem>)> = None;
        for (prefix, filesystem) in mounts.iter() {
            if prefix.len() > components.len() {
                continue;
            }
            if prefix
                .iter()
                .zip(components.iter())
                .all(|(left, right)| left == right)
            {
                let better = best.as_ref().map(|(length, _)| prefix.len() > *length).unwrap_or(true);
                if better {
                    best = Some((prefix.len(), filesystem.clone()));
                }
            }
        }
        best
    };

    let (start, filesystem) = match mounted {
        Some((length, filesystem)) => (length, filesystem),
        None => (0, root().ok_or(VfsError::NotFound)?),
    };

    let mut current = filesystem.root();
    for component in &components[start..] {
        if !current.metadata().is_directory() {
            return Err(VfsError::NotADirectory);
        }
        current = current.lookup(component)?;
    }
    Ok(current)
}

/// `pipe()`: iki uçlu boru açar, (okuma, yazma) fd'lerini döndürür.
pub fn create_pipe() -> Result<(i32, i32)> {
    let (reader, writer) = pipe::PipeInode::pair();
    let process = crate::task::current().ok_or(VfsError::InvalidArgument)?;

    let read_descriptor =
        process
            .fd_table
            .insert(Arc::new(OpenFile::new(reader, 0, false, false)))?;
    let write_descriptor =
        process
            .fd_table
            .insert(Arc::new(OpenFile::new(writer, 1, false, false)))?;
    Ok((read_descriptor, write_descriptor))
}

/// Anonim bir düğümü çağıran sürecin fd tablosuna ekler.
pub fn install_anonymous(inode: Arc<dyn Inode>, flags: u32, nonblock: bool) -> Result<i32> {
    let process = crate::task::current().ok_or(VfsError::InvalidArgument)?;
    process
        .fd_table
        .insert(Arc::new(OpenFile::new(inode, flags, false, nonblock)))
}

pub fn init() {
    let ramfs = ramfs::RamFs::new();
    let root_inode = ramfs.root();

    // /dev ve /tmp ilk açılışta hazır olsun.
    let _ = root_inode.create("dev", FileType::Directory, 0o755);
    let _ = root_inode.create("tmp", FileType::Directory, 0o1777);
    let _ = root_inode.create("proc", FileType::Directory, 0o555);
    let _ = root_inode.create("mnt", FileType::Directory, 0o755);

    mount_root(alloc::sync::Arc::new(ramfs));
    println!("vfs: ramfs kok bagli (devfs/procfs sonra)");
}
