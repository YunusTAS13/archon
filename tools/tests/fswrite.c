/* ext4 yazma testi: dosya oluşturma, yazma, okuma, dizin, silme. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

static int failures;

static void check(int condition, const char *message) {
    if (condition) {
        printf("  ok   %s\n", message);
    } else {
        printf("  FAIL %s (errno=%d %s)\n", message, errno, strerror(errno));
        failures++;
    }
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("fswrite basliyor\n");

    unlink("/archon-write.txt");
    int fd = open("/archon-write.txt", O_CREAT | O_RDWR, 0644);
    check(fd >= 0, "ext4 uzerinde dosya olustur");
    if (fd < 0) {
        return 1;
    }

    const char *text = "archon ext4 yazma testi\n";
    check(write(fd, text, strlen(text)) == (ssize_t)strlen(text), "yaz");
    check(fsync(fd) == 0, "fsync");
    check(close(fd) == 0, "close");

    char buffer[128] = { 0 };
    fd = open("/archon-write.txt", O_RDONLY);
    check(fd >= 0, "yeniden ac");
    ssize_t count = read(fd, buffer, sizeof(buffer));
    check(count == (ssize_t)strlen(text) && memcmp(buffer, text, strlen(text)) == 0,
          "icerik ayni");
    close(fd);

    struct stat info;
    check(stat("/archon-write.txt", &info) == 0, "stat");
    check(S_ISREG(info.st_mode) && info.st_size == (off_t)strlen(text), "boyut dogru");

    /* Ek yazma: aynı dosyanın sonuna. */
    fd = open("/archon-write.txt", O_WRONLY | O_APPEND);
    check(fd >= 0, "O_APPEND ile ac");
    check(write(fd, "ek\n", 3) == 3, "ekle");
    close(fd);
    check(stat("/archon-write.txt", &info) == 0 && info.st_size == (off_t)strlen(text) + 3,
          "boyut buyudu");

    /* Büyük dosya: çok blok ve tek extent. */
    int big = open("/archon-big.bin", O_CREAT | O_RDWR, 0644);
    check(big >= 0, "buyuk dosya ac");
    unsigned char *chunk = malloc(4096);
    int order_ok = 1;
    for (int index = 0; index < 64; index++) {
        memset(chunk, index & 0xff, 4096);
        if (write(big, chunk, 4096) != 4096) {
            order_ok = 0;
            break;
        }
    }
    check(order_ok, "64 x 4K yaz");
    fsync(big);
    check(lseek(big, 0, SEEK_SET) == 0, "basa don");
    int verify_ok = 1;
    for (int index = 0; index < 64; index++) {
        if (read(big, chunk, 4096) != 4096) {
            verify_ok = 0;
            break;
        }
        for (int byte = 0; byte < 4096; byte++) {
            if (chunk[byte] != (unsigned char)(index & 0xff)) {
                verify_ok = 0;
                break;
            }
        }
        if (!verify_ok) {
            break;
        }
    }
    check(verify_ok, "256 KiB icerik dogru");
    close(big);
    free(chunk);

    /* Dizin işlemleri. */
    unlink("/archontest/inner.txt");
    unlink("/archon-big.bin");
    unlink("/archon-write.txt");
    rmdir("/archontest");
    check(mkdir("/archontest", 0755) == 0, "mkdir");
    int inner = open("/archontest/inner.txt", O_CREAT | O_RDWR, 0600);
    check(inner >= 0, "alt dizinde dosya");
    check(inner >= 0 && write(inner, "x", 1) == 1, "alt dosyaya yaz");
    if (inner >= 0) {
        close(inner);
    }
    check(access("/archontest/inner.txt", F_OK) == 0, "alt dosya gorunur");
    check(stat("/archontest", &info) == 0 && S_ISDIR(info.st_mode), "dizin turu");

    check(unlink("/archontest/inner.txt") == 0, "alt dosyayi sil");
    check(access("/archontest/inner.txt", F_OK) != 0, "silinen dosya yok");
    check(rmdir("/archontest") == 0, "rmdir");
    check(access("/archontest", F_OK) != 0, "dizin silindi");

    printf("--- %d hata ---\n", failures);
    return failures == 0 ? 0 : 1;
}
