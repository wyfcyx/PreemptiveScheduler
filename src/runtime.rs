use crate::{
    arch::switch, context::Context as ThreadContext, executor::Executor, task_collection::*,
    waker_page::WakerPage,
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

    // runtime context
    context: ThreadContext,
}

impl ExecutorRuntime {
    pub fn new(cpu_id: u8) -> Self {
        let task_collection = TaskCollection::new();
        let tc_clone = task_collection.clone();
        let e = ExecutorRuntime {
            cpu_id: cpu_id,
            task_collection: task_collection,
            strong_executor: Arc::new(Executor::new(tc_clone)),
            weak_executor_vec: vec![],
            current_executor: None,
            context: ThreadContext::default(),
        };
        e
    }

    pub fn cpu_id(&self) -> u8 {
        self.cpu_id
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
    pub static ref GLOBAL_RUNTIME: [Mutex<ExecutorRuntime>; 2] = [
        Mutex::new(ExecutorRuntime::new(0)),
        Mutex::new(ExecutorRuntime::new(1))
    ];
}

// static num: usize = 0;
// // obtain a task from other cpu.
// pub(crate) fn steal_task_from_other_cpu() -> Option<(Key, Arc<WakerPage>, &'static Task, Waker)> {
//     let runtime = GLOBAL_RUNTIME
//         .iter()
//         .max_by_key(|runtime| runtime.lock().task_num())
//         .unwrap();
//     let runtime = runtime.lock();
//     trace!("fewest_task_cpu_id:{}", runtime.cpu_id());
//     // TODO: ???, SAGETY?
//     runtime.task_collection.take_task()
// }

// per-cpu scheduler.
pub fn run() {
    trace!("GLOBAL_RUNTIME.run()");
    loop {
        let mut runtime = get_current_runtime();
        let runtime_cx = runtime.context.get_context();
        let executor_cx = runtime.strong_executor.context.get_context();
        runtime.current_executor = Some(runtime.strong_executor.clone());
        // 释放保护 global_runtime 的锁
        drop(runtime);
        trace!("run strong executor");
        unsafe {
            switch(runtime_cx as _, executor_cx as _);
            // 该函数返回说明当前 strong_executor 执行的 future 超时或者主动 yield 了,
            // 需要重新创建一个 executor 执行后续的 future, 并且将
            // 新的 executor 作为 strong_executor，旧的 executor 添
            // 加到 weak_exector 中。
        }
        trace!("switch return");
        let mut runtime = get_current_runtime();

        // 只有 strong_executor 主动 yield 时, 才会执行运行 weak_executor;
        if runtime.strong_executor.is_running_future() {
            runtime.downgrade_strong_executor();
            trace!("continued");
            continue;
        }

        // 遍历全部的weak_executor
        if runtime.weak_executor_vec.is_empty() {
            drop(runtime);
            crate::wait_for_interrupt();
            continue;
        }
        trace!("run weak executor size={}", runtime.weak_executor_vec.len());
        for idx in 0..runtime.weak_executor_vec.len() {
            if let Some(executor) = &runtime.weak_executor_vec[idx] {
                if executor.killed() {
                    // TODO: 回收资源
                    continue;
                }
                let executor = executor.clone();
                let context = executor.context.get_context();
                runtime.current_executor = Some(executor);
                drop(runtime);
                unsafe {
                    // sstatus::set_sie();
                    trace!("switch weak executor");
                    switch(runtime_cx as _, context as _);
                    trace!("switch weak executor return");
                    // sstatus::clear_sie();
                }
                trace!("global locking");
                runtime = get_current_runtime();
                trace!("global locking finish");
            }
        }
        trace!("run weak executor finish");
    }
}

pub fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    trace!("spawn coroutine");
    spawn_task(future, None, None);
    trace!("spawn coroutine over");
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
    trace!("handle timeout");
    let runtime = get_current_runtime();
    if runtime
        .current_executor
        .as_ref()
        .unwrap()
        .is_running_future()
    {
        drop(runtime);
        yeild();
        trace!("handle timeout return");
    }
}

/// 运行executor.run()
#[no_mangle]
pub(crate) fn run_executor(executor_addr: usize) {
    trace!("run new executor: executor addr 0x{:x}", executor_addr);
    let mut p = unsafe { Box::from_raw(executor_addr as *mut Executor) };
    p.run();

    let runtime = get_current_runtime();
    let cx_ref = &runtime.context;
    let executor_cx = &(p.context) as *const ThreadContext as usize;
    let runtime_cx = cx_ref as *const ThreadContext as usize;
    drop(runtime);
    unsafe { crate::switch(executor_cx as _, runtime_cx as _) }
    unreachable!();
}

/// switch to runtime, which would select an appropriate executor to run.
pub(crate) fn yeild() {
    let runtime = get_current_runtime();
    let cx_ref = &runtime.context;
    let executor_cx =
        &(runtime.current_executor.as_ref().unwrap().context) as *const ThreadContext as usize;
    let runtime_cx = cx_ref as *const ThreadContext as usize;
    drop(runtime);
    unsafe { crate::switch(executor_cx as _, runtime_cx as _) }
}

/// return runtime `MutexGuard` of current cpu.
pub(crate) fn get_current_runtime() -> MutexGuard<'static, ExecutorRuntime> {
    GLOBAL_RUNTIME[crate::cpu_id() as usize].lock()
}
