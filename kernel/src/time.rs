//! Zaman: PIT kaba tik + TSC ince sayaç.
//!
//! TSC frekansı boot sırasında PIT ile ölçülür. Ölçüm 50 ms sürer ve yalnızca
//! bir kez yapılır.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::{cpu, pit};

static TSC_PER_SECOND: AtomicU64 = AtomicU64::new(0);
static BOOT_TSC: AtomicU64 = AtomicU64::new(0);
static BOOT_NANOS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    BOOT_TSC.store(cpu::rdtsc(), Ordering::Relaxed);
    BOOT_NANOS.store(pit::coarse_nanos(), Ordering::Relaxed);

    // 50 ms boyunca PIT tiklerini sayarak TSC frekansını ölç. PIT IRQ0
    // çalışsın diye kesmeler geçici olarak açılır.
    unsafe { crate::arch::cpu::enable_interrupts() };
    let start_ticks = pit::ticks();
    let start_tsc = cpu::rdtsc();
    let target = pit::frequency().max(1) / 20;
    let mut spins = 0u64;

    while pit::ticks().saturating_sub(start_ticks) < target {
        spins += 1;
        if spins > 200_000_000 {
            println!("time: PIT tik gelmiyor, kalibrasyon atlandi");
            return;
        }
        core::hint::spin_loop();
    }

    let elapsed_ticks = pit::ticks().saturating_sub(start_ticks);
    let elapsed_tsc = cpu::rdtsc().saturating_sub(start_tsc);
    if elapsed_ticks > 0 {
        let frequency = elapsed_tsc.saturating_mul(pit::frequency()) / elapsed_ticks;
        TSC_PER_SECOND.store(frequency, Ordering::Relaxed);
        println!("time: TSC {} MHz", frequency / 1_000_000);

        // TSC tabanlı sayacı PIT'e göre hizala.
        BOOT_TSC.store(start_tsc, Ordering::Relaxed);
        BOOT_NANOS.store(
            start_ticks.saturating_mul(1_000_000_000) / pit::frequency().max(1),
            Ordering::Relaxed,
        );
    } else {
        println!("time: TSC kalibrasyonu basarisiz, PIT kullanilacak");
    }
}

pub fn tsc_frequency() -> u64 {
    TSC_PER_SECOND.load(Ordering::Relaxed)
}

pub fn monotonic_nanos() -> u64 {
    let frequency = TSC_PER_SECOND.load(Ordering::Relaxed);
    if frequency == 0 {
        return pit::coarse_nanos();
    }
    let elapsed = cpu::rdtsc().saturating_sub(BOOT_TSC.load(Ordering::Relaxed));
    // `elapsed * 1e9` birkaç saniyede u64'ü taşır; bölmeyi önce yap.
    let seconds = elapsed / frequency;
    let remainder = elapsed % frequency;
    let nanos = seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(remainder.saturating_mul(1_000_000_000) / frequency);
    BOOT_NANOS.load(Ordering::Relaxed) + nanos
}

pub fn uptime_ms() -> u64 {
    monotonic_nanos() / 1_000_000
}

pub fn busy_wait_ms(milliseconds: u64) {
    let deadline = monotonic_nanos().saturating_add(milliseconds * 1_000_000);
    while monotonic_nanos() < deadline {
        core::hint::spin_loop();
    }
}
