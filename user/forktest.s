# forktest: fork + wait4 + exit durumu.
.section .text
.global _start
_start:
    # fork()
    movq $57, %rax
    syscall
    testq %rax, %rax
    js fork_failed
    jz child

    # ebeveyn: wait4(pid, &status, 0, NULL)
    movq %rax, %r12
    movq $61, %rax
    movq %r12, %rdi
    leaq status(%rip), %rsi
    xorq %rdx, %rdx
    xorq %r10, %r10
    syscall

    # "parent: cocuk toplandi\n"
    movq $1, %rax
    movq $1, %rdi
    leaq parent_message(%rip), %rsi
    movq $parent_message_length, %rdx
    syscall

    # execve("/bin/hello")
    movq $59, %rax
    leaq hello_path(%rip), %rdi
    xorq %rsi, %rsi
    xorq %rdx, %rdx
    syscall

    movq $60, %rax
    movq $1, %rdi
    syscall

child:
    movq $1, %rax
    movq $1, %rdi
    leaq child_message(%rip), %rsi
    movq $child_message_length, %rdx
    syscall

    movq $60, %rax
    movq $7, %rdi
    syscall

fork_failed:
    movq $1, %rax
    movq $1, %rdi
    leaq failed_message(%rip), %rsi
    movq $failed_message_length, %rdx
    syscall

    movq $60, %rax
    movq $1, %rdi
    syscall
1:
    jmp 1b

.section .rodata
parent_message:
    .ascii "parent: wait4 tamam\n"
parent_message_length = . - parent_message
child_message:
    .ascii "cocuk: merhaba (fork cocugu)\n"
child_message_length = . - child_message
failed_message:
    .ascii "fork basarisiz\n"
failed_message_length = . - failed_message
hello_path:
    .asciz "/bin/hello"

.section .bss
.align 8
status:
    .zero 8
