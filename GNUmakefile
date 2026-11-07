override MAKEFLAGS += -rR
.DEFAULT_GOAL := all

-include local.mk

BUILD ?= build
TARGET := x86_64-unknown-none
LIMINE_REPO := https://github.com/limine-bootloader/limine

KERNEL := $(BUILD)/archon.elf
ISO := $(BUILD)/archon.iso
INITRAMFS := $(BUILD)/initramfs.cpio
ROOTFS := $(BUILD)/rootfs.img
USER_PROGRAMS := init hello forktest
# Limine ikilileri ve basligi depoda hazir (vendor/limine). Yeniden indirmek
# icin `make limine-download`.
LIMINE_DIR ?= vendor/limine
LIMINE_H := $(LIMINE_DIR)/limine.h
LIMINE_IDS := kernel/src/boot/limine_ids.rs

CARGO ?= cargo
RUSTFLAGS_KERNEL := -C code-model=kernel -C relocation-model=static \
	-C link-arg=-Tkernel/linker.ld -C link-arg=--gc-sections \
	-C link-arg=--build-id=none -C link-arg=-zmax-page-size=0x1000 \
	-C no-redzone=yes -C force-frame-pointers=yes

QEMU ?= qemu-system-x86_64
QEMU_ACCEL ?= $(shell if test -r /dev/kvm && test -w /dev/kvm; then echo kvm; else echo tcg; fi)
QEMU_MEM ?= 2G
QEMU_SMP ?= 1
# CachyOS/Arch paketleri x86-64-v3 (AVX2) varsayar; qemu64 modelinde AVX yok.
QEMU_CPU ?= host
QEMU_EXTRA ?=
SERIAL_LOG := $(BUILD)/serial.log

.PHONY: all deps kernel iso run run-gui debug check clean distclean limine-ids help \
	user initramfs disk limine-download

all: iso

limine-download:
	@test -n "$(LIMINE_VERSION)" || (echo "LIMINE_VERSION tanimli degil" && false)
	mkdir -p $(BUILD)/limine-extract && rm -rf $(BUILD)/limine-extract/*
	curl -fL --retry 3 -o $(BUILD)/limine.tar.gz \
		$(LIMINE_REPO)/releases/download/$(LIMINE_VERSION)/limine-$(LIMINE_VERSION).tar.gz
	tar -xzf $(BUILD)/limine.tar.gz -C $(BUILD)/limine-extract --strip-components=1
	mkdir -p $(LIMINE_DIR)
	cp -r $(BUILD)/limine-extract/limine-binary/* $(LIMINE_DIR)/
	cp $(BUILD)/limine-extract/limine.h $(LIMINE_H)

help:
	@echo "make deps        - Limine indirir, limine_ids.rs üretir"
	@echo "make             - kernel + ISO"
	@echo "make run         - QEMU, seri konsol (nographic)"
	@echo "make run-gui     - QEMU, framebuffer penceresi"
	@echo "make debug       - QEMU + gdb sunucusu (1234)"
	@echo "make check       - derleme kontrolü, ISO üretmez"
	@echo "make user        - ring3 test programlarını derler"
	@echo "make disk        - ext4 test imajını üretir"
	@echo "make clean       - build/ sil"

$(BUILD):
	@mkdir -p $(BUILD)

$(LIMINE_H):
	@echo "vendor/limine/limine.h yok; once 'make limine-download'"
	@false

limine-ids: $(LIMINE_H)
	python3 tools/gen_limine_ids.py $(LIMINE_H) $(LIMINE_IDS)

deps: limine-ids

$(LIMINE_IDS): $(LIMINE_H)
	python3 tools/gen_limine_ids.py $(LIMINE_H) $(LIMINE_IDS)

KERNEL_SOURCES := $(shell find kernel -name '*.rs' -o -name 'Cargo.toml' -o -name 'linker.ld')

$(KERNEL): $(LIMINE_IDS) $(KERNEL_SOURCES)
	RUSTFLAGS="$(RUSTFLAGS_KERNEL)" $(CARGO) build \
		-Z build-std=core,alloc,compiler_builtins \
		-Z build-std-features=compiler-builtins-mem \
		--target $(TARGET) --release
	cp target/$(TARGET)/release/archon $@

kernel: $(KERNEL)

# --- ring3 programları ve initramfs ---

$(BUILD)/initramfs/bin/%: user/%.s | $(BUILD)
	@mkdir -p $(BUILD)/initramfs/bin
	as --64 -o $(BUILD)/$*.o $<
	ld -Ttext=0x400000 -nostdlib -o $@ $(BUILD)/$*.o

user: $(addprefix $(BUILD)/initramfs/bin/,$(USER_PROGRAMS))

$(INITRAMFS): user | $(BUILD)
	@mkdir -p $(BUILD)/initramfs/etc
	printf 'Archon ring3: initramfs calisiyor.\nBu dosya /etc/motd, cpio arsivinden acildi.\n' \
		> $(BUILD)/initramfs/etc/motd
	python3 tools/mkinitramfs.py $@ $(BUILD)/initramfs

initramfs: $(INITRAMFS)

# --- ext4 test imajı (gerçek bir dosya sistemi, virtio-blk üzerinden) ---

disk: $(ROOTFS)

$(ROOTFS): $(addprefix $(BUILD)/initramfs/bin/,$(USER_PROGRAMS)) | $(BUILD)
	@mkdir -p $(BUILD)/rootdir/bin $(BUILD)/rootdir/etc $(BUILD)/rootdir/lib64 \
		$(BUILD)/rootdir/usr/lib $(BUILD)/rootdir/dev $(BUILD)/rootdir/tmp $(BUILD)/rootdir/proc
	cp $(BUILD)/initramfs/bin/init $(BUILD)/rootdir/bin/init
	cp $(BUILD)/initramfs/bin/hello $(BUILD)/rootdir/bin/hello
	cp $(BUILD)/initramfs/bin/forktest $(BUILD)/rootdir/bin/forktest
	printf 'Archon ext4 kok: hos geldiniz.\n' > $(BUILD)/rootdir/etc/motd
	printf 'archon-test\n' > $(BUILD)/rootdir/etc/hostname
	printf 'NAME="Archon Test Root"\nID=archon\n' > $(BUILD)/rootdir/etc/os-release
	printf 'ext4 uzerinden okunan dosya\n' > $(BUILD)/rootdir/hello.txt
	truncate -s 64M $@
	mke2fs -q -t ext4 -d $(BUILD)/rootdir -F $@

iso: $(KERNEL) $(LIMINE_H) $(INITRAMFS)
	rm -rf $(BUILD)/iso-root
	mkdir -p $(BUILD)/iso-root/boot
	cp $(KERNEL) $(BUILD)/iso-root/boot/archon.elf
	cp $(INITRAMFS) $(BUILD)/iso-root/boot/initramfs.cpio
	cp boot/limine.conf $(BUILD)/iso-root/boot/limine.conf
	cp -r $(LIMINE_DIR)/limine-bios.sys $(LIMINE_DIR)/limine-bios-cd.bin \
		$(LIMINE_DIR)/limine-uefi-cd.bin $(BUILD)/iso-root/boot/
	xorriso -as mkisofs -b boot/limine-bios-cd.bin -no-emul-boot \
		-boot-load-size 4 -boot-info-table --efi-boot boot/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(BUILD)/iso-root -o $(ISO)
	$(LIMINE_DIR)/limine bios-install $(ISO)

run: iso $(ROOTFS)
	$(QEMU) -M q35 -m $(QEMU_MEM) -smp $(QEMU_SMP) -accel $(QEMU_ACCEL) \
		-cpu $(QEMU_CPU) \
		-cdrom $(ISO) -boot d -serial file:$(SERIAL_LOG) -display none \
		-drive file=$(ROOTFS),if=none,id=disk0,format=raw \
		-device virtio-blk-pci,drive=disk0,disable-legacy=on \
		-monitor none -no-reboot $(QEMU_EXTRA)
	@echo "--- serial ---"
	@cat $(SERIAL_LOG)

run-gui: iso $(ROOTFS)
	$(QEMU) -M q35 -m $(QEMU_MEM) -smp $(QEMU_SMP) -accel $(QEMU_ACCEL) \
		-cpu $(QEMU_CPU) \
		-cdrom $(ISO) -boot d -serial stdio -vga std \
		-drive file=$(ROOTFS),if=none,id=disk0,format=raw \
		-device virtio-blk-pci,drive=disk0,disable-legacy=on \
		-no-reboot $(QEMU_EXTRA)

debug: iso
	$(QEMU) -M q35 -m $(QEMU_MEM) -smp 1 -accel $(QEMU_ACCEL) \
		-cdrom $(ISO) -boot d -serial stdio -display none -no-reboot \
		-s -S $(QEMU_EXTRA)

check:
	RUSTFLAGS="$(RUSTFLAGS_KERNEL)" $(CARGO) check \
		-Z build-std=core,alloc,compiler_builtins \
		--target $(TARGET)

clean:
	rm -rf $(BUILD)

distclean: clean
	rm -rf target $(LIMINE_IDS)
