/*
 * Archon uçbirim penceresi: /dev/fb0 üzerine çizen, /dev/kbd'den tuş okuyan
 * ve içinde gerçek bir /bin/sh çalıştıran kullanıcı alanı programı.
 *
 * Kabuk borularla bağlanır: stdin bir boru, stdout/stderr aynı boru.
 */

#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include "font8x16.h"

#define GLYPH_WIDTH 8
#define GLYPH_HEIGHT 16

#define FBIOGET_VSCREENINFO 0x4600
#define FBIOGET_FSCREENINFO 0x4602

struct fb_bitfield {
    uint32_t offset;
    uint32_t length;
    uint32_t msb_right;
};

struct fb_var_screeninfo {
    uint32_t xres, yres, xres_virtual, yres_virtual;
    uint32_t xoffset, yoffset;
    uint32_t bits_per_pixel, grayscale;
    struct fb_bitfield red, green, blue, transp;
    uint32_t nonstd, activate;
    uint32_t height, width;
    uint32_t accel_flags;
    uint32_t pixclock, left_margin, right_margin, upper_margin, lower_margin;
    uint32_t hsync_len, vsync_len, sync, vmode, rotate;
    uint32_t colorspace;
    uint32_t reserved[4];
};

struct fb_fix_screeninfo {
    char id[16];
    unsigned long smem_start;
    uint32_t smem_len;
    uint32_t type, type_aux;
    uint32_t visual;
    uint16_t xpanstep, ypanstep, ywrapstep;
    uint32_t line_length;
    unsigned long mmio_start;
    uint32_t mmio_len;
    uint32_t accel;
    uint16_t capabilities;
    uint16_t reserved[2];
};

static uint8_t *framebuffer;
static int framebuffer_fd = -1;
static uint32_t screen_width, screen_height, line_length;

/* Pencere geometrisi */
static int32_t win_x, win_y, win_w, win_h;
static int32_t text_x, text_y, text_cols, text_rows;

/* Metin tamponu */
#define MAX_LINES 400
#define MAX_COLUMNS 240
static char lines[MAX_LINES][MAX_COLUMNS];
static int line_count;
static int cursor_column;

static const uint32_t COLOR_DESKTOP_TOP = 0x00232a3a;
static const uint32_t COLOR_DESKTOP_BOTTOM = 0x00101419;
static const uint32_t COLOR_WINDOW = 0x000f1419;
static const uint32_t COLOR_TITLE = 0x002b6cb0;
static const uint32_t COLOR_TITLE_TEXT = 0x00f0f4f8;
static const uint32_t COLOR_TEXT = 0x00d8e0e8;
static const uint32_t COLOR_ACCENT = 0x0063b3ed;
static const uint32_t COLOR_BORDER = 0x003a4a5e;

static void put_pixel(uint32_t x, uint32_t y, uint32_t color) {
    if (x >= screen_width || y >= screen_height) {
        return;
    }
    *(uint32_t *)(framebuffer + (size_t)y * line_length + (size_t)x * 4) = color;
}

static void fill_rect(int32_t x, int32_t y, int32_t width, int32_t height, uint32_t color) {
    if (width <= 0 || height <= 0) {
        return;
    }
    int32_t x_end = x + width;
    int32_t y_end = y + height;
    if (x < 0) {
        x = 0;
    }
    if (y < 0) {
        y = 0;
    }
    if (x_end > (int32_t)screen_width) {
        x_end = screen_width;
    }
    if (y_end > (int32_t)screen_height) {
        y_end = screen_height;
    }
    for (int32_t row = y; row < y_end; row++) {
        uint32_t *target = (uint32_t *)(framebuffer + (size_t)row * line_length + (size_t)x * 4);
        for (int32_t column = x; column < x_end; column++) {
            *target++ = color;
        }
    }
}

static void draw_glyph(int32_t x, int32_t y, unsigned char code, uint32_t color) {
    if (code < 32 || code > 126) {
        code = '?';
    }
    const unsigned char *glyph = FONT8X16[code - 32];
    for (int row = 0; row < GLYPH_HEIGHT; row++) {
        unsigned char bits = glyph[row];
        if (!bits) {
            continue;
        }
        for (int column = 0; column < GLYPH_WIDTH; column++) {
            if (bits & (1u << column)) {
                put_pixel(x + column, y + row, color);
            }
        }
    }
}

static void draw_text(int32_t x, int32_t y, const char *text, uint32_t color) {
    for (int32_t index = 0; text[index]; index++) {
        draw_glyph(x + index * GLYPH_WIDTH, y, (unsigned char)text[index], color);
    }
}

static void draw_desktop(void) {
    for (uint32_t row = 0; row < screen_height; row++) {
        uint32_t blend = row * 255 / (screen_height ? screen_height : 1);
        uint32_t red = ((COLOR_DESKTOP_TOP >> 16 & 0xff) * (255 - blend) +
                        (COLOR_DESKTOP_BOTTOM >> 16 & 0xff) * blend) / 255;
        uint32_t green = ((COLOR_DESKTOP_TOP >> 8 & 0xff) * (255 - blend) +
                          (COLOR_DESKTOP_BOTTOM >> 8 & 0xff) * blend) / 255;
        uint32_t blue = ((COLOR_DESKTOP_TOP & 0xff) * (255 - blend) +
                         (COLOR_DESKTOP_BOTTOM & 0xff) * blend) / 255;
        uint32_t color = (red << 16) | (green << 8) | blue;
        uint32_t *target = (uint32_t *)(framebuffer + (size_t)row * line_length);
        for (uint32_t column = 0; column < screen_width; column++) {
            target[column] = color;
        }
    }
}

static void draw_window_chrome(void) {
    fill_rect(win_x - 1, win_y - 1, win_w + 2, win_h + 2, COLOR_BORDER);
    fill_rect(win_x, win_y, win_w, win_h, COLOR_WINDOW);
    fill_rect(win_x, win_y, win_w, 32, COLOR_TITLE);
    draw_text(win_x + 12, win_y + 8, "Archon Terminal", COLOR_TITLE_TEXT);

    /* Sahte kapatma düğmesi */
    int32_t close_x = win_x + win_w - 26;
    fill_rect(close_x, win_y + 9, 14, 14, 0x00c0564a);
    draw_text(close_x + 4, win_y + 8, "x", COLOR_TITLE_TEXT);

    /* Alt bilgi çubuğu */
    int32_t bar_y = win_y + win_h - 20;
    fill_rect(win_x, bar_y, win_w, 20, 0x0018212b);
    draw_text(win_x + 10, bar_y + 2, "ctrl-c gonderir  |  enter calistirir", 0x007a8a9a);
}

/* Metin alanının tamamını yeniden çizer. */
/* Ekranı devralan süreç (ör. tam ekran SDL uygulaması). 0 = serbest. */
static int screen_owner(void) {
    int owner = 0;
    if (framebuffer_fd < 0 || ioctl(framebuffer_fd, 0xF100, &owner) < 0) {
        return 0;
    }
    return owner;
}

static void redraw_text(void) {
    fill_rect(text_x - 8, text_y - 8, text_cols * GLYPH_WIDTH + 16,
              text_rows * GLYPH_HEIGHT + 16, COLOR_WINDOW);

    int first = line_count > text_rows ? line_count - text_rows : 0;
    for (int row = 0; row < text_rows; row++) {
        int index = first + row;
        if (index >= line_count) {
            break;
        }
        draw_text(text_x, text_y + row * GLYPH_HEIGHT, lines[index], COLOR_TEXT);
    }

    /* İmleç */
    int row = line_count - first;
    if (row < 0) {
        row = 0;
    }
    if (row >= text_rows) {
        row = text_rows - 1;
    }
    int column = cursor_column;
    if (column >= text_cols) {
        column = text_cols - 1;
    }
    fill_rect(text_x + column * GLYPH_WIDTH, text_y + row * GLYPH_HEIGHT + GLYPH_HEIGHT - 2,
              GLYPH_WIDTH, 2, COLOR_ACCENT);
}

static void new_line(void) {
    if (line_count < MAX_LINES) {
        line_count++;
    } else {
        memmove(lines[0], lines[1], sizeof(lines[0]) * (MAX_LINES - 1));
    }
    lines[line_count - 1][0] = 0;
    cursor_column = 0;
}

static void append_byte(char byte) {
    if (byte == '\r') {
        return;
    }
    if (byte == '\n') {
        new_line();
        return;
    }
    if (byte == '\b') {
        if (cursor_column > 0) {
            cursor_column--;
            lines[line_count - 1][cursor_column] = 0;
        }
        return;
    }
    if (byte == '\t') {
        for (int index = 0; index < 4; index++) {
            append_byte(' ');
        }
        return;
    }
    if (cursor_column >= text_cols || cursor_column >= MAX_COLUMNS - 1) {
        new_line();
    }
    lines[line_count - 1][cursor_column++] = byte;
    lines[line_count - 1][cursor_column] = 0;
}

static void append_text(const char *text, int length) {
    for (int index = 0; index < length; index++) {
        append_byte(text[index]);
    }
}

static void write_all(int descriptor, const char *data, int length) {
    int written = 0;
    while (written < length) {
        long count = write(descriptor, data + written, length - written);
        if (count <= 0) {
            return;
        }
        written += (int)count;
    }
}

static void run_shell(int master) {
    pid_t child = fork();
    if (child == 0) {
        setsid();
        int slave = open("/dev/pts/0", O_RDWR);
        if (slave < 0) {
            _exit(127);
        }
        dup2(slave, 0);
        dup2(slave, 1);
        dup2(slave, 2);
        if (slave > 2) {
            close(slave);
        }
        close(master);

        char *argv[] = {"/bin/sh", "-i", 0};
        char *envp[] = {
            "PATH=/bin:/usr/bin:/sbin:/usr/sbin",
            "HOME=/",
            "TERM=xterm",
            "PS1=archon\\$ ",
            0,
        };
        execve("/bin/sh", argv, envp);
        _exit(127);
    }
}

int main(void) {
    int fb = open("/dev/fb0", O_RDWR);
    framebuffer_fd = fb;
    if (fb < 0) {
        write(1, "gui: /dev/fb0 yok\n", 18);
        return 1;
    }

    struct fb_var_screeninfo variable;
    memset(&variable, 0, sizeof(variable));
    if (ioctl(fb, FBIOGET_VSCREENINFO, &variable) < 0) {
        write(1, "gui: ekran bilgisi alinamadi\n", 29);
        return 1;
    }

    struct fb_fix_screeninfo fixed;
    memset(&fixed, 0, sizeof(fixed));
    if (ioctl(fb, FBIOGET_FSCREENINFO, &fixed) < 0) {
        write(1, "gui: ekran duzeni alinamadi\n", 28);
        return 1;
    }

    screen_width = variable.xres;
    screen_height = variable.yres;
    line_length = fixed.line_length ? fixed.line_length : screen_width * 4;

    size_t framebuffer_size = (size_t)line_length * screen_height;
    framebuffer = mmap(0, framebuffer_size, PROT_READ | PROT_WRITE, MAP_SHARED, fb, 0);
    if ((long)framebuffer < 0 || framebuffer == 0) {
        write(1, "gui: framebuffer eslenemedi\n", 27);
        return 1;
    }

    /* Pencere: ekranın ortasında, kenarlardan boşluk bırakarak. */
    win_w = screen_width * 9 / 10;
    win_h = screen_height * 8 / 10;
    win_x = (screen_width - win_w) / 2;
    win_y = (screen_height - win_h) / 2;

    text_x = win_x + 14;
    text_y = win_y + 44;
    text_cols = (win_w - 28) / GLYPH_WIDTH;
    text_rows = (win_h - 44 - 28) / GLYPH_HEIGHT;

    line_count = 1;
    lines[0][0] = 0;
    cursor_column = 0;

    draw_desktop();
    draw_window_chrome();

    const char *welcome = "Archon 0.1 - gercek kullanici alani uzerinde calisan kabuk\n";
    append_text(welcome, (int)strlen(welcome));
    redraw_text();

    int master = open("/dev/ptmx", O_RDWR);
    if (master < 0) {
        write(1, "gui: pty acilamadi\n", 19);
        return 1;
    }

    run_shell(master);

    int keyboard = open("/dev/kbd", O_RDONLY);
    if (keyboard < 0) {
        write(1, "gui: klavye yok\n", 16);
        return 1;
    }

    int suspended = 0;

    for (;;) {
        int owner = screen_owner();
        int busy = owner > 0 && owner != getpid();
        if (busy != suspended) {
            suspended = busy;
            if (!suspended) {
                /* Uygulama bitti: ekranı geri al ve tamamen yeniden çiz. */
                draw_desktop();
                draw_window_chrome();
                redraw_text();
            }
        }

        struct pollfd descriptors[2];
        descriptors[0].fd = keyboard;
        descriptors[0].events = POLLIN;
        descriptors[0].revents = 0;
        descriptors[1].fd = master;
        descriptors[1].events = POLLIN;
        descriptors[1].revents = 0;

        int ready = poll(descriptors, 2, 100);
        if (ready <= 0) {
            continue;
        }

        if (descriptors[0].revents & POLLIN) {
            char key;
            if (read(keyboard, &key, 1) == 1) {
                /* Uçbirim satır disiplini yankıyı kendisi yapar. */
                char send = key == '\n' ? '\r' : key;
                write_all(master, &send, 1);
            }
        }

        if (descriptors[1].revents & POLLIN) {
            char buffer[512];
            long count = read(master, buffer, sizeof(buffer));
            if (count > 0) {
                append_text(buffer, (int)count);
                if (!suspended) {
                    redraw_text();
                }
            } else if (count == 0) {
                append_text("\n[kabuk kapandi]\n", 18);
                if (!suspended) {
                    redraw_text();
                }
                break;
            }
        }
    }

    return 0;
}
