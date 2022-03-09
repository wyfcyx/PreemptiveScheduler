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
    let mut cpu_id = 0;
    // TODO
    // unsafe {
    //     asm!("mov {0}, ", out(reg) cpu_id);
    // }
    cpu_id
}
