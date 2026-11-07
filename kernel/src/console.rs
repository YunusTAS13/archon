//! Framebuffer konsolu.
//!
//! Şu an yalnızca çizim ilkelleri var: temizleme, dikdörtgen, piksel ve
//! ilerleme çubuğu. Metin için 8x16 PSF fontu gerekiyor; `tools/fetch_font.py`
//! M1'de eklenecek ve `font.rs` üretilecek. O zamana kadar log seri porttan
//! gider, ekranda yalnızca boot durumu görünür.

use crate::boot;
use crate::sync::SpinLock;

#[derive(Clone, Copy)]
pub struct PixelFormat {
    pub address: u64,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bytes_per_pixel: u64,
}

struct ConsoleState {
    format: Option<PixelFormat>,
    cursor_line: u64,
}

static CONSOLE: SpinLock<ConsoleState> = SpinLock::new(ConsoleState {
    format: None,
    cursor_line: 0,
});

/// Ekranı en son eşleyen süreç. `0` = ekran serbest.
static FB_OWNER: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);

pub const FB_OWNER_IOCTL: u64 = 0xF100;

pub fn acquire_framebuffer(pid: i32) {
    FB_OWNER.store(pid, core::sync::atomic::Ordering::Release);
}

pub fn release_framebuffer(pid: i32) {
    let _ = FB_OWNER.compare_exchange(
        pid,
        0,
        core::sync::atomic::Ordering::AcqRel,
        core::sync::atomic::Ordering::Acquire,
    );
}

pub fn framebuffer_owner() -> i32 {
    FB_OWNER.load(core::sync::atomic::Ordering::Acquire)
}

pub const COLOR_BACKGROUND: u32 = 0x0010_1820;
pub const COLOR_ACCENT: u32 = 0x0040_C0A0;
pub const COLOR_TEXT: u32 = 0x00E0_E8F0;

pub fn init() {
    let info = boot::info();
    if info.framebuffer_address == 0 {
        println!("console: framebuffer yok, yalnizca seri");
        return;
    }

    let format = PixelFormat {
        address: info.framebuffer_address,
        width: info.framebuffer_width,
        height: info.framebuffer_height,
        pitch: info.framebuffer_pitch,
        // Limine bpp'yi bildiriyor; HHDM üzerinden erişilen adres zaten
        // çekirdek sanal adresi olduğu için burada yalnızca genişlik önemli.
        bytes_per_pixel: 4,
    };

    {
        let mut console = CONSOLE.lock();
        console.format = Some(format);
    }

    clear(COLOR_BACKGROUND);
    fill_rect(0, 0, format.width, 48, COLOR_ACCENT);
    println!(
        "console: framebuffer {}x{} hazir",
        format.width, format.height
    );
}

pub fn format() -> Option<PixelFormat> {
    CONSOLE.lock().format
}

pub fn put_pixel(x: u64, y: u64, color: u32) {
    let console = CONSOLE.lock();
    let Some(format) = console.format else {
        return;
    };
    if x >= format.width || y >= format.height {
        return;
    }
    let offset = y * format.pitch + x * format.bytes_per_pixel;
    let pointer = (format.address + offset) as *mut u32;
    unsafe { core::ptr::write_volatile(pointer, color) };
}

pub fn fill_rect(x: u64, y: u64, width: u64, height: u64, color: u32) {
    let console = CONSOLE.lock();
    let Some(format) = console.format else {
        return;
    };
    let x_end = (x + width).min(format.width);
    let y_end = (y + height).min(format.height);

    for row in y..y_end {
        let base = format.address + row * format.pitch + x * format.bytes_per_pixel;
        let pointer = base as *mut u32;
        for column in x..x_end {
            unsafe {
                core::ptr::write_volatile(
                    pointer.add((column - x) as usize),
                    color,
                );
            }
        }
    }
}

pub fn clear(color: u32) {
    let console = CONSOLE.lock();
    let Some(format) = console.format else {
        return;
    };
    let pixels = format.pitch / format.bytes_per_pixel;
    let pointer = format.address as *mut u32;
    for row in 0..format.height {
        for column in 0..pixels {
            unsafe {
                core::ptr::write_volatile(pointer.add((row * pixels + column) as usize), color);
            }
        }
    }
}

/// Boot ilerlemesi için basit gösterge. Metin fontu gelene kadar tek geri
/// bildirim bu.
pub fn progress(fraction: f32) {
    let Some(format) = format() else {
        return;
    };
    let y = format.height - 64;
    let full = format.width - 64;
    let filled = (full as f32 * fraction.clamp(0.0, 1.0)) as u64;

    fill_rect(32, y, full, 24, 0x0020_2830);
    fill_rect(32, y, filled, 24, COLOR_ACCENT);
}

/// Framebuffer'ın fiziksel aralığı: `mmap("/dev/fb0")` bunu doğrudan eşler.
pub fn framebuffer_region() -> Option<(u64, u64)> {
    let console = CONSOLE.lock();
    let format = console.format?;
    let hhdm = crate::boot::info().hhdm_offset;
    let physical = format.address.checked_sub(hhdm)?;
    Some((physical, format.pitch * format.height))
}

/// `/dev/fb0` için Linux fbdev uyumlu `ioctl`'ler.
pub fn framebuffer_ioctl(request: u64, argument: u64) -> Option<i64> {
    use crate::syscall::copy_to_user;

    let console = CONSOLE.lock();
    let format = console.format?;
    drop(console);

    match request {
        FB_OWNER_IOCTL => {
            // Sahip ölmüşse ekran yeniden serbest sayılır.
            let mut owner = framebuffer_owner();
            if owner > 0 && !crate::task::alive(owner as u64) {
                release_framebuffer(owner);
                owner = 0;
            }
            match copy_to_user(argument, &owner.to_le_bytes()) {
                Ok(()) => Some(0),
                Err(code) => Some(code),
            }
        }
        // FBIOGET_VSCREENINFO
        0x4600 => {
            let mut info = [0u8; 160];
            info[0..4].copy_from_slice(&(format.width as u32).to_le_bytes());
            info[4..8].copy_from_slice(&(format.height as u32).to_le_bytes());
            info[8..12].copy_from_slice(&(format.width as u32).to_le_bytes());
            info[12..16].copy_from_slice(&(format.height as u32).to_le_bytes());
            info[24..28].copy_from_slice(&32u32.to_le_bytes()); // bits_per_pixel
            // red/green/blue/transp bit alanları: 16/8/0/24 ofset, 8 bit.
            for (index, (offset, length)) in [(16u32, 8u32), (8, 8), (0, 8), (24, 8)]
                .iter()
                .enumerate()
            {
                let base = 32 + index * 12;
                info[base..base + 4].copy_from_slice(&offset.to_le_bytes());
                info[base + 4..base + 8].copy_from_slice(&length.to_le_bytes());
            }
            info[88..92].copy_from_slice(&(format.height as u32).to_le_bytes());
            info[92..96].copy_from_slice(&(format.width as u32).to_le_bytes());
            match copy_to_user(argument, &info) {
                Ok(()) => Some(0),
                Err(code) => Some(code),
            }
        }
        // FBIOGET_FSCREENINFO
        0x4602 => {
            let mut info = [0u8; 80];
            info[0..7].copy_from_slice(b"archon0");
            let (physical, length) = framebuffer_region()?;
            info[16..24].copy_from_slice(&physical.to_le_bytes()); // smem_start (unsigned long)
            info[24..28].copy_from_slice(&(length as u32).to_le_bytes());
            info[28..32].copy_from_slice(&0u32.to_le_bytes()); // type: PACKED_PIXELS
            info[36..40].copy_from_slice(&2u32.to_le_bytes()); // visual: TRUECOLOR
            info[48..52].copy_from_slice(&(format.pitch as u32).to_le_bytes());
            match copy_to_user(argument, &info) {
                Ok(()) => Some(0),
                Err(code) => Some(code),
            }
        }
        _ => None,
    }
}
