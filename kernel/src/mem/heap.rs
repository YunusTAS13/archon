//! Kernel heap: ilk uygun blok (first fit) ayırıcı.
//!
//! Arena BSS'te sabit boyutlu duruyor; Limine BSS'i sıfırlar, ayrı bir sanal
//! eşleme gerekmez. Blok başlıkları 16 bayt olduğu için normal hizalamalarda
//! (<= 16) yük her zaman 16'ya hizalı başlar. Daha büyük hizalamalarda
//! hizalama boşluğu blok içinde bırakılır ve serbest listeye dönmez; bu
//! kayıp en fazla `alignment` bayttır.

use core::alloc::{GlobalAlloc, Layout};
use core::mem::size_of;
use core::ptr;

use crate::sync::SpinLock;

const ARENA_SIZE: usize = 64 * 1024 * 1024;
const MIN_SPLIT: usize = 32;

#[repr(align(16))]
struct Arena([u8; ARENA_SIZE]);

static mut ARENA: Arena = Arena([0; ARENA_SIZE]);

#[repr(C)]
struct BlockHeader {
    size: usize,
    next: *mut BlockHeader,
}

const HEADER_SIZE: usize = size_of::<BlockHeader>();

struct Heap {
    head: *mut BlockHeader,
    total: usize,
    used: usize,
    initialized: bool,
}

unsafe impl Send for Heap {}

static HEAP: SpinLock<Heap> = SpinLock::new(Heap {
    head: ptr::null_mut(),
    total: 0,
    used: 0,
    initialized: false,
});

pub fn init() {
    let base = unsafe { core::ptr::addr_of_mut!(ARENA.0) } as usize;
    let mut heap = HEAP.lock();

    unsafe {
        let header = base as *mut BlockHeader;
        (*header).size = ARENA_SIZE;
        (*header).next = ptr::null_mut();
        heap.head = header;
    }
    heap.total = ARENA_SIZE;
    heap.used = 0;
    heap.initialized = true;

    println!("mem: heap {} MiB (BSS arena)", ARENA_SIZE / (1024 * 1024));
}

pub fn size() -> usize {
    ARENA_SIZE
}

pub fn used() -> usize {
    HEAP.lock().used
}

/// Tanılama: serbest blok sayısı ve toplam serbest bayt.
pub fn free_stats() -> (usize, usize) {
    let heap = HEAP.lock();
    let mut blocks = 0;
    let mut bytes = 0;
    let mut block = heap.head;
    unsafe {
        while !block.is_null() && blocks < 100_000 {
            blocks += 1;
            bytes += (*block).size;
            block = (*block).next;
        }
    }
    (blocks, bytes)
}

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

unsafe fn take_block(previous: *mut BlockHeader, block: *mut BlockHeader, heap: &mut Heap) {
    if previous.is_null() {
        heap.head = (*block).next;
    } else {
        (*previous).next = (*block).next;
    }
}

struct KernelAllocator;

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut heap = HEAP.lock();
        if !heap.initialized {
            return ptr::null_mut();
        }

        let alignment = layout.align().max(16);
        let size = align_up(layout.size().max(1), 16);

        let mut previous: *mut BlockHeader = ptr::null_mut();
        let mut block = heap.head;

        while !block.is_null() {
            // Başlığın üzerine yazmadan önce alanları yerele al.
            let block_size = (*block).size;
            let block_next = (*block).next;

            let payload = (block as usize) + HEADER_SIZE;
            let aligned = align_up(payload, alignment);
            let padding = aligned - payload;

            if block_size >= padding + size + HEADER_SIZE + MIN_SPLIT {
                let header = (aligned - HEADER_SIZE) as *mut BlockHeader;
                (*header).size = size;
                (*header).next = ptr::null_mut();

                let remainder = (aligned + size) as *mut BlockHeader;
                (*remainder).size = block_size - padding - size - HEADER_SIZE;
                (*remainder).next = block_next;

                if previous.is_null() {
                    heap.head = remainder;
                } else {
                    (*previous).next = remainder;
                }

                heap.used += size;
                return aligned as *mut u8;
            }

            if block_size >= padding + size {
                // Kalan parça bölünmeye değmez: bloğun tamamı verilir ve
                // gerçek boyut başlıkta saklanır.
                let header = (aligned - HEADER_SIZE) as *mut BlockHeader;
                (*header).size = block_size - padding;
                (*header).next = ptr::null_mut();

                if previous.is_null() {
                    heap.head = block_next;
                } else {
                    (*previous).next = block_next;
                }

                heap.used += (*header).size;
                return aligned as *mut u8;
            }

            previous = block;
            block = block_next;
        }

        ptr::null_mut()
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if pointer.is_null() {
            return;
        }

        let mut heap = HEAP.lock();
        if !heap.initialized {
            return;
        }

        let header = (pointer as usize - HEADER_SIZE) as *mut BlockHeader;
        let freed = (*header).size;
        (*header).next = ptr::null_mut();

        // Adres sırasına göre yerleştir.
        let mut previous: *mut BlockHeader = ptr::null_mut();
        let mut current = heap.head;
        while !current.is_null() && (current as usize) < (header as usize) {
            previous = current;
            current = (*current).next;
        }

        (*header).next = current;
        if previous.is_null() {
            heap.head = header;
        } else {
            (*previous).next = header;
        }

        // Sonraki blokla birleştir.
        if !current.is_null() {
            let end = (header as usize) + HEADER_SIZE + (*header).size;
            if end == current as usize {
                (*header).size += HEADER_SIZE + (*current).size;
                (*header).next = (*current).next;
            }
        }

        // Önceki blokla birleştir.
        if !previous.is_null() {
            let end = (previous as usize) + HEADER_SIZE + (*previous).size;
            if end == header as usize {
                (*previous).size += HEADER_SIZE + (*header).size;
                (*previous).next = (*header).next;
            }
        }

        heap.used = heap.used.saturating_sub(freed);
        let _ = layout;
    }
}

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator;
