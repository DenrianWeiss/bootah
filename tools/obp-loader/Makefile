CROSS_COMPILE ?= sparc64-linux-gnu-
CC = $(CROSS_COMPILE)gcc
CFLAGS = -Os -mcpu=ultrasparc -m64 -mcmodel=medlow -mno-app-regs -msoft-float -ffreestanding -fno-builtin -fno-stack-protector -fno-pie -Wall -Wextra -Werror
all: bootah-obp.bin
bootah-obp.elf: entry.S loader.c network.h dhcp.h handoff.h native.h layout.ld
	$(CC) $(CFLAGS) -nostdlib -static -no-pie -Wl,--build-id=none,-T,layout.ld entry.S loader.c -lgcc -o $@
bootah-obp.raw: bootah-obp.elf
	$(CROSS_COMPILE)objcopy -O binary --set-section-flags .bss=alloc,load,contents $< $@
bootstrap.elf: bootstrap.S bootstrap.ld bootah-obp.raw
	$(CC) $(CFLAGS) -nostdlib -static -no-pie -Wl,--build-id=none,-T,bootstrap.ld bootstrap.S -o $@
bootstrap.raw: bootstrap.elf
	$(CROSS_COMPILE)objcopy -O binary $< $@
bootah-obp.bin: bootstrap.raw wrap.py
	python3 wrap.py $< $@
install: bootah-obp.bin
	cp $< ../../assets/bootah-obp.bin
clean:
	rm -f cpu-control.elf bootah-obp.elf bootah-obp.raw bootah-obp.bin qemu-control.elf bootstrap.elf bootstrap.raw .handoff-check .properties-check
	rm -rf .cpu-control
.PHONY: all install clean
HOSTCC ?= cc
check:
	$(HOSTCC) -std=c11 -O2 -Wall -Wextra -Werror -fsanitize=address,undefined test-handoff.c -o .handoff-check
	./.handoff-check
	$(HOSTCC) -std=c11 -O2 -ffreestanding -fno-builtin -Wall -Wextra -Werror -fsanitize=address,undefined test-properties.c -o .properties-check
	./.properties-check
	rm .handoff-check .properties-check
qemu-control.elf: bootstrap.S bootah-obp.raw qemu-control.ld
	$(CC) $(CFLAGS) -nostdlib -static -no-pie -Wl,--build-id=none,-T,qemu-control.ld bootstrap.S -o $@
.PHONY: check

# Isolated build: test payload exercises the production trampoline through CIF.
cpu-control.elf: test-entry.c entry.S loader.c network.h dhcp.h handoff.h native.h layout.ld bootstrap.S qemu-control.ld
	mkdir -p .cpu-control
	$(CC) $(CFLAGS) -nostdlib -static -no-pie -Wl,--build-id=none,-T,layout.ld entry.S test-entry.c -lgcc -o .cpu-control/body.elf
	$(CROSS_COMPILE)objcopy -O binary --set-section-flags .bss=alloc,load,contents .cpu-control/body.elf .cpu-control/bootah-obp.raw
	cp bootstrap.S qemu-control.ld .cpu-control/
	cd .cpu-control && $(CC) $(CFLAGS) -nostdlib -static -no-pie -Wl,--build-id=none,-T,qemu-control.ld bootstrap.S -o ../$@
