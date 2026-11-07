//! ext4: süper blok, grup tanımlayıcıları, inode ve extent ağacı.
//!
//! Okuma yolu yazıldı; yazma ve JBD2 henüz yok (`ROADMAP.md` M4). Bu modül
//! blok aygıtı olarak yalnızca `BlockDevice` trait'ini görür, böylece virtio-blk
//! ve AHCI aynı kodu kullanır.
//!
//! Alan yerleşimleri ext4 doc'un "Superblock" ve "Inode" bölümlerine göre.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{FileSystem, FileType, Inode, InodeNumber, Metadata, Result, VfsError};

pub mod write;

pub const EXT4_MAGIC: u16 = 0xEF53;
pub const EXT4_EXTENTS_FL: u32 = 0x0008_0000;
pub const EXT4_INLINE_DATA_FL: u32 = 0x1000_0000;
const EXTENT_MAGIC: u16 = 0xF30A;

pub trait BlockDevice: Send + Sync {
    fn block_size(&self) -> usize;
    fn read_block(&self, block: u64, buffer: &mut [u8]) -> Result<()>;
    fn write_block(&self, block: u64, buffer: &[u8]) -> Result<()>;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Superblock {
    pub inodes_count: u32,
    pub blocks_count_lo: u32,
    pub reserved_blocks_lo: u32,
    pub free_blocks_count_lo: u32,
    pub free_inodes_count: u32,
    pub first_data_block: u32,
    pub log_block_size: u32,
    pub log_cluster_size: u32,
    pub blocks_per_group: u32,
    pub clusters_per_group: u32,
    pub inodes_per_group: u32,
    pub mount_time: u32,
    pub write_time: u32,
    pub mount_count: u16,
    pub max_mount_count: u16,
    pub magic: u16,
    pub state: u16,
    pub errors: u16,
    pub minor_revision: u16,
    pub last_check: u32,
    pub check_interval: u32,
    pub creator_os: u32,
    pub revision_level: u32,
    pub default_uid: u16,
    pub default_gid: u16,
    pub first_inode: u32,
    pub inode_size: u16,
    pub block_group_nr: u16,
    pub feature_compat: u32,
    pub feature_incompat: u32,
    pub feature_ro_compat: u32,
    pub uuid: [u8; 16],
    pub volume_name: [u8; 16],
    pub last_mounted: [u8; 64],
    pub algorithm_usage_bitmap: u32,
    pub prealloc_blocks: u8,
    pub prealloc_dir_blocks: u8,
    pub reserved_gdt_blocks: u16,
    pub journal_uuid: [u8; 16],
    pub journal_inode: u32,
    pub journal_device: u32,
    pub last_orphan: u32,
    pub hash_seed: [u32; 4],
    pub default_hash_version: u8,
    pub journal_backup_type: u8,
    pub descriptor_size: u16,
    pub default_mount_options: u32,
    pub first_meta_bg: u32,
    pub mkfs_time: u32,
    pub journal_blocks: [u32; 17],
    pub blocks_count_hi: u32,
    pub reserved_blocks_hi: u32,
    pub free_blocks_count_hi: u32,
    pub min_extra_isize: u16,
    pub want_extra_isize: u16,
    pub flags: u32,
    pub raid_stride: u16,
    pub mmp_interval: u16,
    pub mmp_block: u64,
    pub raid_stripe_width: u32,
    pub log_groups_per_flex: u8,
    pub checksum_type: u8,
    pub reserved_pad: u16,
    pub kilo_blocks_written: u64,
    pub reserved: [u8; 152],
}

impl Superblock {
    pub fn block_size(&self) -> u64 {
        1024u64 << self.log_block_size
    }

    pub fn blocks_count(&self) -> u64 {
        ((self.blocks_count_hi as u64) << 32) | self.blocks_count_lo as u64
    }

    pub fn inode_size(&self) -> u16 {
        if self.revision_level == 0 {
            128
        } else {
            self.inode_size.max(128)
        }
    }

    pub fn descriptor_entry_size(&self) -> u16 {
        let is_64bit = self.feature_incompat & 0x80 != 0;
        if is_64bit {
            self.descriptor_size.max(32)
        } else {
            32
        }
    }

    pub fn group_count(&self) -> u32 {
        let blocks = self.blocks_count().saturating_sub(self.first_data_block as u64);
        ((blocks + self.blocks_per_group as u64 - 1) / self.blocks_per_group as u64) as u32
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GroupDescriptor {
    pub block_bitmap_lo: u32,
    pub inode_bitmap_lo: u32,
    pub inode_table_lo: u32,
    pub free_blocks_count_lo: u16,
    pub free_inodes_count_lo: u16,
    pub used_dirs_count_lo: u16,
    pub flags: u16,
    pub exclude_bitmap_lo: u32,
    pub block_bitmap_csum_lo: u16,
    pub inode_bitmap_csum_lo: u16,
    pub itable_unused_lo: u16,
    pub checksum: u16,
    pub block_bitmap_hi: u32,
    pub inode_bitmap_hi: u32,
    pub inode_table_hi: u32,
    pub free_blocks_count_hi: u16,
    pub free_inodes_count_hi: u16,
    pub used_dirs_count_hi: u16,
    pub itable_unused_hi: u16,
    pub exclude_bitmap_hi: u32,
    pub block_bitmap_csum_hi: u16,
    pub inode_bitmap_csum_hi: u16,
    pub reserved: u32,
}

impl GroupDescriptor {
    pub fn inode_table(&self) -> u64 {
        ((self.inode_table_hi as u64) << 32) | self.inode_table_lo as u64
    }

    pub fn block_bitmap(&self) -> u64 {
        ((self.block_bitmap_hi as u64) << 32) | self.block_bitmap_lo as u64
    }

    pub fn inode_bitmap(&self) -> u64 {
        ((self.inode_bitmap_hi as u64) << 32) | self.inode_bitmap_lo as u64
    }
}

/// 32 baytlık (64bit olmayan) grup tanımlayıcılarını 64 baytlık yapıya açar.
pub fn parse_group_descriptor(bytes: &[u8], entry_size: usize) -> GroupDescriptor {
    let mut descriptor: GroupDescriptor = unsafe { core::mem::zeroed() };
    let count = entry_size.min(core::mem::size_of::<GroupDescriptor>()).min(bytes.len());
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            &mut descriptor as *mut GroupDescriptor as *mut u8,
            count,
        );
    }
    descriptor
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ext4Inode {
    pub mode: u16,
    pub uid: u16,
    pub size_lo: u32,
    pub access_time: u32,
    pub change_time: u32,
    pub modify_time: u32,
    pub delete_time: u32,
    pub gid: u16,
    pub links_count: u16,
    pub blocks_lo: u32,
    pub flags: u32,
    pub os_dependent1: u32,
    pub block: [u8; 60],
    pub generation: u32,
    pub file_acl_lo: u32,
    pub size_high: u32,
    pub file_acl_high: u16,
    pub uid_high: u16,
    pub gid_high: u16,
    pub checksum_lo: u16,
    pub reserved: u16,
    pub extra_isize: u16,
    pub checksum_high: u16,
    pub change_time_extra: u32,
    pub modify_time_extra: u32,
    pub access_time_extra: u32,
    pub create_time: u32,
    pub create_time_extra: u32,
    pub version_high: u32,
    pub version: u32,
    pub project_id: u32,
    pub reserved2: [u8; 12],
}

impl Ext4Inode {
    pub fn size(&self) -> u64 {
        ((self.size_high as u64) << 32) | self.size_lo as u64
    }

    pub fn file_type(&self) -> FileType {
        match self.mode & 0xF000 {
            0x4000 => FileType::Directory,
            0x8000 => FileType::Regular,
            0xA000 => FileType::Symlink,
            0x2000 => FileType::CharDevice,
            0x6000 => FileType::BlockDevice,
            0x1000 => FileType::Fifo,
            0xC000 => FileType::Socket,
            _ => FileType::Regular,
        }
    }

    pub fn to_metadata(&self, number: InodeNumber) -> Metadata {
        let mut metadata = Metadata::regular(number, self.size());
        metadata.file_type = self.file_type();
        metadata.mode = (self.mode & 0x0FFF) as u32;
        metadata.uid = self.uid as u32 | ((self.uid_high as u32) << 16);
        metadata.gid = self.gid as u32 | ((self.gid_high as u32) << 16);
        metadata.links = self.links_count as u32;
        metadata.blocks = self.blocks_lo as u64;
        metadata.access_time = self.access_time as i64;
        metadata.modify_time = self.modify_time as i64;
        metadata.change_time = self.change_time as i64;
        if metadata.file_type == FileType::Directory {
            metadata.size = self.size();
        }
        metadata
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ExtentHeader {
    magic: u16,
    entries: u16,
    max: u16,
    depth: u16,
    generation: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Extent {
    block: u32,
    length: u16,
    start_high: u16,
    start_low: u32,
}

impl Extent {
    fn physical_start(&self) -> u64 {
        ((self.start_high as u64) << 32) | self.start_low as u64
    }

    fn physical_length(&self) -> u64 {
        // En yüksek bit işaretlenmemişse uzunluk 32768'i aşabilir.
        if self.length <= 32768 {
            self.length as u64
        } else {
            (self.length - 32768) as u64
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ExtentIndex {
    block: u32,
    leaf_low: u32,
    leaf_high: u16,
    reserved: u16,
}

impl ExtentIndex {
    fn leaf(&self) -> u64 {
        ((self.leaf_high as u64) << 32) | self.leaf_low as u64
    }
}

pub struct Ext4<D: BlockDevice> {
    device: Arc<D>,
    superblock: Superblock,
    group_descriptors: Vec<GroupDescriptor>,
    root_inode: Arc<Ext4InodeHandle<D>>,
}

impl<D: BlockDevice + 'static> Ext4<D> {
    pub fn mount(device: Arc<D>) -> Result<Self> {
        let mut buffer = alloc::vec![0u8; 1024];
        device.read_block(2, &mut buffer)?;

        let superblock: Superblock = unsafe { core::ptr::read_unaligned(buffer.as_ptr() as *const Superblock) };
        if superblock.magic != EXT4_MAGIC {
            return Err(VfsError::InvalidArgument);
        }

        let block_size = superblock.block_size() as usize;
        // BlockDevice 512 baytlık sektörlerle çalışır; ext4 blok numaraları
        // sektöre çevrilir.
        let sectors_per_block = (block_size / 512) as u64;
        let table_block = if block_size == 1024 { 2 } else { 1 };
        let mut table = alloc::vec![0u8; block_size];
        device.read_block(table_block as u64 * sectors_per_block, &mut table)?;

        let entry_size = superblock.descriptor_entry_size() as usize;
        let group_count = superblock.group_count() as usize;
        let mut group_descriptors = Vec::with_capacity(group_count);

        for index in 0..group_count {
            let offset = index * entry_size;
            if offset + entry_size > table.len() {
                // Birden fazla grup tanımlayıcı bloğu: sonraki bloklara geç.
                let block = table_block as u64 + 1 + (offset / block_size) as u64;
                device.read_block(block * sectors_per_block, &mut table)?;
            }
            let offset = offset % block_size;
            group_descriptors.push(parse_group_descriptor(&table[offset..], entry_size));
        }

        let group_descriptors = alloc::sync::Arc::new(group_descriptors);

        // Kök inode (2).
        let root_inode = read_inode_from(&device, &superblock, &group_descriptors, 2)?;
        let root_handle = Arc::new(Ext4InodeHandle {
            device: device.clone(),
            superblock,
            group_descriptors,
            number: 2,
            inode: crate::sync::SpinLock::new(root_inode),
        });

        println!(
            "ext4: {} blok, {} inode, blok boyu {}",
            superblock.blocks_count(),
            superblock.inodes_count,
            superblock.block_size()
        );

        Ok(Self {
            device,
            superblock,
            group_descriptors: Vec::new(),
            root_inode: root_handle,
        })
    }

    pub fn superblock(&self) -> &Superblock {
        &self.superblock
    }
}

pub struct Ext4InodeHandle<D: BlockDevice> {
    device: Arc<D>,
    superblock: Superblock,
    group_descriptors: Arc<Vec<GroupDescriptor>>,
    number: InodeNumber,
    inode: crate::sync::SpinLock<Ext4Inode>,
}

impl<D: BlockDevice> Ext4InodeHandle<D> {
    fn block_size(&self) -> usize {
        self.superblock.block_size() as usize
    }

    fn read_block(&self, block: u64) -> Result<Vec<u8>> {
        let mut buffer = alloc::vec![0u8; self.block_size()];
        let sector = block * (self.block_size() as u64 / 512);
        self.device.read_block(sector, &mut buffer)?;
        Ok(buffer)
    }

    /// Aynı dosya sisteminde başka bir inode için tutamaç üretir.
    fn child(&self, number: InodeNumber) -> Result<Ext4InodeHandle<D>> {
        let inode = self.read_inode(number)?;
        Ok(Ext4InodeHandle {
            device: self.device.clone(),
            superblock: self.superblock,
            group_descriptors: self.group_descriptors.clone(),
            number,
            inode: crate::sync::SpinLock::new(inode),
        })
    }

    pub fn read_inode(&self, number: InodeNumber) -> Result<Ext4Inode> {
        read_inode_from(&self.device, &self.superblock, &self.group_descriptors, number)
    }

    /// Extent ağacını yürüyüp mantıksal bloğu fiziksel bloğa çevirir.
    fn extent_lookup(&self, inode: &Ext4Inode, logical: u32) -> Result<Option<u64>> {
        if inode.flags & EXT4_EXTENTS_FL == 0 {
            return Err(VfsError::Unsupported); // eski blok haritası
        }
        self.walk_extent_node(&inode.block, logical, 0)
    }

    fn walk_extent_node(&self, node: &[u8], logical: u32, depth_guard: usize) -> Result<Option<u64>> {
        if depth_guard > 8 || node.len() < core::mem::size_of::<ExtentHeader>() {
            return Err(VfsError::Io);
        }
        let header: ExtentHeader = unsafe { core::ptr::read_unaligned(node.as_ptr() as *const ExtentHeader) };
        if header.magic != EXTENT_MAGIC {
            return Err(VfsError::Io);
        }

        let count = header.entries as usize;
        if header.depth == 0 {
            for index in 0..count {
                let offset = core::mem::size_of::<ExtentHeader>() + index * core::mem::size_of::<Extent>();
                if offset + core::mem::size_of::<Extent>() > node.len() {
                    break;
                }
                let extent: Extent = unsafe {
                    core::ptr::read_unaligned(node[offset..].as_ptr() as *const Extent)
                };
                let start = extent.block;
                let length = extent.physical_length() as u32;
                if logical >= start && logical < start + length {
                    return Ok(Some(extent.physical_start() + (logical - start) as u64));
                }
            }
            return Ok(None);
        }

        // İç düğüm: mantıksal bloğu kapsayan son indeksi seç.
        let mut chosen: Option<ExtentIndex> = None;
        for index in 0..count {
            let offset =
                core::mem::size_of::<ExtentHeader>() + index * core::mem::size_of::<ExtentIndex>();
            if offset + core::mem::size_of::<ExtentIndex>() > node.len() {
                break;
            }
            let entry: ExtentIndex = unsafe {
                core::ptr::read_unaligned(node[offset..].as_ptr() as *const ExtentIndex)
            };
            if entry.block <= logical {
                chosen = Some(entry);
            }
        }

        match chosen {
            Some(entry) => {
                let block = self.read_block(entry.leaf())?;
                self.walk_extent_node(&block, logical, depth_guard + 1)
            }
            None => Ok(None),
        }
    }

    pub fn read_file(&self, inode: &Ext4Inode, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let size = inode.size();
        if offset >= size {
            return Ok(0);
        }
        let available = (size - offset) as usize;
        let total = available.min(buffer.len());
        let block_size = self.block_size();
        let mut done = 0usize;

        while done < total {
            let position = offset + done as u64;
            let logical = (position / block_size as u64) as u32;
            let within = (position % block_size as u64) as usize;
            let count = (block_size - within).min(total - done);

            match self.extent_lookup(inode, logical)? {
                Some(physical) => {
                    let block = self.read_block(physical)?;
                    buffer[done..done + count].copy_from_slice(&block[within..within + count]);
                }
                None => {
                    // Seyrek dosya: delik sıfır okunur.
                    for byte in &mut buffer[done..done + count] {
                        *byte = 0;
                    }
                }
            }
            done += count;
        }

        Ok(done)
    }

    pub fn readdir(&self, inode: &Ext4Inode) -> Result<Vec<(String, InodeNumber, FileType)>> {
        // Dizin boyutu (i_size) genelde yalnızca ilk bloğu kapsar; kalan
        // girişler sonraki bloklarda olabilir. Bu yüzden mantıksal bloklar
        // extent zinciri bitene kadar okunur.
        let mut entries = Vec::new();
        let mut logical = 0u32;
        loop {
            let Some(physical) = self.extent_lookup(inode, logical)? else {
                break;
            };
            let block = self.read_block(physical)?;
            parse_directory_block(&block, &mut entries);
            logical += 1;
            if logical as u64 > (32 * 1024 * 1024 / self.block_size() as u64) {
                break;
            }
        }
        Ok(entries)
    }
}

fn parse_directory_block(block: &[u8], entries: &mut Vec<(String, InodeNumber, FileType)>) {
    let mut offset = 0usize;
    while offset + 8 <= block.len() {
        let inode_number = u32::from_le_bytes(block[offset..offset + 4].try_into().unwrap());
        let record_length =
            u16::from_le_bytes(block[offset + 4..offset + 6].try_into().unwrap()) as usize;
        let name_length = block[offset + 6] as usize;
        let file_type = block[offset + 7];

        if record_length < 8 || offset + record_length > block.len() {
            break;
        }

        if inode_number != 0 && name_length > 0 && offset + 8 + name_length <= block.len() {
            let name = &block[offset + 8..offset + 8 + name_length];
            if let Ok(name) = core::str::from_utf8(name) {
                if name != "." && name != ".." {
                    entries.push((
                        String::from(name),
                        inode_number as u64,
                        match file_type {
                            1 => FileType::Regular,
                            2 => FileType::Directory,
                            7 => FileType::Symlink,
                            _ => FileType::Regular,
                        },
                    ));
                }
            }
        }

        offset += record_length;
    }
}

impl<D: BlockDevice + 'static> FileSystem for Ext4<D> {
    fn name(&self) -> &'static str {
        "ext4"
    }

    fn root(&self) -> Arc<dyn Inode> {
        self.root_inode.clone()
    }
}

impl<D: BlockDevice + 'static> Inode for Ext4InodeHandle<D> {
    fn metadata(&self) -> Metadata {
        (*self.inode.lock()).to_metadata(self.number)
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let inode = *self.inode.lock();
        // Kısa sembolik bağ hedefi i_block içinde satır içi durur.
        if inode.file_type() == FileType::Symlink && inode.size() < 60 {
            let target = &inode.block[..inode.size() as usize];
            let start = (offset as usize).min(target.len());
            let count = (target.len() - start).min(buffer.len());
            buffer[..count].copy_from_slice(&target[start..start + count]);
            return Ok(count);
        }

        self.read_file(&inode, offset, buffer)
    }

    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<usize> {
        self.write_file(offset, buffer)
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        let inode = *self.inode.lock();
        if !matches!(inode.file_type(), FileType::Directory) {
            return Err(VfsError::NotADirectory);
        }
        for (entry_name, number, _) in self.readdir(&inode)? {
            if entry_name == name {
                return Ok(Arc::new(self.child(number)?) as Arc<dyn Inode>);
            }
        }
        Err(VfsError::NotFound)
    }

    fn create(&self, name: &str, file_type: FileType, mode: u32) -> Result<Arc<dyn Inode>> {
        self.create_entry(name, file_type, mode)
    }

    fn unlink(&self, name: &str) -> Result<()> {
        self.unlink_entry(name)
    }

    fn readdir(&self) -> Result<Vec<(String, Arc<dyn Inode>)>> {
        let inode = *self.inode.lock();
        let entries = Ext4InodeHandle::readdir(self, &inode)?;
        let mut result = Vec::with_capacity(entries.len());
        for (name, number, _) in entries {
            match self.child(number) {
                Ok(handle) => result.push((name, Arc::new(handle) as Arc<dyn Inode>)),
                Err(_) => continue,
            }
        }
        Ok(result)
    }

    fn truncate(&self, size: u64) -> Result<()> {
        self.truncate_entry(size)
    }

    fn sync(&self) -> Result<()> {
        // Her işlem doğrudan diske yazılıyor; yalnızca aygıt tamponu kalabilir.
        Ok(())
    }
}

/// Bir inode numarasını grup tanımlayıcısı ve inode tablosundan okur.
fn read_inode_from<D: BlockDevice>(
    device: &Arc<D>,
    superblock: &Superblock,
    group_descriptors: &[GroupDescriptor],
    number: InodeNumber,
) -> Result<Ext4Inode> {
    if number == 0 {
        return Err(VfsError::InvalidArgument);
    }

    let block_size = superblock.block_size() as u64;
    let sectors_per_block = block_size / 512;
    let group = ((number - 1) / superblock.inodes_per_group as u64) as usize;
    let index = (number - 1) % superblock.inodes_per_group as u64;

    let descriptor = group_descriptors
        .get(group)
        .ok_or(VfsError::InvalidArgument)?;

    let inode_size = superblock.inode_size() as u64;
    let offset = index * inode_size;
    let block = descriptor.inode_table() + offset / block_size;
    let within = (offset % block_size) as usize;

    let mut buffer = alloc::vec![0u8; block_size as usize];
    device.read_block(block * sectors_per_block, &mut buffer)?;

    if within + core::mem::size_of::<Ext4Inode>() > buffer.len() {
        return Err(VfsError::Io);
    }

    Ok(unsafe { core::ptr::read_unaligned(buffer[within..].as_ptr() as *const Ext4Inode) })
}
