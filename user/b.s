# Ring3 preemption testi: harf yaz, gecik, tekrarla, cik.
.section .text
.global _start
_start:
    movq $40, %r12
loop:
    movq $1, %rax
    movq $1, %rdi
    leaq letter(%rip), %rsi
    movq $1, %rdx
    syscall

    movq $20000000, %rcx
delay:
    decq %rcx
    jnz delay

    decq %r12
    jnz loop

    movq $60, %rax
    xorq %rdi, %rdi
    syscall
1:
    jmp 1b

.section .rodata
letter:
    .byte 66
