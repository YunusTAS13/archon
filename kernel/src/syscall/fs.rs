//! Dosya sistemi syscall'ları.

use alloc::string::String;
use alloc::sync::Arc;

use super::{
    copy_from_user, copy_to_user, error, EACCES, EAGAIN, EBADF, EFAULT, EINVAL, ENOENT, ENOSYS,
    EPERM,
};

use crate::fs::{self, DescriptorTable, FileType, Inode, OpenFile, VfsError};

fn descriptors() -> Option<Arc<crate::task::Process>> {
    crate::task::current()
}

pub fn write(descriptor: i32, pointer: u64, length: usize) -> i64 {
    if length == 0 {
        return 0;
    }
    let buffer = match copy_from_user(pointer, length) {
        Ok(buffer) => buffer,
        Err(code) => return code,
    };
    write_bytes(descriptor, &buffer)
}

/// Kullanıcı adresi almadan, hazır baytlarla yazar.
fn write_bytes(descriptor: i32, buffer: &[u8]) -> i64 {
    let Some(process) = descriptors() else {
        return EBADF;
    };
    let file = match process.fd_table.get(descriptor) {
        Ok(file) => file,
        Err(_) => return EBADF,
    };

    if file.is_nonblock() && !file.inode.ready(true) {
        return EAGAIN;
    }

    let offset = if file.append {
        file.inode.metadata().size
    } else {
        file.offset.load(core::sync::atomic::Ordering::Relaxed)
    };

    match file.inode.write_at(offset, buffer) {
        Ok(written) => {
            file.offset
                .store(offset + written as u64, core::sync::atomic::Ordering::Relaxed);
            written as i64
        }
        Err(failure) => error(failure),
    }
}

pub fn read(descriptor: i32, pointer: u64, length: usize) -> i64 {
    if length == 0 {
        return 0;
    }

    let mut buffer = alloc::vec![0u8; length];
    let count = read_bytes(descriptor, &mut buffer);
    if count < 0 {
        return count;
    }
    match copy_to_user(pointer, &buffer[..count as usize]) {
        Ok(()) => count,
        Err(code) => code,
    }
}

/// Kullanıcı adresi almadan, hazır tampona okur.
fn read_bytes(descriptor: i32, buffer: &mut [u8]) -> i64 {
    let Some(process) = descriptors() else {
        return EBADF;
    };
    let file = match process.fd_table.get(descriptor) {
        Ok(file) => file,
        Err(_) => return EBADF,
    };

    if file.is_nonblock() && !file.inode.ready(false) {
        return EAGAIN;
    }

    let offset = file.offset.load(core::sync::atomic::Ordering::Relaxed);
    match file.inode.read_at(offset, buffer) {
        Ok(count) => {
            file.offset
                .store(offset + count as u64, core::sync::atomic::Ordering::Relaxed);
            count as i64
        }
        Err(failure) => error(failure),
    }
}

/// `writev(fd, iovec[], count)`.
pub fn writev(descriptor: i32, pointer: u64, count: usize) -> i64 {
    if count == 0 {
        return 0;
    }
    if count > 1024 {
        return EINVAL;
    }
    let table = match copy_from_user(pointer, count * 16) {
        Ok(table) => table,
        Err(code) => return code,
    };

    let mut data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    for index in 0..count {
        let base = u64::from_le_bytes(table[index * 16..index * 16 + 8].try_into().unwrap());
        let length =
            u64::from_le_bytes(table[index * 16 + 8..index * 16 + 16].try_into().unwrap());
        if length == 0 {
            continue;
        }
        if data.len() + length as usize > 1024 * 1024 {
            return EINVAL;
        }
        match copy_from_user(base, length as usize) {
            Ok(chunk) => data.extend_from_slice(&chunk),
            Err(code) => return code,
        }
    }
    write_bytes(descriptor, &data)
}

/// `readv(fd, iovec[], count)`.
pub fn readv(descriptor: i32, pointer: u64, count: usize) -> i64 {
    if count == 0 {
        return 0;
    }
    if count > 1024 {
        return EINVAL;
    }
    let table = match copy_from_user(pointer, count * 16) {
        Ok(table) => table,
        Err(code) => return code,
    };

    let mut total = 0usize;
    let mut lengths: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    for index in 0..count {
        let length =
            u64::from_le_bytes(table[index * 16 + 8..index * 16 + 16].try_into().unwrap());
        lengths.push(length as usize);
        total += length as usize;
    }
    if total == 0 {
        return 0;
    }

    let mut buffer = alloc::vec![0u8; total];
    let read = read_bytes(descriptor, &mut buffer);
    if read <= 0 {
        return read;
    }

    let mut written = 0usize;
    for index in 0..count {
        let base = u64::from_le_bytes(table[index * 16..index * 16 + 8].try_into().unwrap());
        let length = lengths[index].min(read as usize - written);
        if length == 0 {
            break;
        }
        if let Err(code) = copy_to_user(base, &buffer[written..written + length]) {
            return if written > 0 { written as i64 } else { code };
        }
        written += length;
        if written >= read as usize {
            break;
        }
    }
    written as i64
}

pub fn pipe2(pointer: u64, flags: u32) -> i64 {
    let _ = flags;
    match fs::create_pipe() {
        Ok((read_descriptor, write_descriptor)) => {
            let mut buffer = [0u8; 8];
            buffer[0..4].copy_from_slice(&read_descriptor.to_le_bytes());
            buffer[4..8].copy_from_slice(&write_descriptor.to_le_bytes());
            match copy_to_user(pointer, &buffer) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
        Err(failure) => error(failure),
    }
}

pub fn dup(descriptor: i32) -> i64 {
    let Some(process) = descriptors() else {
        return EBADF;
    };
    match process.fd_table.dup(descriptor, 0) {
        Ok(new_descriptor) => new_descriptor as i64,
        Err(failure) => error(failure),
    }
}

pub fn dup2(old_descriptor: i32, new_descriptor: i32) -> i64 {
    if old_descriptor == new_descriptor {
        return match descriptors().map(|process| process.fd_table.get(old_descriptor)) {
            Some(Ok(_)) => new_descriptor as i64,
            _ => EBADF,
        };
    }
    let Some(process) = descriptors() else {
        return EBADF;
    };
    let file = match process.fd_table.get(old_descriptor) {
        Ok(file) => file,
        Err(_) => return EBADF,
    };
    let _ = process.fd_table.close(new_descriptor);
    match process.fd_table.install(new_descriptor, file) {
        Ok(()) => new_descriptor as i64,
        Err(failure) => error(failure),
    }
}

pub fn fcntl(descriptor: i32, command: u32, argument: u64) -> i64 {
    let Some(process) = descriptors() else {
        return EBADF;
    };
    match command {
        0 | 1030 | 1031 => match process.fd_table.dup(descriptor, argument as i32) {
            Ok(new_descriptor) => {
                if command & 1 != 0 {
                    process.fd_table.set_cloexec(new_descriptor, true);
                }
                new_descriptor as i64
            }
            Err(failure) => error(failure),
        },
        1 => {
            // F_GETFD: kapalı tanıtıcı EBADF döndürmeli, yoksa boş tanıtıcı
            // arayan döngüler (bash) sonsuza kadar döner.
            match process.fd_table.get(descriptor) {
                Ok(_) => process.fd_table.cloexec(descriptor) as i64,
                Err(_) => EBADF,
            }
        }
        2 => {
            if process.fd_table.get(descriptor).is_err() {
                return EBADF;
            }
            process.fd_table.set_cloexec(descriptor, argument as i32 & 1 != 0);
            0
        }
        3 => match process.fd_table.get(descriptor) {  // F_GETFL
            Ok(file) => {
                file.flags as i64
                    | if file.append { 0o2000 } else { 0 }
                    | if file.is_nonblock() { 0o4000 } else { 0 }
            }
            Err(_) => EBADF,
        },
        4 => match process.fd_table.get(descriptor) { // F_SETFL
            Ok(file) => {
                file.nonblock.store(
                    argument as u32 & 0o4000 != 0,
                    core::sync::atomic::Ordering::Relaxed,
                );
                0
            }
            Err(_) => EBADF,
        },
        _ => EINVAL,
    }
}

pub fn open(path_pointer: u64, flags: u64, mode: u32) -> i64 {
    openat(-100, path_pointer, flags as u32, mode) // AT_FDCWD
}

pub fn openat(directory: i32, path_pointer: u64, flags: u32, mode: u32) -> i64 {
    let _ = directory;
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };

    let Some(process) = descriptors() else {
        return EBADF;
    };

    let inode = match fs::lookup_path(&path) {
        Ok(inode) => inode,
        Err(VfsError::NotFound) => {
            // Yalnızca ramfs yazılabilir; ext4 yazma M4'te.
            if flags & fs::O_CREAT == 0 {
                return ENOENT;
            }
            let (parent_path, name) = split_parent(&path);
            let parent = match fs::lookup_path(parent_path) {
                Ok(parent) => parent,
                Err(failure) => return error(failure),
            };
            match parent.create(name, FileType::Regular, mode) {
                Ok(inode) => inode,
                Err(failure) => return error(failure),
            }
        }
        Err(failure) => return error(failure),
    };

    let metadata = inode.metadata();
    if (flags & fs::O_WRONLY != 0 || flags & fs::O_RDWR != 0) && metadata.mode & 0o200 == 0 {
        return EACCES;
    }
    if metadata.is_directory() && flags & fs::O_DIRECTORY == 0 && flags & fs::O_RDONLY == fs::O_RDONLY {
        // Dizin açmak serbest; okuma syscall'ı EISDIR döner.
    }
    if flags & fs::O_TRUNC != 0 {
        let _ = inode.truncate(0);
    }
    if metadata.is_directory() && flags & fs::O_WRONLY != 0 {
        return EINVAL;
    }

    let file = Arc::new(OpenFile::new(
        inode,
        flags,
        flags & fs::O_APPEND != 0,
        flags & fs::O_NONBLOCK != 0,
    ));

    match process.fd_table.insert(file) {
        Ok(descriptor) => {
            if flags & fs::O_CLOEXEC != 0 {
                process.fd_table.set_cloexec(descriptor, true);
            }
            descriptor as i64
        }
        Err(failure) => error(failure),
    }
}

pub fn close(descriptor: i32) -> i64 {
    let Some(process) = descriptors() else {
        return EBADF;
    };
    match process.fd_table.close(descriptor) {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

pub fn lseek(descriptor: i32, offset: i64, whence: u32) -> i64 {
    let Some(process) = descriptors() else {
        return EBADF;
    };
    let file = match process.fd_table.get(descriptor) {
        Ok(file) => file,
        Err(_) => return EBADF,
    };

    let size = file.inode.metadata().size as i64;
    let base = match whence {
        0 => 0,
        1 => file.offset.load(core::sync::atomic::Ordering::Relaxed) as i64,
        2 => size,
        _ => return EINVAL,
    };

    let target = base + offset;
    if target < 0 {
        return EINVAL;
    }
    file.offset.store(target as u64, core::sync::atomic::Ordering::Relaxed);
    target
}

pub fn ioctl(descriptor: i32, request: u64, argument: u64) -> i64 {
    let inode = crate::task::current()
        .and_then(|process| process.fd_table.get(descriptor).ok())
        .map(|file| file.inode.clone());

    // Düğüme özgü ioctl'ler (pty, framebuffer) genel uçbirim yolundan önce.
    if let Some(value) = inode.as_ref().and_then(|inode| inode.ioctl(request, argument)) {
        return value;
    }

    // Uçbirime özgü istekler yalnızca konsol fd'lerinde geçerli; boru/dosya
    // üzerinde ENOTTY dönmeli ki `isatty` yanlış sonuç vermesin.
    if !matches!(request, 0x5421 | 0x541b) {
        let terminal = inode.as_ref().map(|inode| inode.is_terminal()).unwrap_or(false);
        if !terminal {
            return -25; // ENOTTY
        }
    }

    match request {
        // TCGETS: `struct termios`. Bash/readline `isatty` ve satır kipi için
        // bunu okur; ICANON|ECHO|ISIG bildirilir.
        0x5401 => {
            let mut buffer = [0u8; 36];
            // c_iflag: ICRNL|IXON, c_oflag: OPOST|ONLCR, c_cflag: CS8|CREAD,
            // c_lflag: ISIG|ICANON|ECHO.
            buffer[0..4].copy_from_slice(&0x0000_0500u32.to_le_bytes());
            buffer[4..8].copy_from_slice(&0x0000_0005u32.to_le_bytes());
            buffer[8..12].copy_from_slice(&0x0000_00bfu32.to_le_bytes());
            buffer[12..16].copy_from_slice(&0x0000_000bu32.to_le_bytes());
            buffer[17] = 0x03; // VINTR
            buffer[18] = 0x1c; // VQUIT
            buffer[19] = 0x7f; // VERASE
            buffer[20] = 0x15; // VKILL
            buffer[21] = 0x04; // VEOF
            buffer[22] = 0x00; // VTIME
            buffer[23] = 0x01; // VMIN
            match copy_to_user(argument, &buffer) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
        0x5402 | 0x5403 | 0x5404 => 0, // TCSETS / TCSETSW / TCSETSF
        0x802c_542a => {
            // TCGETS2: `struct termios2` (termios + hız alanları).
            let mut buffer = [0u8; 44];
            buffer[0..4].copy_from_slice(&0x0000_0500u32.to_le_bytes());
            buffer[4..8].copy_from_slice(&0x0000_0005u32.to_le_bytes());
            buffer[8..12].copy_from_slice(&0x0000_00bfu32.to_le_bytes());
            buffer[12..16].copy_from_slice(&0x0000_000bu32.to_le_bytes());
            buffer[17] = 0x03;
            buffer[18] = 0x1c;
            buffer[19] = 0x7f;
            buffer[20] = 0x15;
            buffer[21] = 0x04;
            buffer[22] = 0x00;
            buffer[23] = 0x01;
            buffer[36..40].copy_from_slice(&38400u32.to_le_bytes());
            buffer[40..44].copy_from_slice(&38400u32.to_le_bytes());
            match copy_to_user(argument, &buffer) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
        0x402c_542b => 0, // TCSETS2
        0x402c_542c => 0, // TCSETSW2
        0x402c_542d => 0, // TCSETSF2
        0x5409 | 0x540a | 0x540b => 0, // TCGETS2 / TCSETS2 / TCSETSW2 (eski sabitler)
        0x5413 => {
            // TIOCGWINSZ: 24 satır x 80 sütun.
            let mut buffer = [0u8; 8];
            buffer[0..2].copy_from_slice(&24u16.to_le_bytes());
            buffer[2..4].copy_from_slice(&80u16.to_le_bytes());
            match copy_to_user(argument, &buffer) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
        0x5414 => 0, // TIOCSWINSZ
        0x540f => {
            // TIOCGPGRP: ön plan süreç grubu = kabuğun kendi pid'i.
            let group = crate::task::current_pid() as i32;
            match copy_to_user(argument, &group.to_le_bytes()) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
        0x5410 => 0, // TIOCSPGRP
        0x5429 => {
            // TIOCGSID
            let session = crate::task::current_pid() as i32;
            match copy_to_user(argument, &session.to_le_bytes()) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
        0x5419 | 0x541a => 0, // TIOCNOTTY / TIOCSCTTY sessizce yutulur
        0x540e => 0, // TIOCSCTTY
        0x5421 => 0, // FIONBIO
        0x541b => {
            // FIONREAD: okunmayı bekleyen bayt sayısı.
            let available = crate::drivers::ps2::pending();
            match copy_to_user(argument, &(available as i32).to_le_bytes()) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
        _ => -25, // ENOTTY
    }
}

pub fn stat(directory: i32, path_pointer: u64, buffer_pointer: u64) -> i64 {
    let _ = directory;
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };
    let inode = match fs::lookup_path(&path) {
        Ok(inode) => inode,
        Err(failure) => return error(failure),
    };
    write_stat(&inode, buffer_pointer)
}

/// `statx(dirfd, path, flags, mask, buffer)`.
pub fn statx(
    directory: i32,
    path_pointer: u64,
    flags: u32,
    mask: u32,
    buffer_pointer: u64,
) -> i64 {
    let _ = mask;
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };

    let inode = if path.is_empty() {
        if flags & 0x1000 == 0 {
            // AT_EMPTY_PATH yoksa boş yol geçersiz.
            return ENOENT;
        }
        let Some(process) = descriptors() else {
            return EBADF;
        };
        match process.fd_table.get(directory) {
            Ok(file) => file.inode.clone(),
            Err(_) => return EBADF,
        }
    } else {
        match fs::lookup_path(&path) {
            Ok(inode) => inode,
            Err(failure) => return error(failure),
        }
    };

    let metadata = inode.metadata();
    // x86_64 `struct statx` (256 bayt).
    let mut buffer = [0u8; 256];
    buffer[0..4].copy_from_slice(&0x7ffu32.to_le_bytes()); // stx_mask: BASIC_STATS
    buffer[4..8].copy_from_slice(&4096u32.to_le_bytes()); // stx_blksize
    buffer[16..20].copy_from_slice(&(metadata.links as u32).to_le_bytes());
    buffer[20..24].copy_from_slice(&metadata.uid.to_le_bytes());
    buffer[24..28].copy_from_slice(&metadata.gid.to_le_bytes());
    buffer[28..30].copy_from_slice(
        &((metadata.mode as u16) | mode_type(&metadata.file_type) as u16).to_le_bytes(),
    );
    buffer[32..40].copy_from_slice(&metadata.inode.to_le_bytes());
    buffer[40..48].copy_from_slice(&metadata.size.to_le_bytes());
    buffer[48..56].copy_from_slice(&metadata.blocks.to_le_bytes());

    match copy_to_user(buffer_pointer, &buffer) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

/// `prlimit64`: sınırları döndürür; değiştirmeyi yok sayar.
pub fn prlimit64(_pid: i32, resource: u32, new_limit: u64, old_limit: u64) -> i64 {
    let _ = new_limit;
    if old_limit == 0 {
        return 0;
    }
    let (current, maximum) = match resource {
        7 => (1024u64, 1024u64), // RLIMIT_NOFILE
        _ => (u64::MAX, u64::MAX),
    };
    let mut buffer = [0u8; 16];
    buffer[0..8].copy_from_slice(&current.to_le_bytes());
    buffer[8..16].copy_from_slice(&maximum.to_le_bytes());
    match copy_to_user(old_limit, &buffer) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

/// `metadata` yapısını Linux `struct stat` (x86_64, 144 bayt) olarak yazar.
fn write_stat(inode: &Arc<dyn Inode>, buffer_pointer: u64) -> i64 {
    let metadata = inode.metadata();

    // x86_64 `struct stat`: dev(0) ino(8) nlink(16) mode(24) uid(28)
    // gid(32) pad(36) rdev(40) size(48) blksize(56) blocks(64).
    let mut buffer = [0u8; 144];
    buffer[0..8].copy_from_slice(&0u64.to_le_bytes()); // st_dev
    buffer[8..16].copy_from_slice(&metadata.inode.to_le_bytes());
    buffer[16..24].copy_from_slice(&(metadata.links as u64).to_le_bytes());
    buffer[24..28].copy_from_slice(
        &(metadata.mode as u32 | mode_type(&metadata.file_type)).to_le_bytes(),
    );
    buffer[28..32].copy_from_slice(&metadata.uid.to_le_bytes());
    buffer[32..36].copy_from_slice(&metadata.gid.to_le_bytes());
    buffer[40..48].copy_from_slice(&0u64.to_le_bytes()); // st_rdev
    buffer[48..56].copy_from_slice(&metadata.size.to_le_bytes());
    buffer[56..64].copy_from_slice(&4096u64.to_le_bytes()); // blksize
    buffer[64..72].copy_from_slice(&metadata.blocks.to_le_bytes());

    match copy_to_user(buffer_pointer, &buffer) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

/// `fstat(fd, statbuf)`.
/// `access(path, mode)`.
pub fn access(path_pointer: u64, mode: u32) -> i64 {
    let _ = mode;
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };
    match fs::lookup_path(&path) {
        Ok(_) => 0,
        Err(failure) => error(failure),
    }
}

pub fn fstat(descriptor: i32, buffer_pointer: u64) -> i64 {
    let Some(process) = descriptors() else {
        return EBADF;
    };
    let file = match process.fd_table.get(descriptor) {
        Ok(file) => file,
        Err(_) => return EBADF,
    };
    write_stat(&file.inode.clone(), buffer_pointer)
}

/// `pread64(fd, buf, count, offset)`: dosya konumunu değiştirmeden okur.
pub fn pread64(descriptor: i32, pointer: u64, length: usize, offset: u64) -> i64 {
    if length == 0 {
        return 0;
    }
    let Some(process) = descriptors() else {
        return EBADF;
    };
    let file = match process.fd_table.get(descriptor) {
        Ok(file) => file,
        Err(_) => return EBADF,
    };
    let mut buffer = alloc::vec![0u8; length];
    let read = match file.inode.read_at(offset, &mut buffer) {
        Ok(read) => read,
        Err(failure) => return error(failure),
    };
    match copy_to_user(pointer, &buffer[..read]) {
        Ok(()) => read as i64,
        Err(code) => code,
    }
}

/// `getdents64` için `d_type` baytı.
fn dirent_type(file_type: &FileType) -> u8 {
    match file_type {
        FileType::Directory => 4,
        FileType::Regular => 8,
        FileType::Symlink => 10,
        FileType::CharDevice => 2,
        FileType::BlockDevice => 6,
        FileType::Fifo => 1,
        FileType::Socket => 12,
    }
}

fn mode_type(file_type: &FileType) -> u32 {
    match file_type {
        FileType::Directory => 0o040000,
        FileType::Regular => 0o100000,
        FileType::Symlink => 0o120000,
        FileType::CharDevice => 0o020000,
        FileType::BlockDevice => 0o060000,
        FileType::Fifo => 0o010000,
        FileType::Socket => 0o140000,
    }
}

pub fn getdents64(descriptor: i32, pointer: u64, length: usize) -> i64 {
    let Some(process) = descriptors() else {
        return EBADF;
    };
    let file = match process.fd_table.get(descriptor) {
        Ok(file) => file,
        Err(_) => return EBADF,
    };

    let entries = match file.inode.readdir() {
        Ok(entries) => entries,
        Err(failure) => return error(failure),
    };

    let mut buffer = alloc::vec![0u8; length];
    let mut written = 0usize;
    let start = file.offset.load(core::sync::atomic::Ordering::Relaxed) as usize;

    // `struct linux_dirent64`: ino(0) off(8) reclen(16) type(18) name(19).
    let mut consumed = start;
    for (index, (name, inode)) in entries.iter().enumerate().skip(start) {
        let metadata = inode.metadata();
        let name_bytes = name.as_bytes();
        // Linux `DIR_REC_LEN`: isim + sonlandırıcı NUL için bir bayt payı.
        let record = (19 + name_bytes.len() + 1 + 7) & !7;
        if written + record > buffer.len() {
            break;
        }
        let base = written;
        buffer[base..base + 8].copy_from_slice(&metadata.inode.to_le_bytes());
        buffer[base + 8..base + 16].copy_from_slice(&((base + record) as u64).to_le_bytes());
        buffer[base + 16..base + 18].copy_from_slice(&(record as u16).to_le_bytes());
        buffer[base + 18] = dirent_type(&metadata.file_type);
        buffer[base + 19..base + 19 + name_bytes.len()].copy_from_slice(name_bytes);
        written += record;
        consumed = index + 1;
    }

    if written == 0 {
        return 0;
    }

    file.offset
        .store(consumed as u64, core::sync::atomic::Ordering::Relaxed);
    match copy_to_user(pointer, &buffer[..written]) {
        Ok(()) => written as i64,
        Err(code) => code,
    }
}

pub fn readlink(path_pointer: u64, buffer_pointer: u64, length: usize) -> i64 {
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };
    let inode = match fs::lookup_path(&path) {
        Ok(inode) => inode,
        Err(failure) => return error(failure),
    };
    if inode.metadata().file_type != crate::fs::FileType::Symlink {
        return EINVAL;
    }
    let mut buffer = alloc::vec![0u8; length];
    match inode.read_at(0, &mut buffer) {
        Ok(count) => match copy_to_user(buffer_pointer, &buffer[..count]) {
            Ok(()) => count as i64,
            Err(code) => code,
        },
        Err(failure) => error(failure),
    }
}

pub fn execve(path_pointer: u64, argv_pointer: u64, envp_pointer: u64) -> i64 {
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };
    let arguments = read_user_argv(argv_pointer);
    let environment = read_user_argv(envp_pointer);

    let mut borrowed: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    for argument in &arguments {
        borrowed.push(argument.as_str());
    }
    let mut borrowed_environment: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    for variable in &environment {
        borrowed_environment.push(variable.as_str());
    }

    // `FD_CLOEXEC` işaretli tanıtıcılar yeni imaja taşınmaz.
    if let Some(process) = descriptors() {
        process.fd_table.close_cloexec();
    }

    match crate::task::exec_path(&path, &borrowed, &borrowed_environment) {
        Ok(()) => 0,
        Err(reason) => {
            crate::println!("execve: {} -> {}", path, reason);
            ENOENT
        }
    }
}

/// Kullanıcı `argv` dizisini okur: NULL'a kadar işaretçiler, her biri C dizesi.
fn read_user_argv(pointer: u64) -> alloc::vec::Vec<String> {
    let mut arguments = alloc::vec::Vec::new();
    if pointer == 0 {
        return arguments;
    }

    for index in 0..64u64 {
        let entry_pointer = match copy_from_user(pointer + index * 8, 8) {
            Ok(bytes) => u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]),
            Err(_) => break,
        };
        if entry_pointer == 0 {
            break;
        }
        match read_user_string(entry_pointer) {
            Ok(argument) => arguments.push(argument),
            Err(_) => break,
        }
    }

    arguments
}

pub fn getcwd(pointer: u64, length: usize) -> i64 {
    let cwd = fs::getcwd();
    let bytes = cwd.as_bytes();
    if length < bytes.len() + 1 {
        return -34; // ERANGE
    }
    let mut buffer = alloc::vec![0u8; bytes.len() + 1];
    buffer[..bytes.len()].copy_from_slice(bytes);
    match copy_to_user(pointer, &buffer) {
        Ok(()) => (bytes.len() + 1) as i64,
        Err(code) => code,
    }
}

/// `chdir(path)`.
pub fn chdir(pointer: u64) -> i64 {
    let path = match read_user_string(pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };
    match fs::chdir(&path) {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

fn split_parent(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(0) => ("/", &path[1..]),
        Some(index) => (&path[..index], &path[index + 1..]),
        None => ("/", path),
    }
}

fn read_user_string(pointer: u64) -> core::result::Result<String, i64> {
    let mut bytes = alloc::vec::Vec::with_capacity(128);
    for offset in 0..4096u64 {
        let mut byte = [0u8; 1];
        let address = pointer + offset;
        let copy = copy_from_user(address, 1)?;
        byte[0] = copy[0];
        if byte[0] == 0 {
            break;
        }
        bytes.push(byte[0]);
    }
    String::from_utf8(bytes).map_err(|_| EINVAL)
}

/// Kullanılmayan uyarıları önlemek için niyet belgesi.
const _: i64 = EPERM;

/// `fd_set` boyutu: 1024 tanımlayıcı.
const FD_SET_BYTES: usize = 128;

const POLLIN: u16 = 0x0001;
const POLLOUT: u16 = 0x0004;
const POLLERR: u16 = 0x0008;
const POLLHUP: u16 = 0x0010;
const POLLNVAL: u16 = 0x0020;

fn fd_set_test(set: &[u8], descriptor: usize) -> bool {
    match set.get(descriptor / 8) {
        Some(byte) => byte & (1 << (descriptor % 8)) != 0,
        None => false,
    }
}

fn fd_set_add(set: &mut [u8], descriptor: usize) {
    if let Some(byte) = set.get_mut(descriptor / 8) {
        *byte |= 1 << (descriptor % 8);
    }
}

fn read_set(pointer: u64, bytes: usize) -> core::result::Result<alloc::vec::Vec<u8>, i64> {
    if pointer == 0 || bytes == 0 {
        return Ok(alloc::vec![0u8; bytes]);
    }
    copy_from_user(pointer, bytes)
}

/// `select` süre sınırı: `None` sonsuz bekleme demek.
fn timeval_deadline(pointer: u64) -> core::result::Result<Option<u64>, i64> {
    if pointer == 0 {
        return Ok(None);
    }
    let data = copy_from_user(pointer, 16)?;
    let seconds = u64::from_le_bytes(data[0..8].try_into().unwrap());
    let micros = u64::from_le_bytes(data[8..16].try_into().unwrap());
    Ok(Some(
        crate::time::uptime_ms() + seconds.saturating_mul(1000) + micros / 1000,
    ))
}

/// `poll` süre sınırı: negatif değer sonsuz bekleme demek.
fn timespec_deadline(pointer: u64) -> core::result::Result<Option<u64>, i64> {
    if pointer == 0 {
        return Ok(None);
    }
    let data = copy_from_user(pointer, 16)?;
    let seconds = i64::from_le_bytes(data[0..8].try_into().unwrap());
    let nanos = i64::from_le_bytes(data[8..16].try_into().unwrap());
    if seconds < 0 || nanos < 0 {
        return Ok(None);
    }
    Ok(Some(
        crate::time::uptime_ms() + (seconds as u64) * 1000 + (nanos as u64) / 1_000_000,
    ))
}

/// `select(nfds, readfds, writefds, exceptfds, timeout)`.
pub fn select(nfds: i32, readfds: u64, writefds: u64, _exceptfds: u64, timeout: u64) -> i64 {
    let count = nfds.clamp(0, 1024) as usize;
    let bytes = count.div_ceil(64) * 8;

    let read_in = match read_set(readfds, bytes) {
        Ok(set) => set,
        Err(code) => return code,
    };
    let write_in = match read_set(writefds, bytes) {
        Ok(set) => set,
        Err(code) => return code,
    };
    let deadline = match timeval_deadline(timeout) {
        Ok(value) => value,
        Err(code) => return code,
    };

    loop {
        let mut read_out = alloc::vec![0u8; bytes];
        let mut write_out = alloc::vec![0u8; bytes];
        let mut ready = 0i64;

        for descriptor in 0..count {
            if fd_set_test(&read_in, descriptor) {
                match fs::descriptor_ready(descriptor as i32, false) {
                    Some(true) => {
                        fd_set_add(&mut read_out, descriptor);
                        ready += 1;
                    }
                    Some(false) => {}
                    None => return EBADF,
                }
            }
            if fd_set_test(&write_in, descriptor) {
                match fs::descriptor_ready(descriptor as i32, true) {
                    Some(true) => {
                        fd_set_add(&mut write_out, descriptor);
                        ready += 1;
                    }
                    Some(false) => {}
                    None => return EBADF,
                }
            }
        }

        let expired = match deadline {
            Some(limit) => crate::time::uptime_ms() >= limit,
            None => false,
        };

        if ready > 0 || expired {
            if readfds != 0 && bytes > 0 {
                if let Err(code) = copy_to_user(readfds, &read_out) {
                    return code;
                }
            }
            if writefds != 0 && bytes > 0 {
                if let Err(code) = copy_to_user(writefds, &write_out) {
                    return code;
                }
            }
            return ready;
        }

        if crate::task::signal::interrupted() {
            return -4; // EINTR
        }
        crate::task::schedule();
    }
}

/// `poll(fds, nfds, timeout_ms)`.
pub fn poll(fds: u64, nfds: u64, timeout: i64) -> i64 {
    let count = (nfds as usize).min(1024);
    if fds == 0 || count == 0 {
        return 0;
    }
    let data = match copy_from_user(fds, count * 8) {
        Ok(data) => data,
        Err(code) => return code,
    };
    let deadline = if timeout < 0 {
        None
    } else {
        Some(crate::time::uptime_ms() + timeout as u64)
    };

    loop {
        let mut output = alloc::vec![0u8; count * 8];
        let mut ready = 0i64;

        for index in 0..count {
            let base = index * 8;
            let descriptor = i32::from_le_bytes(data[base..base + 4].try_into().unwrap());
            let events = u16::from_le_bytes(data[base + 4..base + 6].try_into().unwrap());
            let mut revents = 0u16;

            if descriptor >= 0 {
                if events & POLLIN != 0 {
                    match fs::descriptor_ready(descriptor, false) {
                        Some(true) => revents |= POLLIN,
                        Some(false) => {}
                        None => revents |= POLLNVAL,
                    }
                }
                if events & POLLOUT != 0 {
                    match fs::descriptor_ready(descriptor, true) {
                        Some(true) => revents |= POLLOUT,
                        Some(false) => {}
                        None => revents |= POLLNVAL,
                    }
                }
                if revents & POLLNVAL == 0 && events & POLLIN != 0 && !fs::descriptor_ready(descriptor, false).unwrap_or(true) {
                    // hâlâ veri yok; HUP bildirimi için ayrı bir yol yok
                }
                if revents == 0 {
                    let _ = (POLLERR, POLLHUP);
                }
            }

            if revents != 0 {
                ready += 1;
            }
            output[base..base + 4].copy_from_slice(&descriptor.to_le_bytes());
            output[base + 4..base + 6].copy_from_slice(&events.to_le_bytes());
            output[base + 6..base + 8].copy_from_slice(&revents.to_le_bytes());
        }

        let expired = match deadline {
            Some(limit) => crate::time::uptime_ms() >= limit,
            None => false,
        };

        if ready > 0 || expired {
            if let Err(code) = copy_to_user(fds, &output) {
                return code;
            }
            return ready;
        }

        if crate::task::signal::interrupted() {
            return -4; // EINTR
        }
        crate::task::schedule();
    }
}

/// `pselect6(nfds, readfds, writefds, exceptfds, timeout, sigmask)`.
pub fn pselect6(nfds: i32, readfds: u64, writefds: u64, exceptfds: u64, timeout: u64) -> i64 {
    select(nfds, readfds, writefds, exceptfds, timeout)
}

/// `ppoll(fds, nfds, timeout, sigmask)`.
pub fn ppoll(fds: u64, nfds: u64, timeout: u64) -> i64 {
    let deadline = match timespec_deadline(timeout) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let millis = match deadline {
        Some(limit) => {
            let now = crate::time::uptime_ms();
            if limit <= now {
                0
            } else {
                (limit - now) as i64
            }
        }
        None => -1,
    };
    poll(fds, nfds, millis)
}

// ---------------------------------------------------------------------------
// Anonim fd'ler: epoll, eventfd, timerfd, memfd, socketpair
// ---------------------------------------------------------------------------

const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;

const EPOLL_CTL_ADD: u32 = 1;
const EPOLL_CTL_DEL: u32 = 2;
const EPOLL_CTL_MOD: u32 = 3;

const EFD_SEMAPHORE: u32 = 1;
const EFD_NONBLOCK: u32 = 0o4000;
const EFD_CLOEXEC: u32 = 0o2000000;

fn inode_of(descriptor: i32) -> Option<Arc<dyn Inode>> {
    let process = descriptors()?;
    Some(process.fd_table.get(descriptor).ok()?.inode.clone())
}

pub fn ftruncate(descriptor: i32, length: i64) -> i64 {
    if length < 0 {
        return EINVAL;
    }
    let Some(process) = descriptors() else {
        return EBADF;
    };
    let file = match process.fd_table.get(descriptor) {
        Ok(file) => file,
        Err(_) => return EBADF,
    };
    match file.inode.truncate(length as u64) {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

pub fn epoll_create1(flags: u32) -> i64 {
    let inode = Arc::new(crate::fs::epoll::EpollInode::new());
    match fs::install_anonymous(inode, 2, false) {
        Ok(descriptor) => {
            if flags & fs::O_CLOEXEC != 0 {
                if let Some(process) = descriptors() {
                    process.fd_table.set_cloexec(descriptor, true);
                }
            }
            descriptor as i64
        }
        Err(failure) => error(failure),
    }
}

pub fn epoll_ctl(epoll_descriptor: i32, operation: u32, descriptor: i32, event_pointer: u64) -> i64 {
    let Some(inode) = inode_of(epoll_descriptor) else {
        return EBADF;
    };
    let Some(epoll) = inode.as_epoll() else {
        return EINVAL;
    };

    if operation == EPOLL_CTL_DEL {
        return match epoll.remove(descriptor) {
            Ok(()) => 0,
            Err(failure) => error(failure),
        };
    }

    if inode_of(descriptor).is_none() {
        return EBADF;
    }

    let data = match copy_from_user(event_pointer, 12) {
        Ok(bytes) => bytes,
        Err(code) => return code,
    };
    let events = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let user_data = u64::from_le_bytes([
        data[4], data[5], data[6], data[7], data[8], data[9], data[10], data[11],
    ]);

    let result = if operation == EPOLL_CTL_ADD {
        epoll.add(descriptor, events, user_data)
    } else if operation == EPOLL_CTL_MOD {
        epoll.modify(descriptor, events, user_data)
    } else {
        return EINVAL;
    };

    match result {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

/// `epoll_wait(epfd, events, maxevents, timeout_ms)`.
pub fn epoll_wait(
    epoll_descriptor: i32,
    events_pointer: u64,
    max_events: i32,
    timeout: i32,
) -> i64 {
    if max_events <= 0 {
        return EINVAL;
    }
    let Some(inode) = inode_of(epoll_descriptor) else {
        return EBADF;
    };
    let Some(epoll) = inode.as_epoll() else {
        return EINVAL;
    };
    let Some(process) = descriptors() else {
        return EBADF;
    };

    let limit = if timeout < 0 {
        None
    } else {
        Some(crate::time::uptime_ms() + timeout as u64)
    };

    loop {
        let mut output: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        let mut reported = 0i32;

        for entry in epoll.entries() {
            if reported >= max_events {
                break;
            }
            let file = match process.fd_table.get(entry.descriptor) {
                Ok(file) => file,
                Err(_) => continue, // kapanmış fd sessizce atlanır
            };
            let mut revents = 0u32;
            if entry.events & EPOLLIN != 0 && file.inode.ready(false) {
                revents |= EPOLLIN;
            }
            if entry.events & EPOLLOUT != 0 && file.inode.ready(true) {
                revents |= EPOLLOUT;
            }
            if revents == 0 {
                continue;
            }
            revents |= entry.events & (EPOLLERR | EPOLLHUP);
            output.extend_from_slice(&revents.to_le_bytes());
            output.extend_from_slice(&entry.data.to_le_bytes());
            reported += 1;
        }

        if reported > 0 {
            return match copy_to_user(events_pointer, &output) {
                Ok(()) => reported as i64,
                Err(code) => code,
            };
        }

        if let Some(limit) = limit {
            if crate::time::uptime_ms() >= limit {
                return 0;
            }
        }
        if crate::task::signal::interrupted() {
            return super::EINTR;
        }
        crate::task::schedule();
    }
}

pub fn eventfd2(initial: u32, flags: u32) -> i64 {
    let inode = Arc::new(crate::fs::anon::EventFdInode::new(
        initial as u64,
        flags & EFD_SEMAPHORE != 0,
    ));
    match fs::install_anonymous(inode, 2, flags & EFD_NONBLOCK != 0) {
        Ok(descriptor) => {
            if flags & EFD_CLOEXEC != 0 {
                if let Some(process) = descriptors() {
                    process.fd_table.set_cloexec(descriptor, true);
                }
            }
            descriptor as i64
        }
        Err(failure) => error(failure),
    }
}

pub fn timerfd_create(_clock: i32, flags: u32) -> i64 {
    let inode = Arc::new(crate::fs::anon::TimerFdInode::new());
    match fs::install_anonymous(inode, 2, flags & EFD_NONBLOCK != 0) {
        Ok(descriptor) => {
            if flags & EFD_CLOEXEC != 0 {
                if let Some(process) = descriptors() {
                    process.fd_table.set_cloexec(descriptor, true);
                }
            }
            descriptor as i64
        }
        Err(failure) => error(failure),
    }
}

fn timespec_millis(bytes: &[u8]) -> u64 {
    let seconds = i64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let nanos = i64::from_le_bytes([
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ]);
    if seconds <= 0 && nanos <= 0 {
        return 0;
    }
    (seconds.max(0) as u64) * 1000 + (nanos.max(0) as u64) / 1_000_000
}

fn millis_timespec(milliseconds: u64) -> [u8; 16] {
    let mut output = [0u8; 16];
    output[..8].copy_from_slice(&((milliseconds / 1000) as i64).to_le_bytes());
    output[8..].copy_from_slice(&(((milliseconds % 1000) * 1_000_000) as i64).to_le_bytes());
    output
}

pub fn timerfd_settime(
    descriptor: i32,
    _flags: u32,
    new_value: u64,
    old_value: u64,
) -> i64 {
    let Some(inode) = inode_of(descriptor) else {
        return EBADF;
    };
    let Some(timer) = inode.as_timerfd() else {
        return EINVAL;
    };
    let data = match copy_from_user(new_value, 32) {
        Ok(data) => data,
        Err(code) => return code,
    };
    if old_value != 0 {
        let (remaining, interval) = timer.values();
        let mut previous = [0u8; 32];
        previous[..16].copy_from_slice(&millis_timespec(interval));
        previous[16..].copy_from_slice(&millis_timespec(remaining));
        if let Err(code) = copy_to_user(old_value, &previous) {
            return code;
        }
    }
    let interval = timespec_millis(&data[..16]);
    let value = timespec_millis(&data[16..32]);
    timer.arm(value, interval);
    0
}

pub fn timerfd_gettime(descriptor: i32, current_value: u64) -> i64 {
    let Some(inode) = inode_of(descriptor) else {
        return EBADF;
    };
    let Some(timer) = inode.as_timerfd() else {
        return EINVAL;
    };
    let (remaining, interval) = timer.values();
    let mut output = [0u8; 32];
    output[..16].copy_from_slice(&millis_timespec(interval));
    output[16..].copy_from_slice(&millis_timespec(remaining));
    match copy_to_user(current_value, &output) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

pub fn memfd_create(name_pointer: u64, flags: u32) -> i64 {
    let name = match read_user_string(name_pointer) {
        Ok(name) => name,
        Err(code) => return code,
    };
    let inode = Arc::new(crate::fs::anon::MemFdInode::new(&name));
    match fs::install_anonymous(inode, 2, flags & EFD_NONBLOCK != 0) {
        Ok(descriptor) => {
            if flags & 0x1 != 0 {
                if let Some(process) = descriptors() {
                    process.fd_table.set_cloexec(descriptor, true);
                }
            }
            descriptor as i64
        }
        Err(failure) => error(failure),
    }
}

// --- AF_UNIX soketleri ----------------------------------------------------

const SOL_SOCKET: i32 = 1;
const SO_TYPE: i32 = 3;
const SO_ERROR: i32 = 4;
const SO_SNDBUF: i32 = 7;
const SO_RCVBUF: i32 = 8;
const SO_PEERCRED: i32 = 17;
const SO_ACCEPTCONN: i32 = 30;
const SCM_RIGHTS: i32 = 1;
const SOCK_CLOEXEC: u32 = 0o2000000;
const SOCK_NONBLOCK: u32 = 0o4000;
const MSG_DONTWAIT: u32 = 0x40;
const MSG_CMSG_CLOEXEC: u32 = 0x4000_0000;
const MSG_CTRUNC: u32 = 0x8;
const ENOTSOCK: i64 = -88;
const EAFNOSUPPORT: i64 = -97;
const EPROTONOSUPPORT: i64 = -93;

fn socket_file(descriptor: i32) -> core::result::Result<Arc<OpenFile>, i64> {
    let process = descriptors().ok_or(EBADF)?;
    process.fd_table.get(descriptor).map_err(|_| EBADF)
}

pub fn socket(domain: u32, kind: u32, _protocol: u32) -> i64 {
    if domain != crate::fs::unix::AF_UNIX {
        return EAFNOSUPPORT;
    }
    let base = kind & 0xf;
    if base != crate::fs::unix::SOCK_STREAM && base != crate::fs::unix::SOCK_DGRAM {
        return EPROTONOSUPPORT;
    }
    let inode = crate::fs::unix::UnixSocketInode::create(base);
    let descriptor = match fs::install_anonymous(inode, 2, kind & SOCK_NONBLOCK != 0) {
        Ok(descriptor) => descriptor,
        Err(failure) => return error(failure),
    };
    if kind & SOCK_CLOEXEC != 0 {
        if let Some(process) = descriptors() {
            process.fd_table.set_cloexec(descriptor, true);
        }
    }
    descriptor as i64
}

pub fn socketpair(domain: u32, kind: u32, _protocol: u32, output_pointer: u64) -> i64 {
    if domain != crate::fs::unix::AF_UNIX {
        return EAFNOSUPPORT;
    }
    let base = kind & 0xf;
    if base != crate::fs::unix::SOCK_STREAM && base != crate::fs::unix::SOCK_DGRAM {
        return EPROTONOSUPPORT;
    }

    let (first, second) = crate::fs::unix::UnixSocketInode::pair(base);
    let nonblock = kind & SOCK_NONBLOCK != 0;
    let first_descriptor = match fs::install_anonymous(first, 2, nonblock) {
        Ok(descriptor) => descriptor,
        Err(failure) => return error(failure),
    };
    let second_descriptor = match fs::install_anonymous(second, 2, nonblock) {
        Ok(descriptor) => descriptor,
        Err(failure) => {
            let _ = close(first_descriptor);
            return error(failure);
        }
    };

    if let Some(process) = descriptors() {
        if kind & SOCK_CLOEXEC != 0 {
            process.fd_table.set_cloexec(first_descriptor, true);
            process.fd_table.set_cloexec(second_descriptor, true);
        }
    }

    let mut buffer = [0u8; 8];
    buffer[..4].copy_from_slice(&first_descriptor.to_le_bytes());
    buffer[4..].copy_from_slice(&second_descriptor.to_le_bytes());
    match copy_to_user(output_pointer, &buffer) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

/// `sockaddr_un` çözümlemesi. Soyut adlar `0` önekli anahtarla saklanır.
fn read_address(pointer: u64, length: usize) -> core::result::Result<alloc::vec::Vec<u8>, i64> {
    if pointer == 0 || length < 2 {
        return Err(EINVAL);
    }
    if length > 110 {
        return Err(EINVAL);
    }
    let bytes = copy_from_user(pointer, length)?;
    let family = u16::from_le_bytes([bytes[0], bytes[1]]) as u32;
    if family != crate::fs::unix::AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    let path = &bytes[2..];
    if path.is_empty() {
        return Err(EINVAL);
    }
    if path[0] == 0 {
        let mut key = alloc::vec![0u8];
        key.extend_from_slice(&path[1..]);
        Ok(key)
    } else {
        let end = path.iter().position(|byte| *byte == 0).unwrap_or(path.len());
        if end == 0 {
            return Err(EINVAL);
        }
        Ok(path[..end].to_vec())
    }
}

fn write_address(pointer: u64, length_pointer: u64, key: Option<&[u8]>) -> i64 {
    if pointer == 0 || length_pointer == 0 {
        return 0;
    }
    let capacity = match copy_from_user(length_pointer, 4) {
        Ok(bytes) => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize,
        Err(code) => return code,
    };
    let mut buffer = alloc::vec![0u8; capacity.max(2)];
    buffer[0..2].copy_from_slice(&(crate::fs::unix::AF_UNIX as u16).to_le_bytes());
    let mut used = 2usize;
    if let Some(key) = key {
        if key.first() == Some(&0) {
            let name = &key[1..];
            let available = capacity.saturating_sub(3);
            let count = name.len().min(available);
            buffer[3..3 + count].copy_from_slice(&name[..count]);
            used = 3 + count;
        } else {
            let available = capacity.saturating_sub(3);
            let count = key.len().min(available);
            buffer[2..2 + count].copy_from_slice(&key[..count]);
            used = 2 + count + 1;
        }
    }
    if let Err(code) = copy_to_user(pointer, &buffer[..used.min(capacity)]) {
        return code;
    }
    let length = used.min(capacity) as u32;
    match copy_to_user(length_pointer, &length.to_le_bytes()) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

pub fn bind(descriptor: i32, pointer: u64, length: usize) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    let Some(socket) = file.inode.as_unix() else {
        return ENOTSOCK;
    };
    let key = match read_address(pointer, length) {
        Ok(key) => key,
        Err(code) => return code,
    };
    let handle = Arc::downgrade(&file.inode);
    let name = if key.first() == Some(&0) {
        None
    } else {
        core::str::from_utf8(&key).ok().map(alloc::string::String::from)
    };
    match socket.bind(key, handle) {
        Ok(()) => {
            // Yol adlı soket dosya sisteminde de görünür (stat/chmod/unlink).
            if let Some(name) = name {
                if let Ok((parent, base)) = fs::parent_and_name(&name) {
                    let _ = parent.create(&base, FileType::Socket, 0o777);
                }
            }
            0
        }
        Err(failure) => error(failure),
    }
}

pub fn listen(descriptor: i32, backlog: i32) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    let Some(socket) = file.inode.as_unix() else {
        return ENOTSOCK;
    };
    let backlog = if backlog < 1 { 1 } else { backlog as usize };
    match socket.listen(backlog) {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

pub fn connect(descriptor: i32, pointer: u64, length: usize) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    let Some(socket) = file.inode.as_unix() else {
        return ENOTSOCK;
    };
    let key = match read_address(pointer, length) {
        Ok(key) => key,
        Err(code) => return code,
    };
    match socket.connect(&key) {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

pub fn accept4(descriptor: i32, address_pointer: u64, length_pointer: u64, flags: u32) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    let Some(socket) = file.inode.as_unix() else {
        return ENOTSOCK;
    };
    if !socket.is_listening() {
        return EINVAL;
    }
    if (file.is_nonblock() || flags & SOCK_NONBLOCK != 0) && !file.inode.ready(false) {
        return EAGAIN;
    }
    let server = match socket.accept() {
        Ok(server) => server,
        Err(failure) => return error(failure),
    };
    let nonblock = file.is_nonblock() || flags & SOCK_NONBLOCK != 0;
    let descriptor = match fs::install_anonymous(server, 2, nonblock) {
        Ok(descriptor) => descriptor,
        Err(failure) => return error(failure),
    };
    if flags & SOCK_CLOEXEC != 0 {
        if let Some(process) = descriptors() {
            process.fd_table.set_cloexec(descriptor, true);
        }
    }
    let code = write_address(address_pointer, length_pointer, None);
    if code < 0 {
        let _ = close(descriptor);
        return code;
    }
    descriptor as i64
}

pub fn getsockname(descriptor: i32, address_pointer: u64, length_pointer: u64) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    let Some(socket) = file.inode.as_unix() else {
        return ENOTSOCK;
    };
    let name = socket.bound_name();
    write_address(address_pointer, length_pointer, name.as_deref())
}

pub fn getpeername(descriptor: i32, address_pointer: u64, length_pointer: u64) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    if file.inode.as_unix().is_none() {
        return ENOTSOCK;
    }
    write_address(address_pointer, length_pointer, None)
}

pub fn shutdown(descriptor: i32, how: u32) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    let Some(socket) = file.inode.as_unix() else {
        return ENOTSOCK;
    };
    match socket.shutdown(how) {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

pub fn setsockopt(
    descriptor: i32,
    _level: i32,
    _option: i32,
    _value: u64,
    _length: usize,
) -> i64 {
    match socket_file(descriptor) {
        Ok(file) if file.inode.as_unix().is_some() => 0,
        Ok(_) => ENOTSOCK,
        Err(code) => code,
    }
}

pub fn getsockopt(
    descriptor: i32,
    level: i32,
    option: i32,
    value: u64,
    length_pointer: u64,
) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    let Some(socket) = file.inode.as_unix() else {
        return ENOTSOCK;
    };
    if level != SOL_SOCKET {
        return EINVAL;
    }
    let payload: alloc::vec::Vec<u8> = match option {
        SO_TYPE => (socket.kind() as i32).to_le_bytes().to_vec(),
        SO_ERROR => 0i32.to_le_bytes().to_vec(),
        SO_SNDBUF | SO_RCVBUF => (64 * 1024i32).to_le_bytes().to_vec(),
        SO_ACCEPTCONN => (socket.is_listening() as i32).to_le_bytes().to_vec(),
        SO_PEERCRED => {
            // pid, uid, gid: tek kullanıcılı sistem, kimlikler 0.
            let mut bytes = alloc::vec::Vec::new();
            bytes.extend_from_slice(&1i32.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes
        }
        _ => return EINVAL,
    };
    let capacity = match copy_from_user(length_pointer, 4) {
        Ok(bytes) => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize,
        Err(code) => return code,
    };
    let count = capacity.min(payload.len());
    if let Err(code) = copy_to_user(value, &payload[..count]) {
        return code;
    }
    match copy_to_user(length_pointer, &(count as u32).to_le_bytes()) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

// --- sendmsg / recvmsg ----------------------------------------------------

struct MessageHeader {
    name: u64,
    name_length: u32,
    vectors: u64,
    vector_count: u64,
    control: u64,
    control_length: u64,
    flags: u32,
}

fn read_message_header(pointer: u64) -> core::result::Result<MessageHeader, i64> {
    let raw = copy_from_user(pointer, 56)?;
    Ok(MessageHeader {
        name: u64::from_le_bytes(raw[0..8].try_into().unwrap()),
        name_length: u32::from_le_bytes(raw[8..12].try_into().unwrap()),
        vectors: u64::from_le_bytes(raw[16..24].try_into().unwrap()),
        vector_count: u64::from_le_bytes(raw[24..32].try_into().unwrap()),
        control: u64::from_le_bytes(raw[32..40].try_into().unwrap()),
        control_length: u64::from_le_bytes(raw[40..48].try_into().unwrap()),
        flags: u32::from_le_bytes(raw[48..52].try_into().unwrap()),
    })
}

fn gather_vectors(pointer: u64, count: u64) -> core::result::Result<alloc::vec::Vec<u8>, i64> {
    let mut data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if count == 0 {
        return Ok(data);
    }
    if count > 1024 {
        return Err(EINVAL);
    }
    let table = copy_from_user(pointer, count as usize * 16)?;
    for index in 0..count as usize {
        let base = u64::from_le_bytes(table[index * 16..index * 16 + 8].try_into().unwrap());
        let length =
            u64::from_le_bytes(table[index * 16 + 8..index * 16 + 16].try_into().unwrap()) as usize;
        if length == 0 {
            continue;
        }
        if data.len() + length > 1024 * 1024 {
            return Err(EINVAL);
        }
        data.extend_from_slice(&copy_from_user(base, length)?);
    }
    Ok(data)
}

fn vector_capacity(pointer: u64, count: u64) -> core::result::Result<usize, i64> {
    if count == 0 {
        return Ok(0);
    }
    if count > 1024 {
        return Err(EINVAL);
    }
    let table = copy_from_user(pointer, count as usize * 16)?;
    let mut total = 0usize;
    for index in 0..count as usize {
        let length =
            u64::from_le_bytes(table[index * 16 + 8..index * 16 + 16].try_into().unwrap()) as usize;
        total = total.saturating_add(length);
    }
    Ok(total.min(1024 * 1024))
}

fn scatter_vectors(
    pointer: u64,
    count: u64,
    data: &[u8],
) -> core::result::Result<(), i64> {
    if count == 0 {
        return Ok(());
    }
    let table = copy_from_user(pointer, count as usize * 16)?;
    let mut offset = 0usize;
    for index in 0..count as usize {
        let base = u64::from_le_bytes(table[index * 16..index * 16 + 8].try_into().unwrap());
        let length =
            u64::from_le_bytes(table[index * 16 + 8..index * 16 + 16].try_into().unwrap()) as usize;
        let count = length.min(data.len() - offset);
        if count > 0 {
            copy_to_user(base, &data[offset..offset + count])?;
            offset += count;
        }
        if offset >= data.len() {
            break;
        }
    }
    Ok(())
}

fn parse_rights(control: u64, length: usize) -> core::result::Result<alloc::vec::Vec<Arc<OpenFile>>, i64> {
    let mut files = alloc::vec::Vec::new();
    if control == 0 || length < 16 {
        return Ok(files);
    }
    let bytes = copy_from_user(control, length)?;
    let mut offset = 0usize;
    while offset + 16 <= bytes.len() {
        let header = &bytes[offset..];
        let cmsg_length = u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
        let level = i32::from_le_bytes(header[8..12].try_into().unwrap());
        let kind = i32::from_le_bytes(header[12..16].try_into().unwrap());
        if cmsg_length < 16 || offset + cmsg_length > bytes.len() {
            break;
        }
        if level == SOL_SOCKET && kind == SCM_RIGHTS {
            let count = (cmsg_length - 16) / 4;
            let process = descriptors().ok_or(EBADF)?;
            for index in 0..count {
                let start = offset + 16 + index * 4;
                let value = i32::from_le_bytes(bytes[start..start + 4].try_into().unwrap());
                match process.fd_table.get(value) {
                    Ok(file) => files.push(file),
                    Err(_) => return Err(EBADF),
                }
            }
        }
        offset += (cmsg_length + 7) & !7;
    }
    Ok(files)
}

pub fn sendmsg(descriptor: i32, header_pointer: u64, flags: u32) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    if file.inode.as_unix().is_none() {
        return ENOTSOCK;
    }
    let header = match read_message_header(header_pointer) {
        Ok(header) => header,
        Err(code) => return code,
    };
    let data = match gather_vectors(header.vectors, header.vector_count) {
        Ok(data) => data,
        Err(code) => return code,
    };
    let files = match parse_rights(header.control, header.control_length as usize) {
        Ok(files) => files,
        Err(code) => return code,
    };
    if (file.is_nonblock() || flags & MSG_DONTWAIT != 0) && !file.inode.ready(true) {
        return EAGAIN;
    }
    match file.inode.send_message(&data, &files) {
        Ok(count) => count as i64,
        Err(failure) => error(failure),
    }
}

pub fn recvmsg(descriptor: i32, header_pointer: u64, flags: u32) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    if file.inode.as_unix().is_none() {
        return ENOTSOCK;
    }
    let header = match read_message_header(header_pointer) {
        Ok(header) => header,
        Err(code) => return code,
    };
    let capacity = match vector_capacity(header.vectors, header.vector_count) {
        Ok(capacity) => capacity,
        Err(code) => return code,
    };
    let max_files = if header.control_length >= 20 {
        ((header.control_length as usize - 16) / 4).min(253)
    } else {
        0
    };
    if (file.is_nonblock() || flags & MSG_DONTWAIT != 0) && !file.inode.ready(false) {
        return EAGAIN;
    }

    let mut buffer = alloc::vec![0u8; capacity];
    let (count, files) = match file.inode.recv_message(&mut buffer, max_files) {
        Ok(result) => result,
        Err(failure) => return error(failure),
    };
    if let Err(code) = scatter_vectors(header.vectors, header.vector_count, &buffer[..count]) {
        return code;
    }

    let mut received = alloc::vec::Vec::new();
    if !files.is_empty() {
        let Some(process) = descriptors() else {
            return EBADF;
        };
        for incoming in files {
            if let Ok(number) = process.fd_table.insert(incoming) {
                if flags & MSG_CMSG_CLOEXEC != 0 {
                    process.fd_table.set_cloexec(number, true);
                }
                received.push(number);
            }
        }
    }

    let mut output_flags = 0u32;
    let mut control_used = 0usize;
    if !received.is_empty() {
        let needed = 16 + received.len() * 4;
        if header.control != 0 && needed <= header.control_length as usize {
            let mut control = alloc::vec![0u8; (needed + 7) & !7];
            control[0..8].copy_from_slice(&(needed as u64).to_le_bytes());
            control[8..12].copy_from_slice(&SOL_SOCKET.to_le_bytes());
            control[12..16].copy_from_slice(&SCM_RIGHTS.to_le_bytes());
            for (index, number) in received.iter().enumerate() {
                let start = 16 + index * 4;
                control[start..start + 4].copy_from_slice(&number.to_le_bytes());
            }
            if let Err(code) = copy_to_user(header.control, &control) {
                return code;
            }
            control_used = control.len();
        } else {
            output_flags |= MSG_CTRUNC;
        }
    }

    let mut control_length = control_used as u64;
    if let Err(code) = copy_to_user(header_pointer + 40, &control_length.to_le_bytes()) {
        return code;
    }
    if let Err(code) = copy_to_user(header_pointer + 48, &output_flags.to_le_bytes()) {
        return code;
    }
    control_length = 0; // sustur: kullanılmıyor
    let zero = 0u32;
    if let Err(code) = copy_to_user(header_pointer + 8, &zero.to_le_bytes()) {
        return code;
    }
    count as i64
}

pub fn sendto(
    descriptor: i32,
    pointer: u64,
    length: usize,
    _flags: u32,
    _address: u64,
    _address_length: usize,
) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    if file.inode.as_unix().is_none() {
        return ENOTSOCK;
    }
    let data = match copy_from_user(pointer, length) {
        Ok(data) => data,
        Err(code) => return code,
    };
    match file.inode.send_message(&data, &[]) {
        Ok(count) => count as i64,
        Err(failure) => error(failure),
    }
}

pub fn recvfrom(
    descriptor: i32,
    pointer: u64,
    length: usize,
    flags: u32,
    address: u64,
    address_length: u64,
) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    if file.inode.as_unix().is_none() {
        return ENOTSOCK;
    }
    if (file.is_nonblock() || flags & MSG_DONTWAIT != 0) && !file.inode.ready(false) {
        return EAGAIN;
    }
    let mut buffer = alloc::vec![0u8; length];
    let (count, _) = match file.inode.recv_message(&mut buffer, 0) {
        Ok(result) => result,
        Err(failure) => return error(failure),
    };
    if let Err(code) = copy_to_user(pointer, &buffer[..count]) {
        return code;
    }
    if address != 0 && address_length != 0 {
        write_address(address, address_length, None);
    }
    count as i64
}

// --- dizin ve isim işlemleri ---------------------------------------------

pub fn mkdir(path_pointer: u64, mode: u32) -> i64 {
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };
    let (parent, name) = match fs::parent_and_name(&path) {
        Ok(result) => result,
        Err(failure) => return error(failure),
    };
    match parent.create(&name, FileType::Directory, mode & 0o7777) {
        Ok(_) => 0,
        Err(failure) => error(failure),
    }
}

pub fn rmdir(path_pointer: u64) -> i64 {
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };
    let inode = match fs::lookup_path(&path) {
        Ok(inode) => inode,
        Err(failure) => return error(failure),
    };
    if !inode.metadata().is_directory() {
        return -20; // ENOTDIR
    }
    match inode.readdir() {
        Ok(entries) if !entries.is_empty() => return -39, // ENOTEMPTY
        _ => {}
    }
    let (parent, name) = match fs::parent_and_name(&path) {
        Ok(result) => result,
        Err(failure) => return error(failure),
    };
    match parent.unlink(&name) {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

pub fn unlink(path_pointer: u64) -> i64 {
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };
    let (parent, name) = match fs::parent_and_name(&path) {
        Ok(result) => result,
        Err(failure) => return error(failure),
    };
    let inode = match parent.lookup(&name) {
        Ok(inode) => inode,
        Err(failure) => return error(failure),
    };
    if inode.metadata().is_directory() {
        return -21; // EISDIR
    }
    match parent.unlink(&name) {
        Ok(()) => {
            crate::fs::unix::forget_path(&fs::absolute_path(&path));
            0
        }
        Err(failure) => error(failure),
    }
}

pub fn unlinkat(directory: i32, path_pointer: u64, flags: u32) -> i64 {
    let _ = directory;
    if flags & 0x200 != 0 {
        // AT_REMOVEDIR
        rmdir(path_pointer)
    } else {
        unlink(path_pointer)
    }
}

pub fn chmod(path_pointer: u64, _mode: u32) -> i64 {
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };
    match fs::lookup_path(&path) {
        Ok(_) => 0,
        Err(failure) => error(failure),
    }
}

pub fn fchmod(descriptor: i32, _mode: u32) -> i64 {
    match socket_file(descriptor) {
        Ok(_) => 0,
        Err(code) => code,
    }
}

pub fn fchmodat(directory: i32, path_pointer: u64, mode: u32, flags: u32) -> i64 {
    let _ = (directory, flags);
    chmod(path_pointer, mode)
}

pub fn utimensat(_directory: i32, path_pointer: u64, _times: u64, _flags: u32) -> i64 {
    if path_pointer == 0 {
        return 0;
    }
    let path = match read_user_string(path_pointer) {
        Ok(path) => path,
        Err(code) => return code,
    };
    match fs::lookup_path(&path) {
        Ok(_) => 0,
        Err(failure) => error(failure),
    }
}

pub fn fsync(descriptor: i32, _data_only: bool) -> i64 {
    let file = match socket_file(descriptor) {
        Ok(file) => file,
        Err(code) => return code,
    };
    match file.inode.sync() {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}
