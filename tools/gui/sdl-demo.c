/*
 * Archon SDL3 demosu.
 *
 * SDL3'ün Linux "evdev" video sürücüsü /dev/fb0 ve /dev/input/event0 üzerinde
 * çalışır. Kare SDL'nin yazılım render yoluyla çizilir, yazı için SDL'nin
 * gömülü 8x8 hata ayıklama yazı tipi kullanılır ve çekirdek framebuffer'ına
 * aktarılır. Klavye olayları SDL'nin kendi olay kuyruğundan gelir.
 */
#include <SDL3/SDL.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>
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

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);

    const char *requested = getenv("SDL_VIDEODRIVER");
    SDL_SetHint(SDL_HINT_VIDEO_DRIVER, (requested && *requested) ? requested : "offscreen");
    if (!SDL_Init(SDL_INIT_VIDEO)) {
        printf("sdl-demo: SDL_Init: %s\n", SDL_GetError());
        return 1;
    }

    int fb = open("/dev/fb0", O_RDWR);
    if (fb < 0) {
        printf("sdl-demo: /dev/fb0 yok\n");
        SDL_Quit();
        return 1;
    }
    struct fb_var_screeninfo info;
    if (ioctl(fb, 0x4600, &info) != 0) {
        printf("sdl-demo: FBIOGET_VSCREENINFO basarisiz\n");
        close(fb);
        SDL_Quit();
        return 1;
    }
    const int width = (int)info.xres;
    const int height = (int)info.yres;
    size_t screen_bytes = (size_t)width * height * (info.bits_per_pixel / 8);
    uint8_t *screen = mmap(NULL, screen_bytes, PROT_READ | PROT_WRITE, MAP_SHARED, fb, 0);
    if (screen == MAP_FAILED) {
        printf("sdl-demo: fb mmap basarisiz\n");
        close(fb);
        SDL_Quit();
        return 1;
    }

    SDL_Window *window = SDL_CreateWindow("Archon SDL", width, height, 0);
    if (!window) {
        printf("sdl-demo: pencere: %s\n", SDL_GetError());
        SDL_Quit();
        return 1;
    }
    SDL_Surface *target = SDL_GetWindowSurface(window);
    SDL_Surface *canvas = SDL_CreateSurface(width, height, SDL_PIXELFORMAT_XRGB8888);
    if (!target || !canvas) {
        printf("sdl-demo: yuzey: %s\n", SDL_GetError());
        SDL_Quit();
        return 1;
    }
    SDL_Renderer *renderer = SDL_CreateSoftwareRenderer(canvas);
    if (!renderer) {
        printf("sdl-demo: renderer: %s\n", SDL_GetError());
        SDL_Quit();
        return 1;
    }

    printf("sdl-demo: %dx%d %u bpp, video=%s, surucu sayisi=%d\n", width, height,
           info.bits_per_pixel, SDL_GetCurrentVideoDriver(), SDL_GetNumVideoDrivers());

    SDL_Rect square = {40, 40, 160, 160};
    int direction_x = 6;
    int direction_y = 4;
    int frames = 0;
    int keys = 0;
    char last_key[48] = "yok";
    int running = 1;

    while (running && frames < 60 * 60) {
        SDL_Event event;
        while (SDL_PollEvent(&event)) {
            if (event.type == SDL_EVENT_QUIT) {
                running = 0;
            } else if (event.type == SDL_EVENT_KEY_DOWN) {
                keys++;
                const char *name = SDL_GetKeyName(event.key.key);
                snprintf(last_key, sizeof(last_key), "%s (0x%x)", name ? name : "?",
                         (unsigned)event.key.key);
                if (event.key.key == SDLK_ESCAPE) {
                    running = 0;
                }
            }
        }

        /* Arka plan degradesi. */
        SDL_LockSurface(canvas);
        uint32_t *pixels = canvas->pixels;
        int pitch = canvas->pitch / 4;
        for (int y = 0; y < height; y++) {
            uint8_t green = (uint8_t)(40 + (y * 120) / height);
            uint8_t blue = (uint8_t)(90 + (y * 90) / height);
            uint32_t color = 0x00100000u | ((uint32_t)green << 8) | blue;
            uint32_t *row = pixels + y * pitch;
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
        SDL_FillSurfaceRect(canvas, &square, SDL_MapSurfaceRGB(canvas, 240, 200, 60));

        SDL_Rect bar = {0, height - 12, (frames * width) / (60 * 60), 12};
        SDL_FillSurfaceRect(canvas, &bar, SDL_MapSurfaceRGB(canvas, 255, 90, 60));

        /* SDL'nin gömülü hata ayıklama yazı tipi. */
        SDL_SetRenderDrawColor(renderer, 255, 255, 255, 255);
        SDL_RenderDebugText(renderer, 16, 16, "Archon + SDL3");
        SDL_RenderDebugTextFormat(renderer, 16, 32, "video: %s", SDL_GetCurrentVideoDriver());
        SDL_RenderDebugTextFormat(renderer, 16, 48, "son tus: %s", last_key);
        SDL_RenderDebugTextFormat(renderer, 16, 64, "tus sayisi: %d", keys);
        SDL_RenderDebugText(renderer, 16, 80, "cikis: ESC");

        SDL_RenderPresent(renderer); /* yazilim render kuyrugunu bosalt */
        SDL_BlitSurface(canvas, NULL, target, NULL);
        SDL_UpdateWindowSurface(window);

        SDL_LockSurface(target);
        if (SDL_BYTESPERPIXEL(target->format) == 4) {
            for (int y = 0; y < height; y++) {
                memcpy(screen + (size_t)y * width * 4,
                       (uint8_t *)target->pixels + (size_t)y * target->pitch,
                       (size_t)width * 4);
            }
        }
        SDL_UnlockSurface(target);

        frames++;
        SDL_Delay(16);
    }

    printf("sdl-demo: %d kare, %d tus, TAMAM\n", frames, keys);
    munmap(screen, screen_bytes);
    close(fb);
    SDL_DestroyRenderer(renderer);
    SDL_DestroySurface(canvas);
    SDL_DestroyWindow(window);
    SDL_Quit();
    return 0;
}
