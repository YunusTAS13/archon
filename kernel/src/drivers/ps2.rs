//! PS/2 klavye, scancode set 1.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::arch::port::inb;
use crate::sync::SpinLock;

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const BUFFER_SIZE: usize = 256;

const KEY_LEFT_SHIFT: u16 = 0x2A;
const KEY_RIGHT_SHIFT: u16 = 0x36;
const KEY_CAPS_LOCK: u16 = 0x3A;
const KEY_LEFT_CTRL: u16 = 0x1D;
const KEY_LEFT_ALT: u16 = 0x38;

static EXTENDED: AtomicBool = AtomicBool::new(false);

/// `0xE0` önekli tuşların Linux anahtar kodları.
fn extended_code(base: u16) -> Option<u16> {
    Some(match base {
        0x48 => 103, // KEY_UP
        0x50 => 108, // KEY_DOWN
        0x4B => 105, // KEY_LEFT
        0x4D => 106, // KEY_RIGHT
        0x47 => 102, // KEY_HOME
        0x4F => 107, // KEY_END
        0x49 => 104, // KEY_PAGEUP
        0x51 => 109, // KEY_PAGEDOWN
        0x53 => 111, // KEY_DELETE
        0x52 => 110, // KEY_INSERT
        0x1C => 96,  // KEY_KPENTER
        0x1D => 97,  // KEY_RIGHTCTRL
        0x38 => 100, // KEY_RIGHTALT
        _ => return None,
    })
}

static BUFFER: [AtomicUsize; BUFFER_SIZE] = [const { AtomicUsize::new(0) }; BUFFER_SIZE];
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);
static SHIFT: AtomicBool = AtomicBool::new(false);
static CAPS: AtomicBool = AtomicBool::new(false);
static CTRL: AtomicBool = AtomicBool::new(false);
static ALT: AtomicBool = AtomicBool::new(false);

const SCANCODE_MAP: [u8; 58] = [
    0, 27, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b'-', b'=', 8, b'\t',
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', b'[', b']', b'\n', 0, b'a',
    b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', b'\'', b'`', 0, b'\\', b'z', b'x',
    b'c', b'v', b'b', b'n', b'm', b',', b'.', b'/', 0, b'*', 0, b' ',
];

const SCANCODE_MAP_SHIFT: [u8; 58] = [
    0, 27, b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*', b'(', b')', b'_', b'+', 8, b'\t',
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', b'{', b'}', b'\n', 0, b'A',
    b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':', b'"', b'~', 0, b'|', b'Z', b'X',
    b'C', b'V', b'B', b'N', b'M', b'<', b'>', b'?', 0, b'*', 0, b' ',
];

fn push(value: u8) {
    let head = HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % BUFFER_SIZE;
    if next == TAIL.load(Ordering::Acquire) {
        return; // tampon dolu, tuş düşürülür
    }
    BUFFER[head].store(value as usize, Ordering::Relaxed);
    HEAD.store(next, Ordering::Release);
}

/// IRQ1 gövdesi. `idt::isr_dispatch` çağırır.
pub fn on_keyboard_irq() {
    let status = unsafe { inb(STATUS_PORT) };
    if status & 1 == 0 {
        return;
    }
    let scancode = unsafe { inb(DATA_PORT) };

    // `evdev` olayları: Linux anahtar kodları set 1 tarama kodlarıyla aynıdır.
    if scancode == 0xE0 {
        EXTENDED.store(true, Ordering::Relaxed);
        return;
    }
    let extended = EXTENDED.swap(false, Ordering::Relaxed);
    let pressed = scancode & 0x80 == 0;
    let base = (scancode & 0x7F) as u16;
    if extended {
        if let Some(code) = extended_code(base) {
            crate::drivers::evdev::push_key(code, if pressed { 1 } else { 0 });
        }
        return;
    }
    if base != 0 && base <= 0x58 {
        crate::drivers::evdev::push_key(base, if pressed { 1 } else { 0 });
    }

    if scancode & 0x80 != 0 {
        let released = (scancode & 0x7F) as u16;
        match released {
            KEY_LEFT_SHIFT | KEY_RIGHT_SHIFT => SHIFT.store(false, Ordering::Relaxed),
            KEY_LEFT_CTRL => CTRL.store(false, Ordering::Relaxed),
            KEY_LEFT_ALT => ALT.store(false, Ordering::Relaxed),
            _ => {}
        }
        return;
    }

    match scancode as u16 {
        KEY_LEFT_SHIFT | KEY_RIGHT_SHIFT => {
            SHIFT.store(true, Ordering::Relaxed);
            return;
        }
        KEY_CAPS_LOCK => {
            let previous = CAPS.load(Ordering::Relaxed);
            CAPS.store(!previous, Ordering::Relaxed);
            return;
        }
        KEY_LEFT_CTRL => {
            CTRL.store(true, Ordering::Relaxed);
            return;
        }
        KEY_LEFT_ALT => {
            ALT.store(true, Ordering::Relaxed);
            return;
        }
        _ => {}
    }

    let index = scancode as usize;
    if index >= SCANCODE_MAP.len() {
        return;
    }

    let shift = SHIFT.load(Ordering::Relaxed);
    let caps = CAPS.load(Ordering::Relaxed);
    let mut character = if shift {
        SCANCODE_MAP_SHIFT[index]
    } else {
        SCANCODE_MAP[index]
    };

    if caps && character.is_ascii_alphabetic() {
        character ^= 0x20;
    }

    // Ctrl + harf: kontrol karakteri (^C = 0x03, ^Z = 0x1a, ^D = 0x04).
    if CTRL.load(Ordering::Relaxed) {
        let lower = character.to_ascii_lowercase();
        if lower.is_ascii_alphabetic() {
            character = lower - b'a' + 1;
        }
    }

    if character != 0 {
        push(character);
    }
}

/// Tamponda bekleyen karakter sayısı.
pub fn pending() -> usize {
    let head = HEAD.load(Ordering::Acquire);
    let tail = TAIL.load(Ordering::Acquire);
    (head + BUFFER_SIZE - tail) % BUFFER_SIZE
}

pub fn read_char() -> Option<u8> {
    let tail = TAIL.load(Ordering::Acquire);
    if tail == HEAD.load(Ordering::Acquire) {
        return None;
    }
    let value = BUFFER[tail].load(Ordering::Relaxed) as u8;
    TAIL.store((tail + 1) % BUFFER_SIZE, Ordering::Release);
    Some(value)
}

pub fn read_line(buffer: &mut [u8]) -> Option<usize> {
    for index in 0..buffer.len() {
        match read_char() {
            Some(b'\n') => return Some(index),
            Some(byte) => buffer[index] = byte,
            None => return None,
        }
    }
    None
}

pub fn modifiers() -> (bool, bool) {
    (
        CTRL.load(Ordering::Relaxed),
        ALT.load(Ordering::Relaxed),
    )
}
