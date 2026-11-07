use core::fmt::Write;
use core::panic::PanicInfo;

use crate::arch::cpu;

pub fn halt_forever() -> ! {
    loop {
        unsafe {
            core::arch::asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}

/// Panic anında yalnızca seri port güvenilir. Framebuffer konsolu yarım
/// çizilmiş olabilir, heap kilitli olabilir; bu yüzden burada hiçbir kilit
/// alınmaz ve hiçbir ayırma yapılmaz.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use crate::arch::serial::Serial;

    let mut out = SerialWriter;
    let _ = out.write_str("\n==== KERNEL PANIC ====\n");
    if let Some(location) = info.location() {
        let _ = writeln!(out, "konum: {}:{}:{}", location.file(), location.line(), location.column());
    }
    let _ = writeln!(out, "mesaj: {}", info.message());

    unsafe {
        let mut cr2: u64;
        core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack));
        let _ = writeln!(out, "cr2: {:#018x}", cr2);
        let _ = writeln!(out, "cr3: {:#018x}", cpu::read_cr3());
    }

    let _ = out.write_str("sistem durduruldu\n");
    halt_forever();
}

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        crate::arch::serial::Serial::write_bytes(text.as_bytes());
        Ok(())
    }
}
