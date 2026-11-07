#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![feature(abi_x86_interrupt)]
#![allow(static_mut_refs)]

extern crate alloc;

#[macro_use]
pub mod log;

pub mod acpi;
pub mod arch;
pub mod boot;
pub mod console;
pub mod drivers;
pub mod elf;
pub mod fbtext;
pub mod font;
pub mod fs;
pub mod mem;
pub mod pci;
pub mod sync;
pub mod syscall;
pub mod sysinfo;
pub mod task;
pub mod time;

mod panic;

use boot::BootInfo;

const KERNEL_STACK_SIZE: usize = 64 * 1024;

// Syscall girişi ve ilk user dönüşü bu yığını kullanır. SMP'de per-CPU olacak(apoyu sikeyim)
#[repr(align(16))]
pub struct KernelStack([u8; KERNEL_STACK_SIZE]);

#[used]
pub static mut SYSCALL_STACK: KernelStack = KernelStack([0; KERNEL_STACK_SIZE]);

pub fn syscall_stack_top() -> u64 {
    unsafe {
        let base = core::ptr::addr_of!(SYSCALL_STACK.0) as u64;
        base + KERNEL_STACK_SIZE as u64
    }
}

fn print_banner() {
    println!();
    println!("  Archon 0.1.0 — x86_64, Rust, Limine");
    println!("  ------------------------------------------------");
}

fn print_boot_info(info: &BootInfo) {
    println!(
        "boot: kernel sanal {:#x}, fiziksel {:#x}",
        info.kernel_virtual, info.kernel_physical
    );
    println!("boot: hhdm {:#x}, memmap {} giris", info.hhdm_offset, info.memmap_len);
    println!("boot: kullanilabilir RAM {} MiB", boot::usable_ram_bytes() / (1024 * 1024));
    if info.framebuffer_address != 0 {
        println!(
            "boot: framebuffer {}x{} pitch {}",
            info.framebuffer_width, info.framebuffer_height, info.framebuffer_pitch
        );
    } else {
        println!("boot: framebuffer yok (seri konsol)");
    }
    if info.rsdp != 0 {
        println!("boot: ACPI RSDP {:#x}", info.rsdp);
    }
}

#[no_mangle]
pub extern "C" fn kmain() -> ! {
    arch::serial::init();
    log::set_level(log::LEVEL_INFO);
    arch::cpu::enable_sse();
    arch::cpu::enable_xsave();
    print_banner();

    boot::init();
    let info = boot::info();
    print_boot_info(&info);

    mem::init(&info);
    println!("mem: PMM {} cefre, heap {} MiB", mem::pmm::total_frames(), mem::heap::size() / (1024 * 1024));

    arch::gdt::init();
    println!("arch: GDT + TSS yuklendi");

    arch::idt::init();
    println!("arch: IDT kuruldu, 256 vektor");

    arch::pic::init();
    arch::pic::unmask(arch::pic::IRQ_TIMER);
    arch::pic::unmask(arch::pic::IRQ_KEYBOARD);
    arch::pit::init(100);
    println!("arch: PIC remap, PIT 100 Hz");

    time::init();
    acpi::init(info.rsdp);
    pci::init();
    console::init();
    fbtext::init();
    fs::init();
    fs::initramfs::extract_from_modules();

    // Disk varsa gerçek kök ext4 olur; yoksa initramfs/ramfs ile devam.
    drivers::virtio_blk::init();
    if let Some(device) = drivers::virtio_blk::device() {
        match fs::ext4::Ext4::mount(device) {
            Ok(filesystem) => {
                println!("ext4: disk kok olarak baglandi");
                fs::mount_root(alloc::sync::Arc::new(filesystem));
                ext4_selftest();
            }
            Err(failure) => println!("ext4: baglanamadi ({:?}), ramfs kok", failure),
        }
    }
    fs::mount("/dev", alloc::sync::Arc::new(fs::devfs::DevFs::new()));
    fs::mount("/proc", alloc::sync::Arc::new(fs::procfs::ProcFs::new()));

    // Yazılabilir geçici alanlar: sock dosyaları, kilit dosyaları, paylaşımlı bellek.
    for directory in ["/tmp", "/run", "/var/tmp", "/dev/shm"] {
        let ramfs = alloc::sync::Arc::new(fs::ramfs::RamFs::new());
        fs::mount(directory, ramfs);
    }
    syscall::init(crate::syscall_stack_top());
    task::init();

    if ring3_selftest() {
        sysinfo::print();
        task::start()
    }

    println!("kernel: hazir, bos donguye giriliyor");
    task::idle_loop()
}

/// /mnt kökünü listeler ve bir dosya okur: disk yolunun uçtan uca kanıtı.
fn ext4_selftest() {
    let Ok(root) = fs::lookup_path("/") else {
        println!("ext4: kok acilamadi");
        return;
    };

    match root.readdir() {
        Ok(entries) => {
            let mut names = alloc::string::String::new();
            for (name, _) in entries.iter().take(12) {
                names.push_str(name);
                names.push(' ');
            }
            println!("ext4: kok icinde: {}", names);
        }
        Err(failure) => println!("ext4: readdir hatasi {:?}", failure),
    }

    for candidate in ["/etc/hostname", "/etc/os-release", "/hello.txt"] {
        let Ok(inode) = fs::lookup_path(candidate) else {
            continue;
        };
        let size = inode.metadata().size as usize;
        if size == 0 || size > 4096 {
            continue;
        }
        let mut buffer = alloc::vec![0u8; size];
        if inode.read_at(0, &mut buffer).is_ok() {
            println!(
                "ext4: {} ({} bayt) -> {}",
                candidate,
                size,
                core::str::from_utf8(&buffer).unwrap_or("<ikili>")
            );
        }
    }
}

/// Limine modülleri olarak gelen ring3 testlerini yükler; ardından ilk hazır
/// sürece geçer. Modül yoksa hiçbir şey yapmaz cart curt falan
fn ring3_selftest() -> bool {
    // initramfs varsa ilk süreç /bin/init olur.
    if crate::fs::lookup_path("/bin/init").is_ok() {
        match task::spawn_path("init", "/bin/init", &["/bin/init"], &["PATH=/bin:/usr/bin", "HOME=/"]) {
            Ok(pid) => println!("ring3: init (pid {}) yuklendi", pid),
            Err(reason) => {
                println!("ring3: init -> {}", reason);
                return false;
            }
        }

        return true;
    }

    let mut spawned = 0u32;

    for module in boot::limine::modules() {
        if module.is_null() {
            continue;
        }
        let path = boot::limine::module_path(*module);
        if !path.ends_with(".elf") {
            continue;
        }

        let file = unsafe { &**module };
        if file.address.is_null() || file.size == 0 {
            continue;
        }

        let image = unsafe {
            core::slice::from_raw_parts(file.address as *const u8, file.size as usize)
        };
        let name = path.rsplit('/').next().unwrap_or(path);

        match task::spawn(name, image, &[name], &[]) {
            Ok(pid) => {
                spawned += 1;
                println!("ring3: {} (pid {}) yuklendi", name, pid);
            }
            Err(reason) => println!("ring3: {} -> {}", name, reason),
        }
    }

    spawned > 0
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("bellek ayirma basarisiz: {} bayt, hizalama {}", layout.size(), layout.align())
}
