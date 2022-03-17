#![no_std]
#![feature(allocator_api)]
#![feature(get_mut_unchecked)]
#![feature(global_asm)]
#![feature(generators, generator_trait)]
#![feature(stmt_expr_attributes)]
#![feature(atomic_mut_ptr)]
#![feature(box_into_inner)]

cfg_if::cfg_if! {
  if #[cfg(target_arch = "x86_64")] {
      #[path = "arch/x86_64/mod.rs"]
      mod arch;
  } else if #[cfg(target_arch = "riscv64")] {
      #[path = "arch/riscv64/mod.rs"]
      mod arch;
  }
}

use arch::*;

extern crate alloc;
#[macro_use]
extern crate log;

mod context;
mod executor;
mod runtime;
mod task_collection;
mod waker_page;

pub use context::{Context, ContextData};
pub use runtime::{handle_timeout, run, spawn};
