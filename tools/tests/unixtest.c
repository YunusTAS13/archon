/* AF_UNIX soket testi: yol adı, soyut ad, SCM_RIGHTS, epoll, shutdown. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/un.h>
#include <sys/wait.h>
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

static void send_fd(int socket, const char *text, int *fds, int count) {
    struct iovec vector = { .iov_base = (void *)text, .iov_len = strlen(text) };
    char control[CMSG_SPACE(sizeof(int) * 8)];
    memset(control, 0, sizeof(control));
    struct msghdr message = { 0 };
    message.msg_iov = &vector;
    message.msg_iovlen = 1;
    if (count > 0) {
        message.msg_control = control;
        message.msg_controllen = CMSG_SPACE(sizeof(int) * count);
        struct cmsghdr *header = CMSG_FIRSTHDR(&message);
        header->cmsg_level = SOL_SOCKET;
        header->cmsg_type = SCM_RIGHTS;
        header->cmsg_len = CMSG_LEN(sizeof(int) * count);
        memcpy(CMSG_DATA(header), fds, sizeof(int) * count);
    }
    ssize_t written = sendmsg(socket, &message, 0);
    check(written == (ssize_t)strlen(text), "sendmsg veri ve fd");
}

static ssize_t receive_fd(int socket, char *buffer, size_t size, int *fds, int *count) {
    struct iovec vector = { .iov_base = buffer, .iov_len = size };
    char control[CMSG_SPACE(sizeof(int) * 8)];
    memset(control, 0, sizeof(control));
    struct msghdr message = { 0 };
    message.msg_iov = &vector;
    message.msg_iovlen = 1;
    message.msg_control = control;
    message.msg_controllen = sizeof(control);
    ssize_t received = recvmsg(socket, &message, 0);
    *count = 0;
    if (received < 0) {
        return received;
    }
    for (struct cmsghdr *header = CMSG_FIRSTHDR(&message); header;
         header = CMSG_NXTHDR(&message, header)) {
        if (header->cmsg_level == SOL_SOCKET && header->cmsg_type == SCM_RIGHTS) {
            int items = (header->cmsg_len - CMSG_LEN(0)) / sizeof(int);
            memcpy(fds, CMSG_DATA(header), items * sizeof(int));
            *count += items;
        }
    }
    return received;
}

static void test_socketpair(void) {
    printf("socketpair + SCM_RIGHTS\n");
    int pair[2];
    check(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0, "socketpair");

    int ends[2];
    check(pipe(ends) == 0, "pipe");
    send_fd(pair[0], "fd", &ends[0], 1);

    char buffer[16] = { 0 };
    int fds[8];
    int count = 0;
    ssize_t received = receive_fd(pair[1], buffer, sizeof(buffer), fds, &count);
    check(received == 2 && memcmp(buffer, "fd", 2) == 0, "recvmsg verisi");
    check(count == 1, "bir fd gecti");

    check(write(ends[1], "x", 1) == 1, "yazma");
    char byte = 0;
    check(count == 1 && read(fds[0], &byte, 1) == 1 && byte == 'x', "gecen fd calisiyor");
    for (int i = 0; i < count; i++) {
        close(fds[i]);
    }
    close(ends[0]);
    close(ends[1]);
    close(pair[0]);
    close(pair[1]);
}

static int pathname_listener(const char *path) {
    int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    check(listener >= 0, "dinleyici soket");
    unlink(path);
    struct sockaddr_un address = { 0 };
    address.sun_family = AF_UNIX;
    strncpy(address.sun_path, path, sizeof(address.sun_path) - 1);
    check(bind(listener, (struct sockaddr *)&address, sizeof(address)) == 0, "bind yol adi");
    check(listen(listener, 8) == 0, "listen");

    struct sockaddr_un local = { 0 };
    socklen_t length = sizeof(local);
    check(getsockname(listener, (struct sockaddr *)&local, &length) == 0, "getsockname");
    check(length > 2 && strcmp(local.sun_path, path) == 0, "getsockname yolu dogru");
    check(local.sun_family == AF_UNIX, "getsockname ailesi AF_UNIX");
    return listener;
}

static int connect_path(const char *path) {
    int client = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un address = { 0 };
    address.sun_family = AF_UNIX;
    strncpy(address.sun_path, path, sizeof(address.sun_path) - 1);
    if (connect(client, (struct sockaddr *)&address, sizeof(address)) != 0) {
        printf("  connect hatasi: %d %s\n", errno, strerror(errno));
        close(client);
        return -1;
    }
    return client;
}

static void test_pathname(void) {
    printf("yol adli dinleyici + fork\n");
    const char *path = "/tmp/unixtest.sock";
    int listener = pathname_listener(path);

    pid_t child = fork();
    if (child == 0) {
        int client = connect_path(path);
        if (client < 0) {
            _exit(1);
        }
        int zero = open("/dev/zero", O_RDONLY);
        send_fd(client, "ping", &zero, 1);
        close(client);
        _exit(0);
    }

    int server = accept(listener, NULL, NULL);
    check(server >= 0, "accept");
    char buffer[16] = { 0 };
    int fds[8];
    int count = 0;
    ssize_t received = receive_fd(server, buffer, sizeof(buffer), fds, &count);
    check(received == 4 && memcmp(buffer, "ping", 4) == 0, "sunucu veriyi aldi");
    check(count == 1, "sunucu fd aldi");
    if (count == 1) {
        char zeros[4];
        check(read(fds[0], zeros, 4) == 4, "alman fd /dev/zero");
        close(fds[0]);
    }
    close(server);

    int status = 0;
    waitpid(child, &status, 0);
    check(WIFEXITED(status) && WEXITSTATUS(status) == 0, "istemci basarili");
    close(listener);
}

static void test_abstract(void) {
    printf("soyut ad alani\n");
    const char *name = "archon-test";
    int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un address = { 0 };
    address.sun_family = AF_UNIX;
    address.sun_path[0] = 0;
    memcpy(address.sun_path + 1, name, strlen(name));
    socklen_t length = offsetof(struct sockaddr_un, sun_path) + 1 + strlen(name);
    check(bind(listener, (struct sockaddr *)&address, length) == 0, "soyut bind");
    check(listen(listener, 4) == 0, "soyut listen");

    pid_t child = fork();
    if (child == 0) {
        int client = socket(AF_UNIX, SOCK_STREAM, 0);
        if (connect(client, (struct sockaddr *)&address, length) != 0) {
            _exit(1);
        }
        if (write(client, "abs", 3) != 3) {
            _exit(1);
        }
        close(client);
        _exit(0);
    }

    int server = accept(listener, NULL, NULL);
    check(server >= 0, "soyut accept");
    char buffer[8] = { 0 };
    check(server >= 0 && read(server, buffer, 3) == 3 && memcmp(buffer, "abs", 3) == 0,
          "soyut veri");
    if (server >= 0) {
        close(server);
    }
    int status = 0;
    waitpid(child, &status, 0);
    check(WIFEXITED(status) && WEXITSTATUS(status) == 0, "soyut istemci");
    close(listener);
}

static void test_epoll_and_shutdown(void) {
    printf("epoll + shutdown\n");
    int pair[2];
    check(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0, "socketpair");
    int epoll = epoll_create1(0);
    check(epoll >= 0, "epoll_create1");
    struct epoll_event event = { .events = EPOLLIN, .data.fd = pair[1] };
    check(epoll_ctl(epoll, EPOLL_CTL_ADD, pair[1], &event) == 0, "epoll_ctl");

    check(write(pair[0], "z", 1) == 1, "yazma");
    struct epoll_event ready = { 0 };
    check(epoll_wait(epoll, &ready, 1, 1000) == 1, "epoll_wait hazir");
    check(ready.data.fd == pair[1] && (ready.events & EPOLLIN), "epoll olayi dogru");

    check(shutdown(pair[0], SHUT_WR) == 0, "shutdown SHUT_WR");
    char byte = 0;
    check(read(pair[1], &byte, 1) == 1 && byte == 'z', "kalan veri");
    check(read(pair[1], &byte, 1) == 0, "kapaninca EOF");

    close(epoll);
    close(pair[0]);
    close(pair[1]);
}

static void test_nonblocking(void) {
    printf("engellemesiz accept\n");
    int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un address = { 0 };
    address.sun_family = AF_UNIX;
    strcpy(address.sun_path, "/tmp/unixtest-nb.sock");
    unlink(address.sun_path);
    check(bind(listener, (struct sockaddr *)&address, sizeof(address)) == 0, "bind");
    check(listen(listener, 2) == 0, "listen");
    int flags = fcntl(listener, F_GETFL, 0);
    check(fcntl(listener, F_SETFL, flags | O_NONBLOCK) == 0, "O_NONBLOCK");
    errno = 0;
    int server = accept(listener, NULL, NULL);
    check(server < 0 && (errno == EAGAIN || errno == EWOULDBLOCK), "accept EAGAIN");
    close(listener);
}

static void test_errors(void) {
    printf("hata durumlari\n");
    int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un address = { 0 };
    address.sun_family = AF_UNIX;
    strcpy(address.sun_path, "/tmp/unixtest-err.sock");
    unlink(address.sun_path);
    check(bind(listener, (struct sockaddr *)&address, sizeof(address)) == 0, "bind");
    int other = socket(AF_UNIX, SOCK_STREAM, 0);
    errno = 0;
    check(bind(other, (struct sockaddr *)&address, sizeof(address)) < 0 && errno == EADDRINUSE,
          "ikinci bind EADDRINUSE");
    check(connect(other, (struct sockaddr *)&address, sizeof(address)) < 0, "dinlemeyene connect");
    close(other);
    close(listener);

    int pair[2];
    socketpair(AF_UNIX, SOCK_STREAM, 0, pair);
    close(pair[1]);
    errno = 0;
    ssize_t written = write(pair[0], "q", 1);
    check(written < 0 && errno == EPIPE, "kapanmis uca yazma EPIPE");
    close(pair[0]);
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("unixtest basliyor\n");
    test_socketpair();
    test_pathname();
    test_abstract();
    test_epoll_and_shutdown();
    test_nonblocking();
    test_errors();
    printf("--- %d hata ---\n", failures);
    return failures == 0 ? 0 : 1;
}
