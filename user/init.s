# init: motd dosyasini okuyup yazar, sonra /bin/hello'yu exec eder.
# Linux x86_64 syscall ABI, statik, libc yok.

.section .text
.global _start
_start:
    # write(1, banner, banner_length)
    movq $1, %rax
    movq $1, %rdi
    leaq banner(%rip), %rsi
    movq $banner_length, %rdx
    syscall

    # open("/etc/motd", O_RDONLY)
    movq $2, %rax
    leaq motd_path(%rip), %rdi
    xorq %rsi, %rsi
    xorq %rdx, %rdx
    syscall
    testq %rax, %rax
    js no_motd
    movq %rax, %r13

    # read(fd, buffer, 256)
    movq $0, %rax
    movq %r13, %rdi
    leaq buffer(%rip), %rsi
    movq $256, %rdx
    syscall
    testq %rax, %rax
    jle no_motd
    movq %rax, %r14

    # write(1, buffer, count)
    movq $1, %rax
    movq $1, %rdi
    leaq buffer(%rip), %rsi
    movq %r14, %rdx
    syscall

no_motd:
    # execve("/bin/hello", ["/bin/hello", NULL], NULL)
    movq $59, %rax
    leaq hello_path(%rip), %rdi
    leaq hello_argv(%rip), %rsi
    xorq %rdx, %rdx
    syscall

    # exec basarisiz
    movq $1, %rax
    movq $1, %rdi
    leaq exec_failed(%rip), %rsi
    movq $exec_failed_length, %rdx
    syscall

    movq $60, %rax
    movq $1, %rdi
    syscall
1:
    jmp 1b

.section .rodata
banner:
    .ascii "init: archon ring3, initramfs hazir\n"
banner_length = . - banner
motd_path:
    .asciz "/etc/motd"
hello_path:
    .asciz "/bin/hello"
exec_failed:
    .ascii "init: /bin/hello calistirilamadi\n"
exec_failed_length = . - exec_failed

.section .data
.align 8
hello_argv:
    .quad hello_arg0
    .quad 0
hello_arg0:
    .asciz "/bin/hello"

.section .bss
.align 16
buffer:
    .zero 256
