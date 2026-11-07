//! 8259 PIC. APIC'e geçince bu modül devre dışı kalacak.

use super::port::{inb, outb};

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

const ICW1_INIT: u8 = 0x11; // init + ICW4
const ICW4_8086: u8 = 0x01;
const EOI: u8 = 0x20;

pub const IRQ_TIMER: u8 = 0;
pub const IRQ_KEYBOARD: u8 = 1;
pub const IRQ_SERIAL: u8 = 4;

pub fn init() {
    unsafe {
        let mask1 = inb(PIC1_DATA);
        let mask2 = inb(PIC2_DATA);

        outb(PIC1_COMMAND, ICW1_INIT);
        outb(PIC2_COMMAND, ICW1_INIT);

        // Vektör ofsetleri: 0x20 ve 0x28. CPU istisnaları 0x00-0x1F'de kalır.
        outb(PIC1_DATA, 0x20);
        outb(PIC2_DATA, 0x28);

        outb(PIC1_DATA, 0x04); // slave, IRQ2 üzerinde
        outb(PIC2_DATA, 0x02); // slave kimliği

        outb(PIC1_DATA, ICW4_8086);
        outb(PIC2_DATA, ICW4_8086);

        let _ = mask1;
        let _ = mask2;

        // Önce hepsini maskele, sonra tek tek aç.
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}

pub fn unmask(irq: u8) {
    unsafe {
        if irq < 8 {
            let mask = inb(PIC1_DATA) & !(1 << irq);
            outb(PIC1_DATA, mask);
        } else if irq < 16 {
            let mask = inb(PIC2_DATA) & !(1 << (irq - 8));
            outb(PIC2_DATA, mask);
            // Slave'e giden IRQ2 hattını da aç.
            let mask1 = inb(PIC1_DATA) & !(1 << 2);
            outb(PIC1_DATA, mask1);
        }
    }
}

pub fn mask(irq: u8) {
    unsafe {
        if irq < 8 {
            let mask = inb(PIC1_DATA) | (1 << irq);
            outb(PIC1_DATA, mask);
        } else if irq < 16 {
            let mask = inb(PIC2_DATA) | (1 << (irq - 8));
            outb(PIC2_DATA, mask);
        }
    }
}

pub fn end_of_interrupt(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_COMMAND, EOI);
        }
        outb(PIC1_COMMAND, EOI);
    }
}

pub fn in_service() -> u16 {
    unsafe {
        outb(PIC1_COMMAND, 0x0B);
        outb(PIC2_COMMAND, 0x0B);
        let low = inb(PIC1_COMMAND) as u16;
        let high = inb(PIC2_COMMAND) as u16;
        (high << 8) | low
    }
}
