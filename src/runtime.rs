use crate::{
    context::ContextData, executor::Executor, task_collection::*,
    waker_page::DroperRef,
};
use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use core::{future::Future, pin::Pin, task::Waker};
use lazy_static::*;
use spin::{Mutex, MutexGuard};

pub struct ExecutorRuntime {
    // runtime only run on this cpu
    cpu_id: u8,

    // 只会在一个 core 上运行，不需要考虑同步问题
    task_collection: Arc<TaskCollection>,

    // 通过 force_switch_future 会将 strong_executor 降级为 weak_executor
    strong_executor: Arc<Pin<Box<Executor>>>,

    // 该 executor 在执行完一次后就会被 drop
    weak_executor_vec: Vec<Option<Arc<Pin<Box<Executor>>>>>,

    // 当前正在执行的 executor
    current_executor: Option<Arc<Pin<Box<Executor>>>>,

    // runtime context data
    context_data: ContextData,
}

impl ExecutorRuntime {
    pub fn new(cpu_id: u8) -> Self {
        let task_collection = TaskCollection::new();
        let tc_clone = task_collection.clone();
        ExecutorRuntime {
            cpu_id,
            task_collection: task_collection,
            strong_executor: Arc::new(Executor::new(tc_clone)),
            weak_executor_vec: vec![],
            current_executor: None,
            context_data: ContextData::default(),
        }
    }

    pub fn cpu_id(&self) -> u8 {
        self.cpu_id
    }

    pub(crate) fn weak_executor_num(&self) -> usize {
        self.weak_executor_vec.len()
    }

    fn add_weak_executor(&mut self, weak_executor: Arc<Pin<Box<Executor>>>) {
        self.weak_executor_vec.push(Some(weak_executor));
    }

    fn downgrade_strong_executor(&mut self) {
        // SAFETY: 只会在一个 core 上运行，不需要考虑同步问题
        let mut old = self.strong_executor.clone();
        unsafe {
            Arc::get_mut_unchecked(&mut old).mark_weak();
        }
        self.add_weak_executor(old);
        self.strong_executor = Arc::new(Executor::new(self.task_collection.clone()));
    }

    // return task number of current cpu.
    fn task_num(&self) -> usize {
        self.task_collection.task_num()
    }

    // 添加一个task，它的初始状态是 notified，也就是说它可以被执行.
    fn add_task<F: Future<Output = ()> + 'static + Send>(&self, priority: usize, future: F) -> Key {
        debug_assert!(priority < MAX_PRIORITY);
        self.task_collection.add_task(future)
    }

    fn remove_task(&self, key: Key) {
        self.task_collection.remove_task(key)
    }

    fn get_context(&self) -> usize {
        &self.context_data as *const ContextData as usize
    }
}

impl Drop for ExecutorRuntime {
    fn drop(&mut self) {
        panic!("drop executor runtime!!!!");
    }
}

// SAFETY: 只会在一个 core 上运行，不需要考虑同步问题
unsafe impl Send for ExecutorRuntime {}
unsafe impl Sync for ExecutorRuntime {}

// TODO: more elegent?
lazy_static! {
    pub static ref GLOBAL_RUNTIME: [Mutex<ExecutorRuntime>; 4] = [
        Mutex::new(ExecutorRuntime::new(0)),
        Mutex::new(ExecutorRuntime::new(1)),
        Mutex::new(ExecutorRuntime::new(2)),
        Mutex::new(ExecutorRuntime::new(3))
    ];
}

// // obtain a task from other cpu.
// pub(crate) fn steal_task_from_other_cpu() -> Option<(Arc<Task>, Waker, DroperRef)> {
//     let runtime = GLOBAL_RUNTIME
//         .iter()
//         .max_by_key(|runtime| runtime.lock().task_num())
//         .unwrap();
//     let runtime = runtime.lock();
//     debug!("most_task_cpu_id:{}", runtime.cpu_id());
//     // TODO: ???, SAGETY?
//     runtime.task_collection.take_task()
// }

// per-cpu scheduler.
pub fn run_until_idle() {
    debug!("GLOBAL_RUNTIME.run()");
    crate::intr_off();  // runtime can't be interrupted
    loop {
        let mut runtime = get_current_runtime();
        let runtime_cx = runtime.get_context();
        let executor_cx = runtime.strong_executor.context.get_context();
        
        runtime.current_executor = Some(runtime.strong_executor.clone());
        // 释放保护 global_runtime 的锁
        drop(runtime);
        trace!("run strong executor");
        unsafe {
            crate::switch(runtime_cx as _, executor_cx as _);
            // 该函数返回说明当前 strong_executor 执行的 future 超时或者主动 yield 了,
            // 需要重新创建一个 executor 执行后续的 future, 并且将
            // 新的 executor 作为 strong_executor，旧的 executor 添
            // 加到 weak_exector 中。
        }
        // trace!("runtime switch return");
        let mut runtime = get_current_runtime();
        // 只有 strong_executor 主动 yield 时, 才会执行运行 weak_executor;
        if runtime.strong_executor.is_running_future() {
            runtime.downgrade_strong_executor();
            continue;
        }

        // 遍历全部的 weak_executor
        if runtime.weak_executor_vec.is_empty() {
            drop(runtime);
            continue;
        }
        // error!("run weak executor size={}", runtime.weak_executor_vec.len());
        runtime
            .weak_executor_vec
            .retain(|executor| executor.is_some() && !executor.as_ref().unwrap().killed());
        for idx in 0..runtime.weak_executor_vec.len() {
            if let Some(executor) = &runtime.weak_executor_vec[idx] {
                if executor.killed() {
                    continue;
                }
                let executor = executor.clone();
                let executor_ctx = executor.context.get_context();
                runtime.current_executor = Some(executor);
                drop(runtime);
                // trace!("switch weak executor");
                switch(runtime_cx as _, executor_ctx as _);
                // trace!("switch weak executor return");
                runtime = get_current_runtime();
            }
        }
        // trace!("run weak executor finish");
    }
}

pub fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    // trace!("spawn coroutine");
    spawn_task(future, None, Some(crate::cpu_id() as _));
    // trace!("spawn coroutine over");
}

/// Spawn a coroutine with `priority` and `cpu_id`
/// Default priority: DEFAULT_PRIORITY
/// Default cpu_id: the cpu with fewest number of tasks
pub fn spawn_task(
    future: impl Future<Output = ()> + Send + 'static,
    priority: Option<usize>,
    cpu_id: Option<usize>,
) {
    let priority = priority.unwrap_or(DEFAULT_PRIORITY);
    let runtime = if let Some(cpu_id) = cpu_id {
        &GLOBAL_RUNTIME[cpu_id]
    } else {
        GLOBAL_RUNTIME
            .iter()
            .min_by_key(|runtime| runtime.lock().task_num())
            .unwrap()
    };
    runtime.lock().add_task(priority, future);
}

/// check whether the running coroutine of current cpu time out, if yes, we will
/// switch to currrent cpu runtime that would create a new executor to run other
/// coroutines.
pub fn handle_timeout() {
    debug!("handle timeout");
    // let runtime = get_current_runtime();
    // if runtime
    //     .current_executor
    //     .as_ref()
    //     .unwrap()
    //     .is_running_future()
    // {
    //     drop(runtime);
    //     sched_yield();
    //     debug!("handle timeout return");
    // }
}

/// 运行executor.run()
#[no_mangle]
pub(crate) fn run_executor(executor_addr: usize) {
    let mut p = unsafe { Box::from_raw(executor_addr as *mut Executor) };
    crate::arch::intr_on(); // executor can be intrrupted
    p.run();
    // Weak executor may return
    let runtime = get_current_runtime();
    let executor_cx = p.context.get_context();
    let runtime_cx = runtime.get_context();
    drop(runtime);
    switch(executor_cx as _, runtime_cx as _);
    unreachable!();
}

/// switch to runtime, which would select an appropriate executor to run.
pub fn sched_yield() {
    let runtime = get_current_runtime();
    if let Some(executor) = runtime.current_executor.as_ref() {
        let executor_cx = executor.context.get_context();
        let runtime_cx = runtime.get_context();
        drop(runtime);
        switch(executor_cx as _, runtime_cx as _);
        // debug!("switch back to executor");
    }
}

pub(crate) fn switch(from_ctx: usize, to_ctx: usize) {
    let intr_enable = crate::intr_get();
    if intr_enable {
        crate::intr_off();
    }
    unsafe { crate::switch(from_ctx as _, to_ctx as _); }
    if intr_enable {
        crate::intr_on();
    }
}

/// return runtime `MutexGuard` of current cpu.
pub(crate) fn get_current_runtime() -> MutexGuard<'static, ExecutorRuntime> {
    GLOBAL_RUNTIME[crate::cpu_id() as usize].lock()
}
