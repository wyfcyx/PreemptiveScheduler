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
    raw_cpuid::CpuId::new()
        .get_feature_info()
        .unwrap()
        .initial_local_apic_id() as u8
}

use x86_64::instructions::interrupts;

pub(crate) fn wait_for_interrupt() {
    let enable = interrupts::are_enabled();
    interrupts::enable_and_hlt();
    if !enable {
        interrupts::disable();
    }
}

pub(crate) fn intr_on() {
    interrupts::enable();
}

pub(crate) fn intr_off() {
    interrupts::disable();
}
