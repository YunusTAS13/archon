/*
 * Archon SDL2 demosu.
 *
 * SDL2'nin "evdev" video surucusu sahte bir video arka ucu ile birlikte
 * gercek /dev/input/event* aygitlarini okur. Bu ornek, cekirdegin
 * framebuffer'ini (/dev/fb0) dogrudan kullanir ve tus olaylarini SDL'nin
 * kendi olay kuyrugundan alir.
 */
#include <SDL2/SDL.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

struct fb_bitfield {
    uint32_t offset, length, msb_right;
};

struct fb_var_screeninfo {
    uint32_t xres, yres, xres_virtual, yres_virtual;
    uint32_t xoffset, yoffset;
    uint32_t bits_per_pixel, grayscale;
    struct fb_bitfield red, green, blue, transp;
    uint32_t nonstd, activate, height, width, accel_flags;
    uint32_t pixclock, left_margin, right_margin, upper_margin, lower_margin;
    uint32_t hsync_len, vsync_len, sync, vmode, rotate, colorspace;
    uint32_t reserved[4];
};

#define GLYPH_W 8
#define GLYPH_H 16

static SDL_Surface *font;

static void draw_text(SDL_Surface *target, int x, int y, const char *text) {
    if (!font) {
        return;
    }
    for (const unsigned char *cursor = (const unsigned char *)text; *cursor; cursor++) {
        int index = (int)*cursor - 32;
        if (index < 0 || index >= 95) {
            x += GLYPH_W;
            continue;
        }
        SDL_Rect source = {index * GLYPH_W, 0, GLYPH_W, GLYPH_H};
        SDL_Rect destination = {x, y, GLYPH_W, GLYPH_H};
        SDL_BlitSurface(font, &source, target, &destination);
        x += GLYPH_W;
    }
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);

    if (SDL_Init(SDL_INIT_VIDEO) < 0) {
        printf("sdl2-demo: SDL_Init: %s\n", SDL_GetError());
        return 1;
    }
    printf("sdl2-demo: video=%s suruculer=", SDL_GetCurrentVideoDriver());
    for (int index = 0; index < SDL_GetNumVideoDrivers(); index++) {
        printf("%s,", SDL_GetVideoDriver(index));
    }
    printf("\n");

    int fb_fd = open("/dev/fb0", O_RDWR);
    if (fb_fd < 0) {
        printf("sdl2-demo: /dev/fb0 yok\n");
        SDL_Quit();
        return 1;
    }
    struct fb_var_screeninfo info;
    memset(&info, 0, sizeof(info));
    if (ioctl(fb_fd, 0x4600, &info) != 0) {
        printf("sdl2-demo: FBIOGET_VSCREENINFO basarisiz\n");
        close(fb_fd);
        SDL_Quit();
        return 1;
    }
    const int fb_width = (int)info.xres;
    const int fb_height = (int)info.yres;
    const int fb_bytes_per_pixel = info.bits_per_pixel / 8;
    const size_t fb_size = (size_t)fb_width * fb_height * fb_bytes_per_pixel;
    uint8_t *fb_pixels = mmap(NULL, fb_size, PROT_READ | PROT_WRITE, MAP_SHARED, fb_fd, 0);
    if (fb_pixels == MAP_FAILED) {
        printf("sdl2-demo: fb mmap basarisiz\n");
        close(fb_fd);
        SDL_Quit();
        return 1;
    }

    /* Pencere tam ekran: ucbirim arka planda kalmasin. */
    const int width = fb_width;
    const int height = fb_height;
    SDL_Window *window = SDL_CreateWindow("Archon SDL2", SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED,
                                          width, height, 0);
    if (!window) {
        printf("sdl2-demo: pencere: %s\n", SDL_GetError());
        SDL_Quit();
        return 1;
    }
    SDL_Surface *canvas = SDL_GetWindowSurface(window);
    if (!canvas) {
        printf("sdl2-demo: yuzey: %s\n", SDL_GetError());
        SDL_Quit();
        return 1;
    }
    printf("sdl2-demo: fb %dx%d %d bpp, pencere %dx%d %d bpp, maske %08x/%08x/%08x\n", fb_width,
           fb_height, info.bits_per_pixel, canvas->w, canvas->h, canvas->format->BitsPerPixel,
           canvas->format->Rmask, canvas->format->Gmask, canvas->format->Bmask);

    font = SDL_LoadBMP("/usr/share/font8x16.bmp");
    if (font) {
        SDL_SetColorKey(font, SDL_TRUE, SDL_MapRGB(font->format, 255, 0, 255));
    } else {
        printf("sdl2-demo: yazi tipi yuklenemedi: %s\n", SDL_GetError());
    }

    const int offset_x = 0;
    const int offset_y = 0;

    SDL_Rect square = {60, 60, 180, 180};
    int direction_x = 7;
    int direction_y = 5;
    int frames = 0;
    int key_count = 0;
    char last_key[64] = "yok";
    int running = 1;

    while (running && frames < 60 * 60) {
        SDL_Event event;
        while (SDL_PollEvent(&event)) {
            if (event.type == SDL_QUIT) {
                running = 0;
            } else if (event.type == SDL_KEYDOWN) {
                key_count++;
                const char *name = SDL_GetKeyName(event.key.keysym.sym);
                snprintf(last_key, sizeof(last_key), "%s (0x%x)", name ? name : "?",
                         (unsigned)event.key.keysym.sym);
                printf("sdl2-demo: tus %s\n", last_key);
                if (event.key.keysym.sym == SDLK_ESCAPE) {
                    running = 0;
                }
            }
        }

        /* Arka plan: dusey degrade. */
        SDL_LockSurface(canvas);
        int pitch = canvas->pitch / 4;
        uint32_t *pixels = canvas->pixels;
        for (int y = 0; y < height; y++) {
            uint8_t red = (uint8_t)(10 + (y * 40) / height);
            uint8_t green = (uint8_t)(30 + (y * 110) / height);
            uint8_t blue = (uint8_t)(80 + (y * 120) / height);
            uint32_t color = SDL_MapRGB(canvas->format, red, green, blue);
            uint32_t *row = pixels + (size_t)y * pitch;
            for (int x = 0; x < width; x++) {
                row[x] = color;
            }
        }
        SDL_UnlockSurface(canvas);

        square.x += direction_x;
        square.y += direction_y;
        if (square.x < 0 || square.x + square.w > width) {
            direction_x = -direction_x;
            square.x += direction_x;
        }
        if (square.y < 0 || square.y + square.h > height) {
            direction_y = -direction_y;
            square.y += direction_y;
        }
        SDL_FillRect(canvas, &square, SDL_MapRGB(canvas->format, 250, 200, 70));

        SDL_Rect bar = {0, height - 10, (frames * width) / (60 * 60), 10};
        SDL_FillRect(canvas, &bar, SDL_MapRGB(canvas->format, 255, 90, 60));

        char line[96];
        draw_text(canvas, 24, 24, "Archon + SDL2");
        snprintf(line, sizeof(line), "video: %s", SDL_GetCurrentVideoDriver());
        draw_text(canvas, 24, 44, line);
        snprintf(line, sizeof(line), "son tus: %s", last_key);
        draw_text(canvas, 24, 64, line);
        snprintf(line, sizeof(line), "tus sayisi: %d", key_count);
        draw_text(canvas, 24, 84, line);
        draw_text(canvas, 24, 104, "cikis: ESC");

        SDL_UpdateWindowSurface(window);

        SDL_LockSurface(canvas);
        for (int y = 0; y < height; y++) {
            uint8_t *source = (uint8_t *)canvas->pixels + (size_t)y * canvas->pitch;
            uint8_t *destination =
                fb_pixels + ((size_t)(y + offset_y) * fb_width + offset_x) * fb_bytes_per_pixel;
            memcpy(destination, source, (size_t)width * 4);
        }
        SDL_UnlockSurface(canvas);

        frames++;
        SDL_Delay(16);
    }

    printf("sdl2-demo: %d kare, %d tus, TAMAM\n", frames, key_count);
    munmap(fb_pixels, fb_size);
    close(fb_fd);
    if (font) {
        SDL_FreeSurface(font);
    }
    SDL_DestroyWindow(window);
    SDL_Quit();
    return 0;
}
