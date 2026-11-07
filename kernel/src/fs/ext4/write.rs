//! ext4 yazma yolu: blok/inode tahsisi, extent güncelleme, dizin girişi.
//!
//! Tasarım gereği **tek blok grubu değil**, tüm gruplar taranır; her tahsis
//! öncesi grup tanımlayıcısı ve süper blok diskten okunup güncellenir. Böylece
//! ayrı bir önbellek tutarlılığı sorunu doğmaz; her işlem diske yazılır.
//!
//! Sınırlar (bilinçli):
//! - extent ağacı yalnızca kök düğüm (en fazla 4 extent); daha fazlası `ENOSPC`
//! - JBD2 yok; imaj `has_journal` olmadan üretilir
//! - `metadata_csum` yok; imaj checksum'sız özelliklerle biçimlendirilir

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::*;

/// Dizin girişi dosya türü kodları (ext4 `file_type`).
pub const DIRENT_REGULAR: u8 = 1;
pub const DIRENT_DIRECTORY: u8 = 2;
pub const DIRENT_SYMLINK: u8 = 7;

const EXTENT_HEADER_SIZE: usize = 12;
const EXTENT_ENTRY_SIZE: usize = 12;
const EXTENT_ROOT_MAX: usize = 4;

fn now_seconds() -> u32 {
    (crate::time::monotonic_nanos() / 1_000_000_000) as u32
}

fn file_type_code(file_type: FileType) -> u8 {
    match file_type {
        FileType::Directory => DIRENT_DIRECTORY,
        FileType::Symlink => DIRENT_SYMLINK,
        _ => DIRENT_REGULAR,
    }
}

fn mode_bits(file_type: FileType) -> u16 {
    match file_type {
        FileType::Directory => 0x4000,
        FileType::Regular => 0x8000,
        FileType::Symlink => 0xA000,
        FileType::CharDevice => 0x2000,
        FileType::BlockDevice => 0x6000,
        FileType::Fifo => 0x1000,
        FileType::Socket => 0xC000,
    }
}

/// Süper bloğu diskten okur (bayt 1024, 1024 bayt).
pub fn read_superblock<D: BlockDevice>(device: &Arc<D>) -> Result<Superblock> {
    let mut buffer = alloc::vec![0u8; 1024];
    device.read_block(2, &mut buffer)?;
    Ok(unsafe { core::ptr::read_unaligned(buffer.as_ptr() as *const Superblock) })
}

pub fn write_superblock<D: BlockDevice>(device: &Arc<D>, superblock: &Superblock) -> Result<()> {
    let bytes = unsafe {
        core::slice::from_raw_parts(superblock as *const Superblock as *const u8, 1024)
    };
    device.write_block(2, bytes)
}

impl<D: BlockDevice + 'static> Ext4InodeHandle<D> {
    pub(super) fn write_block_number(&self, block: u64, buffer: &[u8]) -> Result<()> {
        let sectors_per_block = self.block_size() as u64 / 512;
        self.device.write_block(block * sectors_per_block, buffer)
    }

    fn group_location(&self, index: usize) -> (u64, usize) {
        let block_size = self.block_size();
        let table_block = if block_size == 1024 { 2 } else { 1 };
        let entry_size = self.superblock.descriptor_entry_size() as usize;
        let offset = index * entry_size;
        (
            table_block as u64 + (offset / block_size) as u64,
            offset % block_size,
        )
    }

    pub(super) fn read_group(&self, index: usize) -> Result<GroupDescriptor> {
        let (block, offset) = self.group_location(index);
        let buffer = self.read_block(block)?;
        let entry_size = self.superblock.descriptor_entry_size() as usize;
        if offset + entry_size > buffer.len() {
            return Err(VfsError::Io);
        }
        Ok(parse_group_descriptor(&buffer[offset..], entry_size))
    }

    fn write_group(&self, index: usize, descriptor: &GroupDescriptor) -> Result<()> {
        let (block, offset) = self.group_location(index);
        let mut buffer = self.read_block(block)?;
        let entry_size = self.superblock.descriptor_entry_size() as usize;
        let bytes = unsafe {
            core::slice::from_raw_parts(
                descriptor as *const GroupDescriptor as *const u8,
                core::mem::size_of::<GroupDescriptor>(),
            )
        };
        let count = entry_size.min(bytes.len());
        buffer[offset..offset + count].copy_from_slice(&bytes[..count]);
        self.write_block_number(block, &buffer)
    }

    /// Inode tablosundaki kaydı diske yazar.
    pub(super) fn write_inode_entry(
        &self,
        number: InodeNumber,
        inode: &Ext4Inode,
    ) -> Result<()> {
        if number == 0 {
            return Err(VfsError::InvalidArgument);
        }
        let block_size = self.block_size() as u64;
        let group = ((number - 1) / self.superblock.inodes_per_group as u64) as usize;
        let index = (number - 1) % self.superblock.inodes_per_group as u64;
        let descriptor = self.read_group(group)?;
        let inode_size = self.superblock.inode_size() as u64;
        let offset = index * inode_size;
        let block = descriptor.inode_table() + offset / block_size;
        let within = (offset % block_size) as usize;
        let mut buffer = self.read_block(block)?;
        let size = core::mem::size_of::<Ext4Inode>();
        let bytes =
            unsafe { core::slice::from_raw_parts(inode as *const Ext4Inode as *const u8, size) };
        buffer[within..within + size].copy_from_slice(bytes);
        self.write_block_number(block, &buffer)
    }

    /// Boş bir inode ayırır; içerik ve bağ sayısı çağıranın sorumluluğunda.
    pub(super) fn allocate_inode(&self, mode: u16) -> Result<(InodeNumber, Ext4Inode)> {
        let superblock = read_superblock(&self.device)?;
        for group in 0..superblock.group_count() {
            let mut descriptor = self.read_group(group as usize)?;
            if descriptor.free_inodes_count_lo == 0 {
                continue;
            }
            let bitmap_block = descriptor.inode_bitmap();
            let mut bitmap = self.read_block(bitmap_block)?;
            for bit in 0..superblock.inodes_per_group {
                let index = bit as usize;
                if bitmap[index / 8] & (1 << (index % 8)) != 0 {
                    continue;
                }
                bitmap[index / 8] |= 1 << (index % 8);
                self.write_block_number(bitmap_block, &bitmap)?;
                descriptor.free_inodes_count_lo -= 1;
                if mode & 0xF000 == 0x4000 {
                    descriptor.used_dirs_count_lo += 1;
                }
                self.write_group(group as usize, &descriptor)?;

                let mut updated = read_superblock(&self.device)?;
                updated.free_inodes_count -= 1;
                write_superblock(&self.device, &updated)?;

                let number = group as u64 * superblock.inodes_per_group as u64 + index as u64 + 1;
                let inode = Self::empty_inode(mode);
                return Ok((number, inode));
            }
        }
        Err(VfsError::NoSpace)
    }

    fn empty_inode(mode: u16) -> Ext4Inode {
        let mut inode: Ext4Inode = unsafe { core::mem::zeroed() };
        inode.mode = mode;
        inode.links_count = 1;
        inode.extra_isize = 32;
        inode.flags = EXT4_EXTENTS_FL;
        inode.generation = crate::arch::cpu::rdtsc() as u32;
        let now = now_seconds();
        inode.access_time = now;
        inode.modify_time = now;
        inode.change_time = now;
        // Boş extent kökü: sihir, 0 giriş, 4 yuva, derinlik 0.
        inode.block[0..2].copy_from_slice(&EXTENT_MAGIC.to_le_bytes());
        inode.block[2..4].copy_from_slice(&0u16.to_le_bytes());
        inode.block[4..6].copy_from_slice(&(EXTENT_ROOT_MAX as u16).to_le_bytes());
        inode.block[6..8].copy_from_slice(&0u16.to_le_bytes());
        inode
    }

    pub(super) fn free_inode(&self, number: InodeNumber) -> Result<()> {
        let superblock = read_superblock(&self.device)?;
        let group = ((number - 1) / superblock.inodes_per_group as u64) as usize;
        let index = ((number - 1) % superblock.inodes_per_group as u64) as usize;
        let mut descriptor = self.read_group(group)?;
        let bitmap_block = descriptor.inode_bitmap();
        let mut bitmap = self.read_block(bitmap_block)?;
        if bitmap[index / 8] & (1 << (index % 8)) == 0 {
            return Err(VfsError::NotFound);
        }
        bitmap[index / 8] &= !(1 << (index % 8));
        self.write_block_number(bitmap_block, &bitmap)?;
        descriptor.free_inodes_count_lo += 1;
        let freed = self.read_inode(number)?;
        if freed.file_type() == FileType::Directory && descriptor.used_dirs_count_lo > 0 {
            descriptor.used_dirs_count_lo -= 1;
        }
        self.write_group(group, &descriptor)?;

        let mut updated = read_superblock(&self.device)?;
        updated.free_inodes_count += 1;
        write_superblock(&self.device, &updated)
    }

    /// Boş bir blok ayırır ve içeriğini sıfırlar.
    pub(super) fn allocate_block(&self) -> Result<u64> {
        let superblock = read_superblock(&self.device)?;
        let total_blocks = superblock.blocks_count();
        for group in 0..superblock.group_count() {
            let mut descriptor = self.read_group(group as usize)?;
            if descriptor.free_blocks_count_lo == 0 {
                continue;
            }
            let bitmap_block = descriptor.block_bitmap();
            let mut bitmap = self.read_block(bitmap_block)?;
            let first = if group == 0 {
                superblock.first_data_block as u32
            } else {
                0
            };
            for bit in first..superblock.blocks_per_group {
                let number = group as u64 * superblock.blocks_per_group as u64 + bit as u64;
                if number >= total_blocks {
                    break;
                }
                let index = bit as usize;
                if bitmap[index / 8] & (1 << (index % 8)) != 0 {
                    continue;
                }
                bitmap[index / 8] |= 1 << (index % 8);
                self.write_block_number(bitmap_block, &bitmap)?;
                descriptor.free_blocks_count_lo -= 1;
                self.write_group(group as usize, &descriptor)?;

                let mut updated = read_superblock(&self.device)?;
                updated.free_blocks_count_lo -= 1;
                write_superblock(&self.device, &updated)?;

                let zeros = alloc::vec![0u8; self.block_size()];
                self.write_block_number(number, &zeros)?;
                return Ok(number);
            }
        }
        Err(VfsError::NoSpace)
    }

    pub(super) fn free_block(&self, number: u64) -> Result<()> {
        let superblock = read_superblock(&self.device)?;
        let group = (number / superblock.blocks_per_group as u64) as usize;
        let index = (number % superblock.blocks_per_group as u64) as usize;
        let mut descriptor = self.read_group(group)?;
        let bitmap_block = descriptor.block_bitmap();
        let mut bitmap = self.read_block(bitmap_block)?;
        if bitmap[index / 8] & (1 << (index % 8)) == 0 {
            return Ok(());
        }
        bitmap[index / 8] &= !(1 << (index % 8));
        self.write_block_number(bitmap_block, &bitmap)?;
        descriptor.free_blocks_count_lo += 1;
        self.write_group(group, &descriptor)?;

        let mut updated = read_superblock(&self.device)?;
        updated.free_blocks_count_lo += 1;
        write_superblock(&self.device, &updated)
    }

    /// Kök extent düğümüne bir blok ekler; bitişikse mevcut extent'i büyütür.
    pub(super) fn append_extent(
        inode: &mut Ext4Inode,
        logical: u32,
        physical: u64,
    ) -> Result<()> {
        let magic = u16::from_le_bytes(inode.block[0..2].try_into().unwrap());
        if magic != EXTENT_MAGIC {
            return Err(VfsError::Unsupported);
        }
        let depth = u16::from_le_bytes(inode.block[6..8].try_into().unwrap());
        if depth != 0 {
            return Err(VfsError::Unsupported);
        }
        let entries = u16::from_le_bytes(inode.block[2..4].try_into().unwrap()) as usize;

        for index in 0..entries {
            let offset = EXTENT_HEADER_SIZE + index * EXTENT_ENTRY_SIZE;
            let start = u32::from_le_bytes(inode.block[offset..offset + 4].try_into().unwrap());
            let raw_length =
                u16::from_le_bytes(inode.block[offset + 4..offset + 6].try_into().unwrap());
            let high =
                u16::from_le_bytes(inode.block[offset + 6..offset + 8].try_into().unwrap());
            let low = u32::from_le_bytes(inode.block[offset + 8..offset + 12].try_into().unwrap());
            let start_physical = ((high as u64) << 32) | low as u64;
            let length = if raw_length <= 32768 {
                raw_length as u64
            } else {
                (raw_length - 32768) as u64
            };
            if start as u64 + length == logical as u64
                && start_physical + length == physical
                && length < 32768
            {
                let grown = (length + 1) as u16;
                inode.block[offset + 4..offset + 6].copy_from_slice(&grown.to_le_bytes());
                return Ok(());
            }
        }

        if entries >= EXTENT_ROOT_MAX {
            return Err(VfsError::NoSpace);
        }
        let offset = EXTENT_HEADER_SIZE + entries * EXTENT_ENTRY_SIZE;
        inode.block[offset..offset + 4].copy_from_slice(&logical.to_le_bytes());
        inode.block[offset + 4..offset + 6].copy_from_slice(&1u16.to_le_bytes());
        inode.block[offset + 6..offset + 8].copy_from_slice(&((physical >> 32) as u16).to_le_bytes());
        inode.block[offset + 8..offset + 12].copy_from_slice(&(physical as u32).to_le_bytes());
        inode.block[2..4].copy_from_slice(&((entries + 1) as u16).to_le_bytes());
        Ok(())
    }

    /// Dosyanın tüm fiziksel bloklarını serbest bırakır (yalnızca kök extent).
    pub(super) fn release_blocks(&self, inode: &Ext4Inode) -> Result<()> {
        if inode.flags & EXT4_EXTENTS_FL == 0 {
            return Ok(());
        }
        let magic = u16::from_le_bytes(inode.block[0..2].try_into().unwrap());
        if magic != EXTENT_MAGIC {
            return Ok(());
        }
        let depth = u16::from_le_bytes(inode.block[6..8].try_into().unwrap());
        if depth != 0 {
            return Ok(());
        }
        let entries = u16::from_le_bytes(inode.block[2..4].try_into().unwrap()) as usize;
        for index in 0..entries.min(EXTENT_ROOT_MAX) {
            let offset = EXTENT_HEADER_SIZE + index * EXTENT_ENTRY_SIZE;
            let raw_length =
                u16::from_le_bytes(inode.block[offset + 4..offset + 6].try_into().unwrap());
            let high =
                u16::from_le_bytes(inode.block[offset + 6..offset + 8].try_into().unwrap());
            let low = u32::from_le_bytes(inode.block[offset + 8..offset + 12].try_into().unwrap());
            let start = ((high as u64) << 32) | low as u64;
            let length = if raw_length <= 32768 {
                raw_length as u64
            } else {
                (raw_length - 32768) as u64
            };
            for block in start..start + length {
                self.free_block(block)?;
            }
        }
        Ok(())
    }

    /// Dosyaya yazar; gerektiğinde blok ayırır ve extent'i büyütür.
    pub(super) fn write_file(&self, offset: u64, buffer: &[u8]) -> Result<usize> {
        let block_size = self.block_size() as u64;
        let mut inode = *self.inode.lock();
        if inode.file_type() == FileType::Directory {
            return Err(VfsError::IsADirectory);
        }
        let mut done = 0usize;
        let mut allocated = 0u64;

        while done < buffer.len() {
            let position = offset + done as u64;
            let logical = (position / block_size) as u32;
            let within = (position % block_size) as usize;
            let count = (block_size as usize - within).min(buffer.len() - done);

            let physical = match self.extent_lookup(&inode, logical)? {
                Some(block) => block,
                None => {
                    let block = self.allocate_block()?;
                    if let Err(failure) = Self::append_extent(&mut inode, logical, block) {
                        let _ = self.free_block(block);
                        return Err(failure);
                    }
                    allocated += 1;
                    block
                }
            };

            let mut data = self.read_block(physical)?;
            data[within..within + count].copy_from_slice(&buffer[done..done + count]);
            self.write_block_number(physical, &data)?;
            done += count;
        }

        let end = offset + done as u64;
        if end > inode.size() {
            inode.size_lo = end as u32;
            inode.size_high = (end >> 32) as u32;
        }
        inode.blocks_lo = inode
            .blocks_lo
            .wrapping_add((allocated * (block_size / 512)) as u32);
        inode.modify_time = now_seconds();
        inode.change_time = inode.modify_time;
        self.write_inode_entry(self.number, &inode)?;
        *self.inode.lock() = inode;
        Ok(done)
    }

    fn find_entry_space(block: &[u8], needed: usize) -> Option<(usize, usize)> {
        let mut offset = 0usize;
        while offset + 8 <= block.len() {
            let record = u16::from_le_bytes(block[offset + 4..offset + 6].try_into().unwrap()) as usize;
            let name_length = block[offset + 6] as usize;
            if record < 8 || offset + record > block.len() {
                return None;
            }
            let actual = (8 + name_length + 3) & !3;
            if record >= actual + needed {
                return Some((offset, actual));
            }
            offset += record;
        }
        None
    }

    pub(super) fn insert_entry(
        &self,
        parent: &mut Ext4Inode,
        name: &str,
        number: InodeNumber,
        file_type: u8,
    ) -> Result<()> {
        let needed = (8 + name.len() + 3) & !3;
        let mut logical = 0u32;
        loop {
            match self.extent_lookup(parent, logical)? {
                Some(physical) => {
                    let mut block = self.read_block(physical)?;
                    if let Some((offset, actual)) = Self::find_entry_space(&block, needed) {
                        let record =
                            u16::from_le_bytes(block[offset + 4..offset + 6].try_into().unwrap())
                                as usize;
                        block[offset + 4..offset + 6]
                            .copy_from_slice(&(actual as u16).to_le_bytes());
                        let target = offset + actual;
                        block[target..target + 4]
                            .copy_from_slice(&(number as u32).to_le_bytes());
                        block[target + 4..target + 6]
                            .copy_from_slice(&((record - actual) as u16).to_le_bytes());
                        block[target + 6] = name.len() as u8;
                        block[target + 7] = file_type;
                        block[target + 8..target + 8 + name.len()]
                            .copy_from_slice(name.as_bytes());
                        self.write_block_number(physical, &block)?;
                        return Ok(());
                    }
                    logical += 1;
                }
                None => {
                    let physical = self.allocate_block()?;
                    if let Err(failure) = Self::append_extent(parent, logical, physical) {
                        let _ = self.free_block(physical);
                        return Err(failure);
                    }
                    let block_size = self.block_size();
                    let mut block = alloc::vec![0u8; block_size];
                    block[0..4].copy_from_slice(&(number as u32).to_le_bytes());
                    block[4..6].copy_from_slice(&(block_size as u16).to_le_bytes());
                    block[6] = name.len() as u8;
                    block[7] = file_type;
                    block[8..8 + name.len()].copy_from_slice(name.as_bytes());
                    self.write_block_number(physical, &block)?;

                    let size = (logical as u64 + 1) * block_size as u64;
                    if size > parent.size() {
                        parent.size_lo = size as u32;
                        parent.size_high = (size >> 32) as u32;
                    }
                    parent.blocks_lo = parent.blocks_lo.wrapping_add((block_size / 512) as u32);
                    return Ok(());
                }
            }
        }
    }

    pub(super) fn remove_entry(&self, parent: &mut Ext4Inode, name: &str) -> Result<bool> {
        let block_size = self.block_size();
        let mut logical = 0u32;
        while let Some(physical) = self.extent_lookup(parent, logical)? {
            let mut block = self.read_block(physical)?;
            let mut offset = 0usize;
            let mut previous: Option<(usize, usize)> = None;
            while offset + 8 <= block_size {
                let entry_inode =
                    u32::from_le_bytes(block[offset..offset + 4].try_into().unwrap());
                let record =
                    u16::from_le_bytes(block[offset + 4..offset + 6].try_into().unwrap()) as usize;
                let name_length = block[offset + 6] as usize;
                if record < 8 || offset + record > block_size {
                    break;
                }
                if entry_inode != 0
                    && name_length == name.len()
                    && offset + 8 + name_length <= block_size
                    && &block[offset + 8..offset + 8 + name_length] == name.as_bytes()
                {
                    match previous {
                        Some((previous_offset, previous_record)) => {
                            block[previous_offset + 4..previous_offset + 6].copy_from_slice(
                                &((previous_record + record) as u16).to_le_bytes(),
                            );
                        }
                        None => {
                            block[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());
                        }
                    }
                    self.write_block_number(physical, &block)?;
                    return Ok(true);
                }
                previous = Some((offset, record));
                offset += record;
            }
            logical += 1;
        }
        Ok(false)
    }

    /// Yeni dizin için "." ve ".." girişlerini ve ilk bloğunu hazırlar.
    pub(super) fn initialise_directory(
        &self,
        inode: &mut Ext4Inode,
        number: InodeNumber,
        parent: InodeNumber,
    ) -> Result<()> {
        let block_size = self.block_size();
        let physical = self.allocate_block()?;
        if let Err(failure) = Self::append_extent(inode, 0, physical) {
            let _ = self.free_block(physical);
            return Err(failure);
        }
        let mut block = alloc::vec![0u8; block_size];
        block[0..4].copy_from_slice(&(number as u32).to_le_bytes());
        block[4..6].copy_from_slice(&(12u16).to_le_bytes());
        block[6] = 1;
        block[7] = DIRENT_DIRECTORY;
        block[8] = b'.';
        block[12..16].copy_from_slice(&(parent as u32).to_le_bytes());
        block[16..18].copy_from_slice(&((block_size - 12) as u16).to_le_bytes());
        block[18] = 2;
        block[19] = DIRENT_DIRECTORY;
        block[20] = b'.';
        block[21] = b'.';
        self.write_block_number(physical, &block)?;

        inode.size_lo = block_size as u32;
        inode.size_high = 0;
        inode.links_count = 2;
        inode.blocks_lo = (block_size / 512) as u32;
        Ok(())
    }

    pub(super) fn create_entry(
        &self,
        name: &str,
        file_type: FileType,
        mode: u32,
    ) -> Result<Arc<dyn Inode>> {
        let mut parent = *self.inode.lock();
        if parent.file_type() != FileType::Directory {
            return Err(VfsError::NotADirectory);
        }
        if name.is_empty() || name.len() > 255 || name.contains('/') {
            return Err(VfsError::InvalidArgument);
        }
        for (existing, _, _) in Ext4InodeHandle::readdir(self, &parent)? {
            if existing == name {
                return Err(VfsError::AlreadyExists);
            }
        }

        let raw_mode = mode_bits(file_type) | (mode as u16 & 0x0FFF);
        let (number, mut inode) = self.allocate_inode(raw_mode)?;
        if file_type == FileType::Directory {
            if let Err(failure) = self.initialise_directory(&mut inode, number, self.number) {
                let _ = self.free_inode(number);
                return Err(failure);
            }
        }
        if let Err(failure) = self.write_inode_entry(number, &inode) {
            let _ = self.free_inode(number);
            return Err(failure);
        }
        if let Err(failure) = self.insert_entry(&mut parent, name, number, file_type_code(file_type))
        {
            let _ = self.free_inode(number);
            return Err(failure);
        }
        if file_type == FileType::Directory {
            parent.links_count += 1;
            parent.modify_time = now_seconds();
            self.write_inode_entry(self.number, &parent)?;
        }
        *self.inode.lock() = parent;
        Ok(Arc::new(self.child(number)?) as Arc<dyn Inode>)
    }

    pub(super) fn unlink_entry(&self, name: &str) -> Result<()> {
        let mut parent = *self.inode.lock();
        if parent.file_type() != FileType::Directory {
            return Err(VfsError::NotADirectory);
        }
        let mut target: Option<InodeNumber> = None;
        for (existing, number, _) in Ext4InodeHandle::readdir(self, &parent)? {
            if existing == name {
                target = Some(number);
                break;
            }
        }
        let number = target.ok_or(VfsError::NotFound)?;
        if !self.remove_entry(&mut parent, name)? {
            return Err(VfsError::NotFound);
        }

        let mut inode = self.read_inode(number)?;
        let is_directory = inode.file_type() == FileType::Directory;
        if is_directory {
            inode.links_count = 0;
        } else if inode.links_count > 0 {
            inode.links_count -= 1;
        }

        if inode.links_count == 0 {
            self.release_blocks(&inode)?;
            self.free_inode(number)?;
        } else {
            self.write_inode_entry(number, &inode)?;
        }
        if is_directory && parent.links_count > 2 {
            parent.links_count -= 1;
        }
        parent.modify_time = now_seconds();
        self.write_inode_entry(self.number, &parent)?;
        *self.inode.lock() = parent;
        Ok(())
    }

    pub(super) fn truncate_entry(&self, size: u64) -> Result<()> {
        let mut inode = *self.inode.lock();
        if inode.file_type() == FileType::Directory {
            return Err(VfsError::IsADirectory);
        }
        if size == 0 {
            self.release_blocks(&inode)?;
            inode.size_lo = 0;
            inode.size_high = 0;
            inode.blocks_lo = 0;
            inode.block[2..4].copy_from_slice(&0u16.to_le_bytes());
        } else if size < inode.size() {
            // Kısmi küçültmede bloklar korunur (extent'ler yeniden yazılmıyor).
            inode.size_lo = size as u32;
            inode.size_high = (size >> 32) as u32;
        }
        inode.modify_time = now_seconds();
        inode.change_time = inode.modify_time;
        self.write_inode_entry(self.number, &inode)?;
        *self.inode.lock() = inode;
        Ok(())
    }

    pub(super) fn directory_entries(&self) -> Result<Vec<(String, InodeNumber, FileType)>> {
        let inode = *self.inode.lock();
        Ext4InodeHandle::readdir(self, &inode)
    }
}
