.section ".text.entry", "ax"
.global _start
_start:
    mov %o4, %g4
    mov %sp, %g5
    sethi %hi(stack_top), %sp
    or %sp, %lo(stack_top), %sp
    sub %sp, 2047, %sp
    mov %g4, %o0
    call loader_main
     mov %g5, %o1
1:  ba 1b
     nop

.section .note.GNU-stack,"",@progbits

.section .text,"ax"
.register %g2, #scratch
.register %g3, #scratch
.balign 8
.global stage_trampoline_start, stage_trampoline_end
stage_trampoline_start:
    mov %o1, %sp
    save %sp, -192, %sp
    mov %i0, %l0
    ldx [%l0 + 0], %l1
    ldx [%l0 + 8], %l2
    flushw
    add %l0, 24, %o0
    jmpl %l2, %o7
     nop
    mov %l1, %g1
    mov %l2, %g2
    ldx [%l0 + 16], %g3
    rdpr %ver, %g4
    and %g4, 31, %g4
    wrpr %g0, 0, %canrestore
    wrpr %g0, 0, %otherwin
    wrpr %g4, 0, %cleanwin
    sub %g4, 1, %g4
    wrpr %g4, 0, %cansave
    wrpr %g0, 0, %cwp
    mov %g3, %sp
    clr %o0
    clr %o1
    clr %o2
    clr %o3
    mov %g2, %o4
    clr %o5
    jmpl %g1, %g0
     nop
stage_trampoline_end:
