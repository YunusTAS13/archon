# /bin/hello: exec zincirinin ikinci halkasi.
.section .text
.global _start
_start:
    movq $1, %rax
    movq $1, %rdi
    leaq message(%rip), %rsi
    movq $message_length, %rdx
    syscall

    movq $60, %rax
    xorq %rdi, %rdi
    syscall
1:
    jmp 1b

.section .rodata
message:
    .ascii "hello: /bin/hello calisiyor (exec basarili)\n"
message_length = . - message
