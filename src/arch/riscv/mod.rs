use core::arch::global_asm;

mod context;

pub use context::*;

global_asm!(include_str!("switch.S"));
global_asm!(include_str!("executor_entry.S"));

extern "C" {
    pub fn switch(old: *const ContextData, new: *const ContextData);
    pub fn executor_entry();
}

pub(crate) fn cpu_id() -> u8 {
    let mut cpu_id;
    unsafe {
        asm!("mv {0}, tp", out(reg) cpu_id);
    }
    cpu_id
}

use riscv::{asm, register::sstatus};

pub(crate) fn wait_for_interrupt() {
    asm::wfi();
}

pub(crate) fn intr_on() {
    unsafe { sstatus::set_sie() };
}

pub(crate) fn intr_off() {
    unsafe { sstatus::clear_sie() };
}
