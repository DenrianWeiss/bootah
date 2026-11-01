/* Position-independent Sun a.out entry. Claim before moving the linked image. */
.section .text,"ax"
.global _bootstrap
_bootstrap:
    save %sp, -192, %sp
    mov %i4, %l0
    call 1f
     nop
1:  add %o7, 8, %l7
    add %l7, claim_args - 1b, %o0
    jmpl %l0, %o7
     nop
    brnz,pn %o0, failed
     nop
    add %l7, claim_args - 1b, %l2
    ldx [%l2 + 48], %l3
    sethi %hi(0x1000000), %l1
    cmp %l3, %l1
    bne,pn %xcc, failed
     nop
    add %l7, image - 1b, %l2
    sethi %hi(image_end - image), %l3
    or %l3, %lo(image_end - image), %l3
    mov %l1, %l4
2:  ldub [%l2], %l5
    stb %l5, [%l4]
    add %l2, 1, %l2
    add %l4, 1, %l4
    subcc %l3, 1, %l3
    bne,pt %xcc, 2b
     nop
    mov %l1, %l2
3:  flush %l2
    add %l2, 8, %l2
    cmp %l2, %l4
    blu,pt %xcc, 3b
     nop
    membar #Sync
    mov %l1, %g1
    jmpl %g1, %g0
     restore
failed:
    add %l7, exit_args - 1b, %o0
    jmpl %l0, %o7
     nop
4:  ba 4b
     nop
.balign 8
claim_args:
    .xword claim_name, 3, 1, 0x1000000, ((image_end - image + 8191) & -8192), 0, -1
exit_args:
    .xword exit_name, 0, 0
claim_name: .asciz "claim"
exit_name: .asciz "exit"
.balign 8
image:
.incbin "bootah-obp.raw"
image_end:
.section .note.GNU-stack,"",@progbits
