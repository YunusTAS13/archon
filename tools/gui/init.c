#include <fcntl.h>
#include <unistd.h>
#include <sys/wait.h>

/*
 * pid 1: /etc/gui-enabled varsa grafik uçbirimi, yoksa konsolda kabuğu
 * başlatır. Çocuk çıkarsa yeniden başlatır.
 */

static void run(const char *path, char *const argv[], char *const envp[]) {
    pid_t child = fork();
    if (child == 0) {
        /* Kabuk kendi süreç grubunda olsun; ^C yalnız onu etkilesin. */
        setsid();
        execve(path, argv, envp);
        _exit(127);
    }
    int status = 0;
    waitpid(child, &status, 0);
}

int main(void) {
    char *envp[] = {
        "PATH=/bin:/usr/bin:/sbin:/usr/sbin",
        "HOME=/",
        "TERM=linux",
        "PS1=archon# ",
        0,
    };
    int gui = access("/etc/gui-enabled", F_OK) == 0;

    for (;;) {
        if (gui) {
            char *argv[] = {"/bin/gui-terminal", 0};
            run("/bin/gui-terminal", argv, envp);
        } else {
            char *argv[] = {"/bin/sh", "-i", 0};
            run("/bin/sh", argv, envp);
            const char message[] = "\n[kabuk kapandi, yeniden baslatiliyor]\n";
            write(1, message, sizeof(message) - 1);
        }
    }
}
