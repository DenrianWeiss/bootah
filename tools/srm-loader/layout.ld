OUTPUT_ARCH(alpha)
ENTRY(_start)
SECTIONS {
  . = 0x20000000;
  .text : { KEEP(*(.text.entry)) *(.text*) }
  .rodata : { *(.rodata*) }
  .data : { *(.data*) *(.sdata*) }
  . = ALIGN(16);
  _gp = . + 0x8000;
  .bss : { *(.bss*) *(.sbss*) *(COMMON) }
  . = ALIGN(8192);
  _end = .;
  /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
