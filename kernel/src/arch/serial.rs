//! 16550 UART sürücüsü, COM1, 115200 8N1.
//!
//! Log çıktısı buradan gider; boot sırasında heap ve kilit altyapısı hazır
//! olmadan da çalışması gerekir.

use super::port::{inb, outb, COM1};

const REG_DATA: u16 = 0;
const REG_IER: u16 = 1;
const REG_FIFO_CTRL: u16 = 2;
const REG_LINE_CTRL: u16 = 3;
const REG_MODEM_CTRL: u16 = 4;
const REG_LINE_STATUS: u16 = 5;
const REG_SCRATCH: u16 = 7;

const LSR_DATA_READY: u8 = 0x01;
const LSR_THR_EMPTY: u8 = 0x20;

pub struct Serial;

impl Serial {
    pub fn init() {
        unsafe {
            outb(COM1 + REG_IER, 0x00);
            outb(COM1 + REG_LINE_CTRL, 0x80); // DLAB acik
            outb(COM1 + REG_DATA, 0x01); // 115200: bolen 1
            outb(COM1 + REG_IER, 0x00);
            outb(COM1 + REG_LINE_CTRL, 0x03); // 8N1, DLAB kapali
            outb(COM1 + REG_FIFO_CTRL, 0xC7); // FIFO acik, temizle, 14 bayt esik
            outb(COM1 + REG_MODEM_CTRL, 0x0B); // DTR, RTS, OUT2
            outb(COM1 + REG_SCRATCH, 0xA5);
            if inb(COM1 + REG_SCRATCH) != 0xA5 {
                // Seri port yok: yazma sessizce yutulur, boot devam eder.
                return;
            }
        }
    }

    pub fn write_byte(byte: u8) {
        unsafe {
            let mut spins = 0u32;
            while inb(COM1 + REG_LINE_STATUS) & LSR_THR_EMPTY == 0 {
                spins += 1;
                if spins > 1_000_000 {
                    return;
                }
                core::hint::spin_loop();
            }
            outb(COM1 + REG_DATA, byte);
        }
    }

    pub fn write_bytes(bytes: &[u8]) {
        for byte in bytes {
            if *byte == b'\n' {
                Self::write_byte(b'\r');
            }
            Self::write_byte(*byte);
        }
    }

    pub fn can_read() -> bool {
        unsafe { inb(COM1 + REG_LINE_STATUS) & LSR_DATA_READY != 0 }
    }

    pub fn read_byte() -> Option<u8> {
        if !Self::can_read() {
            return None;
        }
        Some(unsafe { inb(COM1 + REG_DATA) })
    }
}

pub fn init() {
    Serial::init();
}

/// IRQ4 ile gelen baytı kuyruğa atan kesme henüz bağlı değil; polling
/// `Serial::read_byte` üzerinden yapılıyor. M2 sonunda IRQ tabanlıya geçilecek.
pub fn poll_line(buffer: &mut [u8]) -> usize {
    let mut count = 0;
    while count < buffer.len() {
        match Serial::read_byte() {
            Some(byte) if byte == b'\r' => {
                buffer[count] = b'\n';
                count += 1;
            }
            Some(byte) => {
                buffer[count] = byte;
                count += 1;
            }
            None => break,
        }
    }
    count
}
