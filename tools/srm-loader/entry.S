.set noreorder
.section .text.entry,"ax"
.globl _start
.ent _start
_start:
    br $27,1f
1:  ldgp $29,0($27)
    jsr $26,loader_main
    call_pal 0
.end _start

.text
.align 4
.globl srm_call
.ent srm_call
srm_call:
    ldgp $29,0($27)
    lda $30,-80($30)
    stq $26,0($30)
    stq $9,8($30)
    stq $10,16($30)
    stq $11,24($30)
    stq $12,32($30)
    stq $13,40($30)
    stq $14,48($30)
    stq $15,56($30)
    stq $29,64($30)
    ldq $27,dispatch_descriptor
    ldq $1,8($27)
    jsr $26,($1)
    ldq $26,0($30)
    ldq $9,8($30)
    ldq $10,16($30)
    ldq $11,24($30)
    ldq $12,32($30)
    ldq $13,40($30)
    ldq $14,48($30)
    ldq $15,56($30)
    ldq $29,64($30)
    lda $30,80($30)
    ret $31,($26),1
.end srm_call

.align 4
.globl osf_switch
.ent osf_switch
osf_switch:
    lda $30,-160($30)
    stq $26,0($30)
    stq $29,8($30)
    stq $9,16($30)
    stq $10,24($30)
    stq $11,32($30)
    stq $12,40($30)
    stq $13,48($30)
    stq $14,56($30)
    stq $15,64($30)
    /* a0=physical PCB, a1=PTBR, a2=VPTB, a3=virtual PCB. */
    stq $30,0($19)
    stq $17,16($19)
    mov $30,$20
    mov $18,$19
    mov $16,$18
    lda $16,2($31)
    br $17,2f
1:  ldq $26,0($30)
    ldq $29,8($30)
    ldq $9,16($30)
    ldq $10,24($30)
    ldq $11,32($30)
    ldq $12,40($30)
    ldq $13,48($30)
    ldq $14,56($30)
    ldq $15,64($30)
    lda $30,160($30)
    ret $31,($26),1
2:  call_pal 10
    br $31,1b
.end osf_switch

.globl kernel_enter
.ent kernel_enter
kernel_enter:
    mov $17,$30
    mov $16,$27
    lda $16,1($31)
    call_pal 0x2b
    call_pal 0x86
    jmp $31,($27),0
.end kernel_enter
