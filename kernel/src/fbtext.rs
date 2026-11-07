//! Framebuffer metin konsolu.
//!
//! `console` yalnızca piksel/dikdörtgen ilkelleri sunar; metin için
//! `font.rs` içindeki 8x16 glif tablosunu kullanır. Çekirdek log'u hem seri
//! porta hem buraya yazılır.

use crate::console::{self, COLOR_BACKGROUND, COLOR_TEXT};
use crate::font::{FIRST_CHAR, FONT, GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::sync::SpinLock;

const LAST_CHAR: u8 = 126;

struct TextConsole {
    cols: u64,
    rows: u64,
    column: u64,
    row: u64,
    enabled: bool,
    frozen: bool,
    foreground: u32,
    background: u32,
    /// Görünüm alanının sol üst köşesi (piksel).
    origin_x: u64,
    origin_y: u64,
    /// Görünüm alanının boyutu (piksel).
    view_width: u64,
    view_height: u64,
}

static TEXT: SpinLock<TextConsole> = SpinLock::new(TextConsole {
    cols: 0,
    rows: 0,
    column: 0,
    row: 0,
    enabled: false,
    frozen: false,
    foreground: COLOR_TEXT,
    background: COLOR_BACKGROUND,
    origin_x: 0,
    origin_y: 0,
    view_width: 0,
    view_height: 0,
});

/// Framebuffer'ı metin alanı olarak hazırlar.
pub fn init() {
    let Some(format) = console::format() else {
        return;
    };

    let mut text = TEXT.lock();
    text.cols = format.width / GLYPH_WIDTH as u64;
    text.rows = format.height / GLYPH_HEIGHT as u64;
    text.column = 0;
    text.row = 0;
    text.enabled = true;
    text.origin_x = 0;
    text.origin_y = 0;
    text.view_width = format.width;
    text.view_height = format.height;
}

/// Metin alanını ekranın bir dikdörtgenine taşır (pencere içi konsol).
pub fn set_viewport(x: u64, y: u64, width: u64, height: u64) {
    let mut text = TEXT.lock();
    text.origin_x = x;
    text.origin_y = y;
    text.view_width = width;
    text.view_height = height;
    text.cols = width / GLYPH_WIDTH as u64;
    text.rows = height / GLYPH_HEIGHT as u64;
    text.column = 0;
    text.row = 0;
}

/// Yalnızca görünüm alanını temizler.
pub fn clear_viewport() {
    let (x, y, width, height, background) = {
        let text = TEXT.lock();
        (
            text.origin_x,
            text.origin_y,
            text.view_width,
            text.view_height,
            text.background,
        )
    };
    console::fill_rect(x, y, width, height, background);
    let mut text = TEXT.lock();
    text.column = 0;
    text.row = 0;
}

/// Ekranı yeniden çizim için açar (`freeze` sonrası GUI devralır).
pub fn thaw() {
    TEXT.lock().frozen = false;
}

pub fn clear() {
    let background = TEXT.lock().background;
    console::clear(background);
    let mut text = TEXT.lock();
    text.column = 0;
    text.row = 0;
}

/// Bundan sonraki log'lar yalnızca seri porta gider; ekran sabit kalır.
pub fn freeze() {
    TEXT.lock().frozen = true;
}

pub fn set_foreground(color: u32) {
    TEXT.lock().foreground = color;
}

/// Metni verilen sütun/satıra yazar ve imleci değiştirmez.
pub fn write_at(column: u64, row: u64, text: &str) {
    let (saved_column, saved_row) = {
        let mut state = TEXT.lock();
        let saved = (state.column, state.row);
        state.column = column;
        state.row = row;
        saved
    };

    write_str(text);

    let mut state = TEXT.lock();
    state.column = saved_column;
    state.row = saved_row;
}

pub fn write_str(text: &str) {
    {
        let state = TEXT.lock();
        if !state.enabled || state.frozen {
            return;
        }
    }

    for byte in text.bytes() {
        put_byte(byte);
    }
}

/// Görünüm alanından bağımsız, piksel konumuna metin çizer (pencere/başlık).
pub fn draw_text(x: u64, y: u64, text: &str, color: u32) {
    let mut cursor = x;
    for byte in text.bytes() {
        let code = if (FIRST_CHAR..=LAST_CHAR).contains(&byte) {
            (byte - FIRST_CHAR) as usize
        } else {
            (b'?' - FIRST_CHAR) as usize
        };
        for (line, bits) in FONT[code].iter().enumerate() {
            for pixel in 0..GLYPH_WIDTH {
                if bits & (1 << pixel) != 0 {
                    console::put_pixel(cursor + pixel as u64, y + line as u64, color);
                }
            }
        }
        cursor += GLYPH_WIDTH as u64;
    }
}

fn put_byte(byte: u8) {
    match byte {
        8 => {
            // Geri silme: bir glif geri git ve arka planla boya.
            let (column, row, background) = {
                let mut state = TEXT.lock();
                if state.column == 0 {
                    return;
                }
                state.column -= 1;
                (state.column, state.row, state.background)
            };
            let (origin_x, origin_y) = {
                let state = TEXT.lock();
                (
                    state.origin_x + column * GLYPH_WIDTH as u64,
                    state.origin_y + row * GLYPH_HEIGHT as u64,
                )
            };
            console::fill_rect(
                origin_x,
                origin_y,
                GLYPH_WIDTH as u64,
                GLYPH_HEIGHT as u64,
                background,
            );
        }
        b'\n' => {
            newline();
        }
        b'\r' => {
            TEXT.lock().column = 0;
        }
        b'\t' => {
            for _ in 0..4 {
                put_char(b' ');
            }
        }
        0x20..=0x7E => put_char(byte),
        _ => put_char(b'?'),
    }
}

fn put_char(byte: u8) {
    let (column, row, foreground) = {
        let mut state = TEXT.lock();
        if state.column >= state.cols {
            state.column = 0;
            state.row += 1;
        }
        if state.row >= state.rows {
            drop(state);
            scroll();
            return put_char(byte);
        }
        let cursor = (state.column, state.row, state.foreground);
        state.column += 1;
        cursor
    };

    let code = if (FIRST_CHAR..=LAST_CHAR).contains(&byte) {
        (byte - FIRST_CHAR) as usize
    } else {
        (b'?' - FIRST_CHAR) as usize
    };

    let (origin_x, origin_y) = {
        let state = TEXT.lock();
        (
            state.origin_x + column * GLYPH_WIDTH as u64,
            state.origin_y + row * GLYPH_HEIGHT as u64,
        )
    };

    for (line, bits) in FONT[code].iter().enumerate() {
        for pixel in 0..GLYPH_WIDTH {
            if bits & (1 << pixel) != 0 {
                console::put_pixel(
                    origin_x + pixel as u64,
                    origin_y + line as u64,
                    foreground,
                );
            }
        }
    }
}

fn newline() {
    let (rows, row) = {
        let mut state = TEXT.lock();
        state.column = 0;
        state.row += 1;
        (state.rows, state.row)
    };
    if row >= rows {
        scroll();
    }
}

fn scroll() {
    let Some(format) = console::format() else {
        return;
    };
    let (origin_x, origin_y, view_width, view_height, rows, background) = {
        let state = TEXT.lock();
        (
            state.origin_x,
            state.origin_y,
            state.view_width,
            state.view_height,
            state.rows,
            state.background,
        )
    };

    // Görünüm alanını satır satır yukarı kaydır (pitch'e saygılı).
    let row_bytes = (view_width * format.bytes_per_pixel) as usize;
    let line_bytes = format.pitch as usize;
    unsafe {
        let base = format.address as *mut u8;
        for row in 1..rows {
            let source = base
                .add(((origin_y + row * GLYPH_HEIGHT as u64) * format.pitch) as usize)
                .add((origin_x * format.bytes_per_pixel) as usize);
            let destination = base
                .add(((origin_y + (row - 1) * GLYPH_HEIGHT as u64) * format.pitch) as usize)
                .add((origin_x * format.bytes_per_pixel) as usize);
            core::ptr::copy_nonoverlapping(source, destination, row_bytes.min(line_bytes));
        }
    }
    let last_row = origin_y + view_height.saturating_sub(GLYPH_HEIGHT as u64);
    console::fill_rect(origin_x, last_row, view_width, GLYPH_HEIGHT as u64, background);

    let mut state = TEXT.lock();
    state.row = state.rows.saturating_sub(1);
    state.column = 0;
}
