use crate::task_collection::Task;
use crate::waker_page::WAKER_PAGE_SIZE;
use crate::{
    context::{Context as ExecuterContext, ContextData},
    waker_page::WakerPageRef,
};
use alloc::alloc::{Allocator, Global, Layout};
use core::matches;
use core::pin::Pin;
use core::task::Waker;
use riscv::register::sstatus;
use {
    alloc::boxed::Box,
    alloc::sync::Arc,
    core::future::Future,
    core::ptr::NonNull,
    core::task::{Context, Poll},
};

use crate::executor_entry;
use crate::task_collection::TaskCollection;

enum ExecutorState {
    STRONG,
    WEAK, // 执行完一次future后就需要被drop
    KILLED,
    UNUSED,
}

pub struct Executor {
    task_collection: Arc<TaskCollection<Task>>,
    stack_base: usize,
    pub context: ExecuterContext,
    is_running_future: bool,
    state: ExecutorState,
}

const STACK_SIZE: usize = 4096 * 32;

impl Executor {
    pub fn new(task_collection: Arc<TaskCollection<Task>>) -> Pin<Box<Self>> {
        unsafe {
            let stack_base: NonNull<u8> = Global
                .allocate(Layout::new::<[u8; STACK_SIZE]>())
                .expect("Stack Alloction Failed.")
                .cast();
            let stack_base = stack_base.as_ptr() as usize;
            let mut pin_executor = Pin::new(Box::new(Executor {
                task_collection: task_collection,
                stack_base: stack_base,
                context: ExecuterContext::default(),
                is_running_future: false,
                state: ExecutorState::UNUSED,
            }));

            pin_executor.context.set_context(pin_executor.init_stack());

            trace!(
                "stack top 0x{:x} executor addr 0x{:x}",
                pin_executor.context.get_sp(),
                pin_executor.context.get_pc(),
            );
            pin_executor
        }
    }

    // stack layout: [executor_addr | context ]
    fn init_stack(&mut self) -> usize {
        let mut stack_top = self.stack_base + STACK_SIZE;
        let self_addr = self as *const Self as usize;
        stack_top = push_stack(stack_top, self_addr);
        let context_data = ContextData::new(executor_entry as *const () as usize, stack_top);
        stack_top = push_stack(stack_top, context_data);
        stack_top
    }

    fn run_task(
        &mut self,
        key: u64,
        page_ref: WakerPageRef,
        pinned_task_ref: Pin<&mut Task>,
        waker: Waker,
    ) {
        let mut cx = Context::from_waker(&waker);
        let pinned_ptr = unsafe { Pin::into_inner_unchecked(pinned_task_ref) as *mut Task };
        let pinned_ref = unsafe { Pin::new_unchecked(&mut *pinned_ptr) };
        unsafe {
            sstatus::set_sie();
        } // poll future时允许中断
        self.is_running_future = true;

        trace!("polling future");
        let ret = { Future::poll(pinned_ref, &mut cx) };
        trace!("polling future over");
        unsafe {
            sstatus::clear_sie();
        } // 禁用中断
        self.is_running_future = false;

        if let ExecutorState::WEAK = self.state {
            info!("weak executor finish poll future, need killed");
            self.state = ExecutorState::KILLED;
            return;
        }

        match ret {
            Poll::Ready(()) => {
                // self.task_collection.remove_task(key);
                page_ref.mark_dropped((key % (WAKER_PAGE_SIZE as u64)) as usize);
            }
            Poll::Pending => (),
        }
    }

    pub fn run(&mut self) {
        trace!("new executor run, addr={:x}", self as *const _ as usize);
        loop {
            if let Some((key, page_ref, pinned_task_ref, waker)) =
                unsafe { Arc::get_mut_unchecked(&mut self.task_collection).take_task() }
            {
                self.run_task(key, page_ref, pinned_task_ref, waker)
            } else if let Some((key, page_ref, pinned_task_ref, waker)) =
                crate::runtime::steal_task_from_other_cpu()
            {
                trace!("task from other cpu");
                self.run_task(key, page_ref, pinned_task_ref, waker)
            } else {
                // trace!("no future to run, need yield");
                crate::runtime::yeild();
                // trace!("yield over");
                // unsafe {
                //     crate::wait_for_interrupt();
                // }
            }
        }
    }

    // 当前是否在运行future
    // 发生supervisor时钟中断时, 若executor在运行future, 则
    // 说明该future超时, 需要切换到另一个executor来执行其他future.
    pub fn is_running_future(&self) -> bool {
        return self.is_running_future;
    }

    pub fn killed(&self) -> bool {
        return matches!(self.state, ExecutorState::KILLED);
    }

    pub fn mark_weak(&mut self) {
        self.state = ExecutorState::WEAK;
    }
}

unsafe impl Send for Executor {}
unsafe impl Sync for Executor {}

pub unsafe fn push_stack<T>(stack_top: usize, val: T) -> usize {
    let stack_top = (stack_top as *mut T).sub(1);
    *stack_top = val;
    stack_top as _
}
