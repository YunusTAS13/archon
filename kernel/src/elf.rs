//! ELF64 yükleyici.
//!
//! `ET_EXEC` (sabit adresli) ve `ET_DYN` (PIE / paylaşımlı nesne) ikilileri
//! yükler. `PT_INTERP` varsa dinamik bağlayıcı (`ld.so`) ayrı bir tabana
//! yerleştirilir ve denetime o verilir; `AT_BASE` auxv girdisi ile bildirilir.

use alloc::vec::Vec;

use crate::mem::{hhdm, pmm, vmm};

pub const STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
const STACK_PAGES: usize = 2048;
const PAGE_SIZE: u64 = 4096;

/// PIE ana programın yüklendiği taban.
pub const EXEC_BASE: u64 = 0x0000_5555_0000_0000;
/// `ld.so` için ayrı taban; mmap bölgesinin (0x6000...) üstünde.
pub const INTERP_BASE: u64 = 0x0000_7F00_0000_0000;

const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;

const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE: u64 = 7;
const AT_ENTRY: u64 = 9;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_PLATFORM: u64 = 15;
const AT_CLKTCK: u64 = 17;
const AT_SECURE: u64 = 23;
const AT_RANDOM: u64 = 25;
const AT_EXECFN: u64 = 31;

#[derive(Debug, Clone, Copy)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    NotX86_64,
    UnsupportedType,
    NeedsInterpreter,
    OutOfMemory,
    BadProgramHeader,
}

#[repr(C)]
struct ElfHeader {
    ident: [u8; 16],
    kind: u16,
    machine: u16,
    version: u32,
    entry: u64,
    program_header_offset: u64,
    section_header_offset: u64,
    flags: u32,
    header_size: u16,
    program_header_size: u16,
    program_header_count: u16,
    section_header_size: u16,
    section_header_count: u16,
    section_name_index: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ProgramHeader {
    kind: u32,
    flags: u32,
    offset: u64,
    virtual_address: u64,
    physical_address: u64,
    file_size: u64,
    memory_size: u64,
    alignment: u64,
}

pub struct LoadedImage {
    pub entry: u64,
    pub stack_pointer: u64,
    pub brk_start: u64,
    pub program_headers: u64,
    pub interpreter_base: u64,
}

struct MappedImage {
    entry: u64,
    phdr: u64,
    highest: u64,
}

fn read_header(image: &[u8]) -> Result<ElfHeader, ElfError> {
    if image.len() < core::mem::size_of::<ElfHeader>() {
        return Err(ElfError::TooSmall);
    }
    let header: ElfHeader =
        unsafe { core::ptr::read_unaligned(image.as_ptr() as *const ElfHeader) };
    if &header.ident[0..4] != b"\x7FELF" {
        return Err(ElfError::BadMagic);
    }
    if header.machine != EM_X86_64 {
        return Err(ElfError::NotX86_64);
    }
    if header.kind != ET_EXEC && header.kind != ET_DYN {
        return Err(ElfError::UnsupportedType);
    }
    let ph_size = core::mem::size_of::<ProgramHeader>();
    if header.program_header_size as usize != ph_size {
        return Err(ElfError::BadProgramHeader);
    }
    Ok(header)
}

fn program_headers<'a>(image: &'a [u8], header: &ElfHeader) -> Result<&'a [ProgramHeader], ElfError> {
    let offset = header.program_header_offset as usize;
    let count = header.program_header_count as usize;
    if offset + count * core::mem::size_of::<ProgramHeader>() > image.len() {
        return Err(ElfError::BadProgramHeader);
    }
    Ok(unsafe {
        core::slice::from_raw_parts(
            image.as_ptr().add(offset) as *const ProgramHeader,
            count,
        )
    })
}

/// `PT_INTERP` yolunu (C dizesi, sonlandırıcı olmadan) döndürür.
pub fn interpreter_path(image: &[u8]) -> Option<Vec<u8>> {
    let header = read_header(image).ok()?;
    let headers = program_headers(image, &header).ok()?;
    let interp = headers.iter().find(|entry| entry.kind == PT_INTERP)?;
    let start = interp.offset as usize;
    let end = start + interp.file_size as usize;
    if end > image.len() {
        return None;
    }
    let mut bytes = image[start..end].to_vec();
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    Some(bytes)
}

fn map_image(pml4: u64, image: &[u8], base: u64) -> Result<MappedImage, ElfError> {
    let header = read_header(image)?;
    let headers = program_headers(image, &header)?;

    // AT_PHDR: program başlıklarının bellekteki sanal adresi.
    // PT_PHDR varsa doğrudan; yoksa phdr'leri içeren PT_LOAD'un tabanından
    // hesaplanır (ET_EXEC'te vaddr 0 olmadığı için şart).
    let mut phdr = 0u64;
    for entry in headers {
        if entry.kind == PT_PHDR {
            phdr = base + entry.virtual_address;
        }
    }
    if phdr == 0 {
        let phoff = header.program_header_offset;
        for entry in headers {
            if entry.kind == PT_LOAD
                && phoff >= entry.offset
                && phoff < entry.offset + entry.file_size
            {
                phdr = base + entry.virtual_address + (phoff - entry.offset);
                break;
            }
        }
    }

    let mut highest = 0u64;
    for entry in headers {
        if entry.kind != PT_LOAD {
            continue;
        }

        let start = (base + entry.virtual_address) & !(PAGE_SIZE - 1);
        let end = (base + entry.virtual_address + entry.memory_size + PAGE_SIZE - 1)
            & !(PAGE_SIZE - 1);
        highest = highest.max(end);

        // Kesimler sayfa hizalı olmayabilir (vaddr ve offset farklı sayfa
        // fazlalıkları taşıyabilir); kopyalanacak aralığı sayfa bazında kes.
        let segment_start = base + entry.virtual_address;
        let segment_file_end = segment_start + entry.file_size;

        let mut address = start;
        while address < end {
            let frame = pmm::alloc_frame().ok_or(ElfError::OutOfMemory)?;
            let destination = hhdm::phys_to_ptr::<u8>(frame);
            // BSS ve dosya sonrası dolgu sıfır olmalı; PMM eski içerik döndürür.
            unsafe { core::ptr::write_bytes(destination, 0, PAGE_SIZE as usize) };

            let copy_start = address.max(segment_start);
            let copy_end = (address + PAGE_SIZE).min(segment_file_end);
            if copy_start < copy_end {
                let count = (copy_end - copy_start) as usize;
                let source = (entry.offset + (copy_start - segment_start)) as usize;
                let destination_offset = (copy_start - address) as usize;
                if source + count > image.len() {
                    return Err(ElfError::BadProgramHeader);
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        image.as_ptr().add(source),
                        destination.add(destination_offset),
                        count,
                    );
                }
            }

            let mut flags = vmm::PRESENT | vmm::USER;
            if entry.flags & PF_W != 0 {
                flags |= vmm::WRITABLE;
            }
            if entry.flags & PF_X == 0 {
                flags |= vmm::NO_EXECUTE;
            }

            vmm::map_in(pml4, address, frame, flags).map_err(|_| ElfError::OutOfMemory)?;
            address += PAGE_SIZE;
        }
    }

    Ok(MappedImage {
        entry: base + header.entry,
        phdr,
        highest,
    })
}

/// Ana programı ve varsa yorumlayıcıyı yükler, yığını kurar.
///
/// `interpreter` boş değilse `image` içindeki `PT_INTERP` yolundan okunmuş
/// `ld.so` imajıdır (`ET_DYN` olmalıdır).
pub fn load(
    pml4: u64,
    image: &[u8],
    argv: &[&str],
    envp: &[&str],
    interpreter: Option<&[u8]>,
) -> Result<LoadedImage, ElfError> {
    let header = read_header(image)?;
    let base = if header.kind == ET_DYN { EXEC_BASE } else { 0 };
    let main = map_image(pml4, image, base)?;

    let mut entry = main.entry;
    let mut interpreter_base = 0u64;
    if let Some(interpreter_image) = interpreter {
        let interpreter_header = read_header(interpreter_image)?;
        let mapped = map_image(pml4, interpreter_image, INTERP_BASE)?;
        let _ = interpreter_header;
        entry = mapped.entry;
        interpreter_base = INTERP_BASE;
    }

    // Yığın.
    let stack_size = (STACK_PAGES as u64) * PAGE_SIZE;
    let stack_base = STACK_TOP - stack_size;
    let mut stack_pages = Vec::with_capacity(STACK_PAGES);
    for index in 0..STACK_PAGES {
        let frame = pmm::alloc_frame().ok_or(ElfError::OutOfMemory)?;
        unsafe { hhdm::zero_physical(frame, PAGE_SIZE as usize) };
        vmm::map_in(
            pml4,
            stack_base + index as u64 * PAGE_SIZE,
            frame,
            vmm::PRESENT | vmm::USER | vmm::WRITABLE | vmm::NO_EXECUTE,
        )
        .map_err(|_| ElfError::OutOfMemory)?;
        stack_pages.push(frame);
    }

    let execfn = argv.first().copied().unwrap_or("");
    let stack_pointer = build_stack(
        &stack_pages,
        argv,
        envp,
        execfn,
        main.entry,
        main.phdr,
        header.program_header_count,
        interpreter_base,
    );

    Ok(LoadedImage {
        entry,
        stack_pointer,
        brk_start: (main.highest + PAGE_SIZE - 1) & !(PAGE_SIZE - 1),
        program_headers: main.phdr,
        interpreter_base,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_stack(
    pages: &[u64],
    argv: &[&str],
    envp: &[&str],
    execfn: &str,
    entry: u64,
    phdr: u64,
    phnum: u16,
    interpreter_base: u64,
) -> u64 {
    const PLATFORM: &str = "x86_64";

    let total = pages.len() * PAGE_SIZE as usize;
    let mut buffer = alloc::vec![0u8; total];
    let mut cursor = total;

    // 1) Dizeler en tepede.
    let mut place_string = |bytes: &[u8], buffer: &mut [u8], cursor: &mut usize| -> u64 {
        *cursor -= bytes.len() + 1;
        buffer[*cursor..*cursor + bytes.len()].copy_from_slice(bytes);
        buffer[*cursor + bytes.len()] = 0;
        (STACK_TOP - total as u64) + *cursor as u64
    };

    let execfn_address = place_string(execfn.as_bytes(), &mut buffer, &mut cursor);
    let platform_address = place_string(PLATFORM.as_bytes(), &mut buffer, &mut cursor);

    let mut argument_pointers = Vec::with_capacity(argv.len());
    for argument in argv.iter().rev() {
        let bytes = argument.as_bytes();
        cursor -= bytes.len() + 1;
        buffer[cursor..cursor + bytes.len()].copy_from_slice(bytes);
        buffer[cursor + bytes.len()] = 0;
        argument_pointers.push((STACK_TOP - total as u64) + cursor as u64);
    }
    argument_pointers.reverse();

    let mut environment_pointers = Vec::with_capacity(envp.len());
    for variable in envp.iter().rev() {
        let bytes = variable.as_bytes();
        cursor -= bytes.len() + 1;
        buffer[cursor..cursor + bytes.len()].copy_from_slice(bytes);
        buffer[cursor + bytes.len()] = 0;
        environment_pointers.push((STACK_TOP - total as u64) + cursor as u64);
    }
    environment_pointers.reverse();

    // AT_RANDOM için 16 bayt.
    cursor -= 16;
    let random_offset = cursor;
    let seed = crate::time::monotonic_nanos() ^ crate::arch::cpu::rdtsc();
    for index in 0..16 {
        buffer[random_offset + index] =
            ((seed >> (index % 8 * 8)) & 0xFF) as u8 ^ (index as u8 * 37);
    }
    let random_address = (STACK_TOP - total as u64) + random_offset as u64;

    // 2) Kelime dizisi: argc, argv[], NULL, envp NULL, auxv.
    let mut words: Vec<u64> = Vec::new();
    words.push(argv.len() as u64);
    for pointer in &argument_pointers {
        words.push(*pointer);
    }
    words.push(0); // argv sonu
    for pointer in &environment_pointers {
        words.push(*pointer);
    }
    words.push(0); // envp sonu

    let auxv: [(u64, u64); 16] = [
        (AT_PHDR, phdr),
        (AT_PHENT, core::mem::size_of::<ProgramHeader>() as u64),
        (AT_PHNUM, phnum as u64),
        (AT_PAGESZ, PAGE_SIZE),
        (AT_BASE, interpreter_base),
        (AT_ENTRY, entry),
        (AT_UID, 0),
        (AT_EUID, 0),
        (AT_GID, 0),
        (AT_EGID, 0),
        (AT_PLATFORM, platform_address),
        (AT_CLKTCK, 100),
        (AT_SECURE, 0),
        (AT_RANDOM, random_address),
        (AT_EXECFN, execfn_address),
        (AT_NULL, 0),
    ];
    for (key, value) in auxv {
        words.push(key);
        words.push(value);
    }
    words.push(AT_NULL);
    words.push(0);

    let bytes = words.len() * 8;
    cursor = (cursor - bytes) & !15;
    for (index, word) in words.iter().enumerate() {
        let offset = cursor + index * 8;
        buffer[offset..offset + 8].copy_from_slice(&word.to_le_bytes());
    }

    // 3) Yerel tamponu fiziksel sayfalara kopyala.
    for (index, frame) in pages.iter().enumerate() {
        let destination = hhdm::phys_to_ptr::<u8>(*frame);
        let source = &buffer[index * PAGE_SIZE as usize..(index + 1) * PAGE_SIZE as usize];
        unsafe {
            core::ptr::copy_nonoverlapping(source.as_ptr(), destination, PAGE_SIZE as usize);
        }
    }

    (STACK_TOP - total as u64) + cursor as u64
}
