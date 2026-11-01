OUTPUT_ARCH(sparc:v9)
ENTRY(_start)
SECTIONS {
  . = 0x1000000;
  .text : { KEEP(*(.text.entry)) *(.text*) }
  .rodata : { *(.rodata*) }
  .data : { *(.data*) }
  .bss : { *(.bss*) *(COMMON) . = ALIGN(16); . += 65536; stack_top = .; }
  image_end = .;
  /DISCARD/ : { *(.comment) *(.eh_frame*) }
}
