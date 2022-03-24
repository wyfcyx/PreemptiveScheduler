use crate::context::{Context as ExecuterContext, ContextData};
use alloc::alloc::{Allocator, Global, Layout};
use core::pin::Pin;
use {
    alloc::boxed::Box,
    alloc::sync::Arc,
    core::ptr::NonNull,
    core::task::{Context, Poll},
};

use crate::executor_entry;
use crate::task_collection::TaskCollection;

#[derive(Debug, PartialEq, Eq)]
enum ExecutorState {
    STRONG,
    WEAK,       // 执行完一次future后就需要被drop
    KILLED,
    UNUSED,
}

pub struct Executor {
    task_collection: Arc<TaskCollection>,
    stack_base: usize,
    pub context: ExecuterContext,
    is_running_future: bool,
    state: ExecutorState,
}

const STACK_SIZE: usize = 4096 * 32;
const STACK_LAYOUT: Layout = Layout::new::<[u8; STACK_SIZE]>();

impl Executor {
    pub fn new(task_collection: Arc<TaskCollection>) -> Pin<Box<Self>> {
        let stack: NonNull<u8> = Global
            .allocate(STACK_LAYOUT)
            .expect("Alloction Stack Failed.")
            .cast();
        let stack_base = stack.as_ptr() as usize;
        let mut pin_executor = Pin::new(Box::new(Executor {
            task_collection,
            stack_base,
            context: ExecuterContext::default(),
            is_running_future: false,
            state: ExecutorState::UNUSED,
        }));

        let sp = pin_executor.init_stack();
        pin_executor.context.set_context(sp);

        debug!(
            "stack top 0x{:x} executor addr 0x{:x}, pgbr = 0x{:x}",
            pin_executor.context.get_sp(),
            pin_executor.context.get_pc(),
            pin_executor.context.get_pgbr(),
        );
        pin_executor
    }

    // stack layout: [executor_addr | context ]
    fn init_stack(&mut self) -> usize {
        let mut stack_top = self.stack_base + STACK_SIZE;
        let self_addr = self as *const Self as usize;
        stack_top = unsafe { push_stack(stack_top, self_addr) };
        #[cfg(target_arch = "riscv64")]
        {
            const SUM: usize = 1 << 18;
            // const SIE: usize = 1 << 1;
            let sstatus = SUM;
            stack_top = unsafe { push_stack(stack_top, sstatus) };
        }
        #[cfg(target_arch = "x86_64")]
        {
            // const IF: usize = 1 << 9;
            let rflags = 0;
            stack_top = unsafe { push_stack(stack_top, rflags) };
        }
        let context_data = ContextData::new(
            executor_entry as *const () as usize, 
            stack_top, 
            crate::pg_base_register(),
        );
        stack_top = unsafe { push_stack(stack_top, context_data) };
        stack_top
    }

    pub fn run(&mut self) {
        loop {
            let task_info = self.task_collection.take_task();
            // if task_info.is_none() {
            //     task_info = crate::runtime::steal_task_from_other_cpu()
            // }
            if let Some((task, waker, droper)) = task_info {
                let mut cx = Context::from_waker(&waker);
                // let pinned_ptr = unsafe { Pin::into_inner_unchecked(task) as *mut Task };
                // let pinned_ref = unsafe { Pin::new_unchecked(&mut *pinned_ptr) };
                self.is_running_future = true;
                // debug!("polling future");
                let ret = task.poll(&mut cx);
                // debug!("polling future over");
                self.is_running_future = false;

                if let ExecutorState::WEAK = self.state {
                    self.state = ExecutorState::KILLED;
                    return;
                }

                match ret {
                    Poll::Ready(()) => {
                        // self.task_collection.remove_task(key);
                        droper.drop_by_ref();
                    }
                    Poll::Pending => (),
                }
            } else {
                let runtime = crate::runtime::get_current_runtime();
                let has_other_task = runtime.weak_executor_num() != 0;
                drop(runtime);
                if has_other_task {
                    error!("no future to run, need yield");
                    crate::runtime::sched_yield();
                } else {
                    error!("no other tasks, wait for interrupt, intr = {}", crate::arch::intr_get());
                    crate::arch::wait_for_interrupt();
                }
                // debug!("switch back to strong executor");
                // debug!("yield over");
            }
        }
    }

    // 当前是否在运行future
    // 发生supervisor时钟中断时, 若executor在运行future, 则
    // 说明该future超时, 需要切换到另一个executor来执行其他future.
    pub fn is_running_future(&self) -> bool {
        self.is_running_future
    }

    pub fn killed(&self) -> bool {
        self.state == ExecutorState::KILLED
    }

    pub fn mark_weak(&mut self) {
        self.state = ExecutorState::WEAK;
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        unsafe {
            let stack = NonNull::<u8>::new_unchecked(self.stack_base as *mut u8);
            Global.deallocate(stack, STACK_LAYOUT);
        }
    }
}

unsafe impl Send for Executor {}
unsafe impl Sync for Executor {}

pub unsafe fn push_stack<T>(stack_top: usize, val: T) -> usize {
    let stack_top = (stack_top as *mut T).sub(1);
    *stack_top = val;
    stack_top as _
}
