CROSS_COMPILE ?= alpha-linux-gnu-
CC = $(CROSS_COMPILE)gcc
CFLAGS = -Os -mcpu=ev4 -mno-fp-regs -mno-bwx -ffreestanding -fno-builtin -fno-stack-protector -fno-pie -Wall -Wextra -Werror

all: bootah-srm.bin
bootah-srm.elf: entry.S loader.c layout.ld
	$(CC) $(CFLAGS) -nostdlib -static -no-pie -Wl,--build-id=none,-T,layout.ld entry.S loader.c -lgcc -o $@
bootah-srm.bin: bootah-srm.elf
	$(CROSS_COMPILE)objcopy -O binary --set-section-flags .bss=alloc,load,contents $< $@
clean:
	rm -f bootah-srm.elf bootah-srm.bin
install: bootah-srm.bin
	cp bootah-srm.bin ../../assets/bootah-srm.bin
.PHONY: all clean install
