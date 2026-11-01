OUTPUT_ARCH(sparc:v9)
ENTRY(_bootstrap)
SECTIONS {
  . = 0x4000;
  .text : { *(.text*) }
  /DISCARD/ : { *(.comment) *(.eh_frame*) }
}
