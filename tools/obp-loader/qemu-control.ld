/* Test-only OpenBIOS -kernel entry mapping; production body stays unchanged. */
OUTPUT_ARCH(sparc:v9)
ENTRY(_bootstrap)
SECTIONS {
  . = 0x40004000;
  .text : AT(0x2000000) {
    *(.text*)
    /* OpenBIOS maps kernel_size bytes starting 16 KiB before its entry. */
    . += 0x4000; BYTE(0);
  }
  /DISCARD/ : { *(.comment) *(.eh_frame*) }
}
