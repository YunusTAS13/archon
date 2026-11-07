//! Çekirdek log'u. Şimdilik yalnızca seri port; M6'da framebuffer konsolu
//! eklenecek ve ikisi birlikte yazacak.

use core::fmt::{self, Write};

use crate::arch::serial::Serial;
use crate::sync::SpinLock;

pub const LEVEL_ERROR: u8 = 0;
pub const LEVEL_WARN: u8 = 1;
pub const LEVEL_INFO: u8 = 2;
pub const LEVEL_DEBUG: u8 = 3;

static LOG_LEVEL: SpinLock<u8> = SpinLock::new(LEVEL_INFO);
static LOG_LOCK: SpinLock<()> = SpinLock::new(());

pub fn set_level(level: u8) {
    *LOG_LEVEL.lock() = level;
}

pub fn level() -> u8 {
    *LOG_LEVEL.lock()
}

struct LogWriter;

impl Write for LogWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        Serial::write_bytes(text.as_bytes());
        crate::fbtext::write_str(text);
        Ok(())
    }
}

/// Ham baytları seri porta ve framebuffer konsoluna yazar.
pub fn write_bytes(bytes: &[u8]) {
    Serial::write_bytes(bytes);
    let text = alloc::string::String::from_utf8_lossy(bytes);
    crate::fbtext::write_str(&text);
}

pub fn write(args: fmt::Arguments<'_>) {
    let _guard = LOG_LOCK.lock();
    let mut writer = LogWriter;
    let _ = writer.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::log::write(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {{
        $crate::print!("{}\n", format_args!($($arg)*));
    }};
}
