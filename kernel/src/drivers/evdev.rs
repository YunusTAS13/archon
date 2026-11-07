//! Linux `evdev` olay kuyruğu: `/dev/input/event0`.
//!
//! Klavye IRQ'su buraya `input_event` yazar; kullanıcı alanı `read` ile
//! 24 baytlık kayıtları alır. Kuyruk, IRQ ile syscall bağlamı arasında
//! kilit paylaşmamak için tek üretici/tek tüketici halka tamponudur.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::sync::SpinLock;

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const SYN_REPORT: u16 = 0x00;

/// Linux `struct input_event` (x86_64): 16 bayt `timeval`, 2+2+4 bayt.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct InputEvent {
    pub seconds: i64,
    pub micros: i64,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

const CAPACITY: usize = 256;

struct Ring {
    slots: UnsafeCell<[InputEvent; CAPACITY]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl Sync for Ring {}

static RING: Ring = Ring {
    slots: UnsafeCell::new(
        [InputEvent {
            seconds: 0,
            micros: 0,
            kind: 0,
            code: 0,
            value: 0,
        }; CAPACITY],
    ),
    head: AtomicUsize::new(0),
    tail: AtomicUsize::new(0),
};

/// Tüketicileri sıraya sokan kilit; IRQ bu kilidi almaz.
static CONSUMER: SpinLock<()> = SpinLock::new(());

/// Basılı tuşların bit eşlemi (`EVIOCGKEY`).
static PRESSED: SpinLock<[u8; 16]> = SpinLock::new([0; 16]);

fn now() -> (i64, i64) {
    let nanos = crate::time::monotonic_nanos();
    ((nanos / 1_000_000_000) as i64, ((nanos % 1_000_000_000) / 1000) as i64)
}

fn push(event: InputEvent) {
    let head = RING.head.load(Ordering::Relaxed);
    let next = (head + 1) % CAPACITY;
    if next == RING.tail.load(Ordering::Acquire) {
        return; // kuyruk dolu: en yeni olay düşürülür
    }
    unsafe {
        (*RING.slots.get())[head] = event;
    }
    RING.head.store(next, Ordering::Release);
}

/// Bir tuş olayı ve ardından `SYN_REPORT` gönderir.
pub fn push_key(code: u16, value: i32) {
    let (seconds, micros) = now();
    push(InputEvent {
        seconds,
        micros,
        kind: EV_KEY,
        code,
        value,
    });
    push(InputEvent {
        seconds,
        micros,
        kind: EV_SYN,
        code: SYN_REPORT,
        value: 0,
    });

    if (code as usize) < 128 {
        let mut pressed = PRESSED.lock();
        let index = code as usize / 8;
        let bit = 1u8 << (code % 8);
        if value != 0 {
            pressed[index] |= bit;
        } else {
            pressed[index] &= !bit;
        }
    }
}

pub fn pop_event() -> Option<InputEvent> {
    let _guard = CONSUMER.lock();
    let tail = RING.tail.load(Ordering::Relaxed);
    if tail == RING.head.load(Ordering::Acquire) {
        return None;
    }
    let event = unsafe { (*RING.slots.get())[tail] };
    RING.tail.store((tail + 1) % CAPACITY, Ordering::Release);
    Some(event)
}

pub fn pending() -> usize {
    let head = RING.head.load(Ordering::Acquire);
    let tail = RING.tail.load(Ordering::Acquire);
    (head + CAPACITY - tail) % CAPACITY
}

fn key_supported(code: u16) -> bool {
    // Standart blok (1..=58) ve genişletilmiş gezinme/rakam takımı (59..=111).
    (1..=111).contains(&code) && code != 84 && code != 85 && code != 86 && code != 87
}

/// `EVIOCGBIT` için yetenek eşlemi.
fn capability(kind: u16, output: &mut [u8]) {
    if kind == 0 {
        output[0] = (1 << EV_SYN) | (1 << EV_KEY);
        return;
    }
    if kind != EV_KEY {
        return;
    }
    for code in 1..=111u16 {
        if !key_supported(code) {
            continue;
        }
        let index = code as usize / 8;
        if index < output.len() {
            output[index] |= 1u8 << (code % 8);
        }
    }
}

fn write_bytes(pointer: u64, bytes: &[u8]) -> i64 {
    match crate::syscall::copy_to_user(pointer, bytes) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

/// `ioctl` istekleri: `_IOC` kod çözümüyle `EVIOC*` ailesi.
pub fn ioctl(request: u64, argument: u64) -> Option<i64> {
    let number = (request & 0xFF) as u8;
    let size = ((request >> 16) & 0x3FFF) as usize;
    let direction = (request >> 30) as u8;
    if direction != 2 {
        return None; // yalnızca okuma istekleri
    }

    match number {
        0x01 => {
            // EVIOCGVERSION
            let version = 0x0001_0001i32;
            Some(write_bytes(argument, &version.to_le_bytes()))
        }
        0x02 => {
            // EVIOCGID: bus=virtio/ISA, vendor, product, version
            let mut bytes = [0u8; 8];
            bytes[0..2].copy_from_slice(&0x11u16.to_le_bytes());
            bytes[2..4].copy_from_slice(&0x0001u16.to_le_bytes());
            bytes[4..6].copy_from_slice(&0x0001u16.to_le_bytes());
            bytes[6..8].copy_from_slice(&0x0001u16.to_le_bytes());
            Some(write_bytes(argument, &bytes))
        }
        0x06 => {
            // EVIOCGNAME
            let name = b"archon-klavye\0";
            let count = size.min(name.len());
            Some(write_bytes(argument, &name[..count]))
        }
        0x07 => {
            // EVIOCGPHYS
            let path = b"isa0060/serio0/input0\0";
            let count = size.min(path.len());
            Some(write_bytes(argument, &path[..count]))
        }
        0x08 => Some(write_bytes(argument, &[0])),
        0x09 => {
            // EVIOCGPROP: özellik yok
            let zeros = alloc::vec![0u8; size];
            Some(write_bytes(argument, &zeros))
        }
        0x18 => {
            // EVIOCGKEY
            let pressed = PRESSED.lock();
            let count = size.min(pressed.len());
            let mut bytes = alloc::vec![0u8; size];
            bytes[..count].copy_from_slice(&pressed[..count]);
            Some(write_bytes(argument, &bytes))
        }
        _ if (0x20..=0x3F).contains(&number) => {
            let kind = (number - 0x20) as u16;
            let mut bytes = alloc::vec![0u8; size];
            capability(kind, &mut bytes);
            Some(write_bytes(argument, &bytes))
        }
        _ => None,
    }
}
