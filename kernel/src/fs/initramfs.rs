//! initramfs: `newc` (cpio) arşivini ramfs'e açar.
//!
//! Arch kurulumu için ileride `mkinitcpio` çıktısı aynı biçimi kullanacak.
//! Arşiv biçimi bilinçli olarak basit: 6 bayt sihir, 13 adet 8 haneli onaltılık
//! alan, ad, 4 bayt hizalama, veri.

use alloc::vec::Vec;

use super::{FileType, VfsError};

const MAGIC: &[u8; 6] = b"070701";
const HEADER_SIZE: usize = 110;

struct Header {
    mode: u32,
    file_size: usize,
    name_size: usize,
}

fn hex(field: &[u8]) -> Option<u32> {
    let mut value = 0u32;
    for byte in field {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        value = value.checked_mul(16)?.checked_add(digit as u32)?;
    }
    Some(value)
}

fn parse_header(archive: &[u8], offset: usize) -> Option<Header> {
    let header = archive.get(offset..offset + HEADER_SIZE)?;
    if &header[0..6] != MAGIC {
        return None;
    }

    Some(Header {
        mode: hex(&header[14..22])?,
        file_size: hex(&header[54..62])? as usize,
        name_size: hex(&header[94..102])? as usize,
    })
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

/// Arşivi kök dizine açar. Dönen değer yazılan dosya sayısı.
pub fn extract(archive: &[u8]) -> usize {
    let mut offset = 0usize;
    let mut files = 0usize;

    while offset + HEADER_SIZE <= archive.len() {
        let Some(header) = parse_header(archive, offset) else {
            break;
        };

        let name_start = offset + HEADER_SIZE;
        let name_end = name_start + header.name_size;
        let Some(name_bytes) = archive.get(name_start..name_end) else {
            break;
        };

        let name = core::str::from_utf8(&name_bytes[..header.name_size.saturating_sub(1)])
            .unwrap_or("");
        let data_start = align4(name_end);
        let data_end = data_start.saturating_add(header.file_size);
        let Some(data) = archive.get(data_start..data_end) else {
            break;
        };

        if name == "TRAILER!!!" {
            break;
        }

        if !name.is_empty() {
            create_entry(name, header.mode, data, &mut files);
        }

        offset = align4(data_end);
    }

    files
}

/// Limine modülleri arasında `initramfs.cpio` arar ve açarsa sayısını döner.
pub fn extract_from_modules() -> usize {
    for module in crate::boot::limine::modules() {
        if module.is_null() {
            continue;
        }
        let path = crate::boot::limine::module_path(*module);
        if !path.ends_with("initramfs.cpio") {
            continue;
        }
        let file = unsafe { &**module };
        if file.address.is_null() || file.size == 0 {
            continue;
        }
        let archive =
            unsafe { core::slice::from_raw_parts(file.address as *const u8, file.size as usize) };
        let files = extract(archive);
        println!("initramfs: {} dosya acildi", files);
        return files;
    }
    0
}

fn create_entry(path: &str, mode: u32, data: &[u8], files: &mut usize) {
    let is_directory = mode & 0o170000 == 0o040000;
    let is_regular = mode & 0o170000 == 0o100000;

    if !is_directory && !is_regular {
        // Aygıt, FIFO ve symlink'ler M4'te; initramfs'te nadiren gerekli.
        return;
    }

    let components: Vec<&str> = path.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    if components.is_empty() {
        return;
    }

    let Some(filesystem) = super::root() else {
        return;
    };
    let mut directory = filesystem.root();

    for component in &components[..components.len() - 1] {
        directory = match directory.lookup(component) {
            Ok(next) if next.metadata().is_directory() => next,
            Ok(_) => return,
            Err(VfsError::NotFound) => match directory.create(component, FileType::Directory, 0o755) {
                Ok(next) => next,
                Err(_) => return,
            },
            Err(_) => return,
        };
    }

    let leaf = components[components.len() - 1];

    if is_directory {
        match directory.lookup(leaf) {
            Ok(_) => {}
            Err(_) => {
                let _ = directory.create(leaf, FileType::Directory, 0o755);
            }
        }
        return;
    }

    let inode = match directory.lookup(leaf) {
        Ok(inode) => inode,
        Err(_) => match directory.create(leaf, FileType::Regular, mode & 0o7777) {
            Ok(inode) => inode,
            Err(_) => return,
        },
    };

    if inode.write_at(0, data).is_ok() {
        let _ = inode.truncate(data.len() as u64);
        *files += 1;
    }
}
