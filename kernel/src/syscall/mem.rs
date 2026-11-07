//! Bellek syscall'ları: `brk`, `mmap`, `munmap`, `mprotect`.
//!
//! `mmap` yalnızca anonim ve özel eşlemeyi destekler. Dosya eşlemesi (`MAP_SHARED`
//! page cache) M4'te ext4 ile birlikte gelecek; Plasma `wl_shm` için anonim
//! `memfd` kullanır, o da bu yolu izler.

use core::sync::atomic::Ordering;

use super::{copy_to_user, EINVAL, ENOMEM, ENOSYS};
use crate::mem::{hhdm, pmm, vmm};

pub const PROT_NONE: u64 = 0;
pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;

pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_SHARED: u64 = 0x01;
pub const MAP_ANONYMOUS: u64 = 0x20;
pub const MAP_FIXED: u64 = 0x10;

const PAGE_SIZE: u64 = 4096;
/// Kullanıcı `mmap` bölgesi: yığının (0x7fff_ffff_0000) altında, PIE
/// ikililerin (0x5555_5555_0000) üstünde.
const MMAP_BASE: u64 = 0x0000_6000_0000_0000;

static MMAP_NEXT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(MMAP_BASE);

fn prot_to_flags(protections: u64) -> u64 {
    let mut flags = vmm::PRESENT | vmm::USER;
    if protections & PROT_WRITE != 0 {
        flags |= vmm::WRITABLE;
    }
    if protections & PROT_EXEC == 0 {
        flags |= vmm::NO_EXECUTE;
    }
    flags
}

fn page_count(size: usize) -> Option<u64> {
    let size = size as u64;
    Some((size + PAGE_SIZE - 1) / PAGE_SIZE)
}

pub fn mmap(
    address: u64,
    length: usize,
    protections: u64,
    flags: u64,
    descriptor: i32,
    offset: i64,
) -> i64 {
    if length == 0 {
        return EINVAL;
    }

    // Dosya eşlemesi: MAP_PRIVATE, sayfa önbelleği olmadan dosyadan kopyalanır.
    let file_inode = if flags & MAP_ANONYMOUS == 0 {
        let Some(process) = crate::task::current() else {
            return ENOMEM;
        };
        match process.fd_table.get(descriptor) {
            Ok(file) => Some(file.inode.clone()),
            Err(_) => return super::EBADF,
        }
    } else {
        None
    };

    let pages = match page_count(length) {
        Some(pages) if pages > 0 => pages,
        _ => return EINVAL,
    };

    let base = if address != 0 && flags & MAP_FIXED != 0 {
        address & !(PAGE_SIZE - 1)
    } else {
        MMAP_NEXT.fetch_add(pages * PAGE_SIZE, Ordering::Relaxed) & !(PAGE_SIZE - 1)
    };

    let Some(process) = crate::task::current() else {
        return ENOMEM;
    };

    let mapping_flags = prot_to_flags(protections);

    // Aygıt belleği (ör. /dev/fb0): sayfa ayırma yok, fiziksel aralık doğrudan
    // kullanıcı adres alanına eşlenir.
    if let Some((physical_base, physical_size)) = file_inode
        .as_ref()
        .and_then(|inode| inode.physical_region())
    {
        let offset = offset as u64;
        for index in 0..pages {
            let physical = physical_base + offset + index * PAGE_SIZE;
            if physical + PAGE_SIZE > physical_base + physical_size {
                return EINVAL;
            }
            if vmm::map_in(
                process.pml4.load(Ordering::Relaxed),
                base + index * PAGE_SIZE,
                physical,
                mapping_flags,
            )
            .is_err()
            {
                return ENOMEM;
            }
        }
        // Uygulama ekranı devraldı: çekirdek metin konsolu artık çizmesin.
        crate::fbtext::freeze();
        if let Some(inode) = &file_inode {
            inode.mmap_notify(true);
        }
        return base as i64;
    }
    // MAP_FIXED: Linux eski eşlemeleri sessizce kaldırır (ld.so libc'yi
    // önceden ayırdığı aralığa bu şekilde yerleştirir).
    if flags & MAP_FIXED != 0 {
        for index in 0..pages {
            let address = base + index * PAGE_SIZE;
            if let Some(physical) = vmm::unmap_in(process.pml4.load(Ordering::Relaxed), address) {
                pmm::free_frame(physical);
            }
        }
    }
    for index in 0..pages {
        let file_offset = offset as u64 + index * PAGE_SIZE;

        // `MAP_SHARED` + `memfd`: sayfa önbelleği paylaşılır, kopya alınmaz.
        if flags & MAP_SHARED != 0 {
            if let Some(inode) = &file_inode {
                if let Some(frame) = inode.shared_frame(file_offset / PAGE_SIZE) {
                    if vmm::map_in(
                        process.pml4.load(Ordering::Relaxed),
                        base + index * PAGE_SIZE,
                        frame,
                        mapping_flags,
                    )
                    .is_err()
                    {
                        return ENOMEM;
                    }
                    continue;
                }
            }
        }

        let Some(frame) = pmm::alloc_frame() else {
            return ENOMEM;
        };
        // Anonim eşlemeler sıfır dolu olmalı; PMM eski içerik döndürür.
        unsafe { hhdm::zero_physical(frame, PAGE_SIZE as usize) };
        if let Some(inode) = &file_inode {
            let size = inode.metadata().size;
            if file_offset < size {
                let count = (size - file_offset).min(PAGE_SIZE) as usize;
                let destination = hhdm::phys_to_ptr::<u8>(frame);
                let slice = unsafe { core::slice::from_raw_parts_mut(destination, count) };
                let _ = inode.read_at(file_offset, slice);
            }
        }
        match vmm::map_in(process.pml4.load(Ordering::Relaxed), base + index * PAGE_SIZE, frame, mapping_flags) {
            Ok(()) => {}
            Err(_) => return ENOMEM,
        }
    }

    base as i64
}

pub fn munmap(address: u64, length: usize) -> i64 {
    if address % PAGE_SIZE != 0 {
        return EINVAL;
    }
    let Some(process) = crate::task::current() else {
        return EINVAL;
    };
    let pages = (length as u64 + PAGE_SIZE - 1) / PAGE_SIZE;

    for index in 0..pages {
        if let Some(physical) = vmm::unmap_in(process.pml4.load(Ordering::Relaxed), address + index * PAGE_SIZE) {
            // `memfd` çerçeveleri inode'a aittir; eşleme kalkınca silinmez.
            if !crate::fs::anon::is_shared_frame(physical) {
                pmm::free_frame(physical);
            }
        }
    }
    0
}

pub fn mprotect(address: u64, length: usize, protections: u64) -> i64 {
    if address % PAGE_SIZE != 0 {
        return EINVAL;
    }
    let Some(process) = crate::task::current() else {
        return EINVAL;
    };

    let pages = (length as u64 + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut flags = vmm::PRESENT | vmm::USER;
    if protections & PROT_WRITE != 0 {
        flags |= vmm::WRITABLE;
    }
    if protections & PROT_EXEC == 0 {
        flags |= vmm::NO_EXECUTE;
    }

    for index in 0..pages {
        let virtual_address = address + index * PAGE_SIZE;
        let Some(physical) = vmm::translate(process.pml4.load(Ordering::Relaxed), virtual_address) else {
            return ENOMEM;
        };
        // Eski eşlemeyi kaldırıp yeni bayraklarla geri koy.
        vmm::unmap_in(process.pml4.load(Ordering::Relaxed), virtual_address);
        if vmm::map_in(process.pml4.load(Ordering::Relaxed), virtual_address, physical & !(PAGE_SIZE - 1), flags).is_err() {
            return ENOMEM;
        }
    }
    0
}

pub fn brk(requested: u64) -> i64 {
    let Some(process) = crate::task::current() else {
        return ENOMEM;
    };
    let current = process.brk.load(Ordering::Relaxed);

    if requested == 0 {
        return current as i64;
    }

    let requested = (requested + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    if requested < current {
        // Küçültme: sayfaları geri ver.
        let mut address = requested;
        while address < current {
            if let Some(physical) = vmm::unmap_in(process.pml4.load(Ordering::Relaxed), address) {
                pmm::free_frame(physical);
            }
            address += PAGE_SIZE;
        }
        process.brk.store(requested, Ordering::Relaxed);
        return requested as i64;
    }

    let mut address = current;
    while address < requested {
        let Some(frame) = pmm::alloc_frame() else {
            return ENOMEM;
        };
        // Heap sayfaları sıfır olmalı: malloc/atexit listeleri buna güvenir.
        unsafe { hhdm::zero_physical(frame, PAGE_SIZE as usize) };
        let flags = vmm::PRESENT | vmm::USER | vmm::WRITABLE | vmm::NO_EXECUTE;
        if vmm::map_in(process.pml4.load(Ordering::Relaxed), address, frame, flags).is_err() {
            return ENOMEM;
        }
        address += PAGE_SIZE;
    }

    process.brk.store(requested, Ordering::Relaxed);
    requested as i64
}

/// `write(fd, ptr, n)` gibi kullanımlar için yardımcı: kullanıcı adresine
/// sıfır yazar.
pub fn zero_user(address: u64, length: usize) -> i64 {
    let zeros = alloc::vec![0u8; length];
    match copy_to_user(address, &zeros) {
        Ok(()) => 0,
        Err(code) => code,
    }
}
