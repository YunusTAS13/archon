//! 8254 PIT, 100 Hz sistem tiki.

use core::sync::atomic::{AtomicU64, Ordering};

use super::port::outb;

const PIT_CHANNEL0: u16 = 0x40;
const PIT_COMMAND: u16 = 0x43;
const PIT_BASE_FREQUENCY: u64 = 1_193_182;

static TICKS: AtomicU64 = AtomicU64::new(0);
static FREQUENCY: AtomicU64 = AtomicU64::new(0);

pub fn init(hz: u64) {
    let divisor = PIT_BASE_FREQUENCY / hz;
    debug_assert!(divisor > 0 && divisor <= 0xFFFF);

    unsafe {
        // Kanal 0, lobyte/hibyte, mod 2 (rate generator).
        outb(PIT_COMMAND, 0x34);
        outb(PIT_CHANNEL0, (divisor & 0xFF) as u8);
        outb(PIT_CHANNEL0, ((divisor >> 8) & 0xFF) as u8);
    }

    FREQUENCY.store(hz, Ordering::Relaxed);
}

pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn frequency() -> u64 {
    FREQUENCY.load(Ordering::Relaxed)
}

/// TSC tabanlı ince zaman için ham sayaç; kalibrasyon `time.rs`'de yapılır.
pub fn coarse_nanos() -> u64 {
    let frequency = frequency().max(1);
    let ticks = ticks();
    (ticks / frequency)
        .saturating_mul(1_000_000_000)
        .saturating_add((ticks % frequency).saturating_mul(1_000_000_000) / frequency)
}
