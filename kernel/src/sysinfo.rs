//! Açılış sonunda framebuffer'a basılan sistem kartı ("fastfetch").

use alloc::format;

use crate::console::{COLOR_ACCENT, COLOR_TEXT};
use crate::fbtext;

const LOGO: [&str; 7] = [
    "        ##        ",
    "       ####       ",
    "      ##  ##      ",
    "     ########     ",
    "    ##      ##    ",
    "   ##        ##   ",
    "  ##          ##  ",
];

const LEFT_COLUMN: u64 = 4;
const INFO_COLUMN: u64 = 26;
const LABEL_WIDTH: u64 = 11;

fn field(row: &mut u64, label: &str, value: &str) {
    fbtext::set_foreground(COLOR_ACCENT);
    fbtext::write_at(INFO_COLUMN, *row, label);
    fbtext::set_foreground(COLOR_TEXT);
    fbtext::write_at(INFO_COLUMN + LABEL_WIDTH, *row, value);
    *row += 1;
}

pub fn print() {
    let info = crate::boot::info();
    let top = 7;

    fbtext::clear();

    for (index, line) in LOGO.iter().enumerate() {
        fbtext::set_foreground(COLOR_ACCENT);
        fbtext::write_at(LEFT_COLUMN, top + index as u64, line);
    }

    let mut row = top;

    fbtext::set_foreground(COLOR_ACCENT);
    fbtext::write_at(INFO_COLUMN, row, "archon@localhost");
    row += 1;
    fbtext::write_at(INFO_COLUMN, row, "-------------------");
    row += 1;

    let memory = crate::mem::pmm::total_frames() as u64 * 4096 / (1024 * 1024);
    let heap = crate::mem::heap::size() as u64 / (1024 * 1024);
    let cpu_mhz = crate::time::tsc_frequency() / 1_000_000;

    field(&mut row, "OS", "Archon 0.1.0 x86_64");
    field(&mut row, "Kernel", "Rust no_std, Limine");
    field(
        &mut row,
        "CPU",
        &format!("{} @ {} MHz", crate::arch::cpu::brand_string(), cpu_mhz),
    );
    field(&mut row, "Memory", &format!("{} MiB usable", memory));
    field(&mut row, "Heap", &format!("{} MiB arena", heap));

    let disk = match crate::drivers::virtio_blk::capacity_mib() {
        Some(mib) => format!("{} MiB virtio-blk (ext4)", mib),
        None => "yok (initramfs)".into(),
    };
    field(&mut row, "Disk", &disk);

    let root = crate::fs::root()
        .map(|filesystem| filesystem.name())
        .unwrap_or("yok");
    field(&mut row, "Root", &format!("{} (read-only)", root));

    field(
        &mut row,
        "Display",
        &format!(
            "{}x{} @32bpp",
            info.framebuffer_width, info.framebuffer_height
        ),
    );
    field(&mut row, "Uptime", &format!("{} ms", crate::time::uptime_ms()));
    field(&mut row, "Tasks", &format!("{}", crate::task::process_count()));
    field(&mut row, "Syscalls", &format!("{}", crate::syscall::count()));

    let footer = top + row_delta(top, row) + 1;
    fbtext::set_foreground(COLOR_ACCENT);
    fbtext::write_at(
        LEFT_COLUMN,
        footer,
        "M4: Limine -> kernel -> ext4 -> ring3",
    );

    // Kart ekranda kalsın; sonraki log'lar yalnızca seri porta gitsin.
    fbtext::freeze();
}

fn row_delta(from: u64, to: u64) -> u64 {
    to.saturating_sub(from)
}
