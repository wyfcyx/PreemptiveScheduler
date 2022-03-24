extern crate alloc;

use crate::waker_page::{DroperRef, WakerPage, WAKER_PAGE_SIZE};
// use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bit_iter::BitIter;
use core::cell::RefCell;
use core::ops::{Generator, GeneratorState};
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::Waker;
use spin::Mutex;
use unicycle::pin_slab::PinSlab;

use {
    alloc::boxed::Box,
    core::cell::RefMut,
    core::future::Future,
    core::pin::Pin,
    core::task::{Context, Poll},
};

use core::fmt::{Debug, Formatter, Result};

// #[allow(unused)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    BLOCKED,
    RUNNABLE,
    RUNNING,
}

pub struct Task {
    future: Mutex<Pin<Box<dyn Future<Output = ()> + Send>>>,
    inner: Mutex<TaskInner>,
}

struct TaskInner {
    priority: usize,
    state: TaskState,
}

impl core::fmt::Debug for Task {
    fn fmt(&self, f: &mut Formatter) -> Result {
        let inner = self.inner.lock();
        let mut f = f.debug_struct("X86PTE");
        f.field("priority", &inner.priority);
        f.field("state", &inner.state);
        f.finish()
    }
}

// impl Future for Task {
//     type Output = ();
//     fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
//         let mut f = self.future.lock();
//         f.as_mut().poll(cx)
//     }
// }

impl Task {
    pub fn new(future: impl Future<Output = ()> + Send + 'static, priority: usize) -> Self {
        Self {
            future: Mutex::new(Box::pin(future)),
            inner: Mutex::new(TaskInner {
                priority,
                state: TaskState::RUNNABLE,
            }),
        }
    }
    pub fn poll(&self, cx: &mut Context) -> Poll<()> {
        let mut f = self.future.lock();
        f.as_mut().poll(cx)
    }
}

pub struct FutureCollection {
    pub slab: PinSlab<Arc<Task>>,
    // pub vec: VecDeque<Key>,
    // root_waker: SharedWaker,
    pub pages: Vec<Arc<WakerPage>>,
    pub priority: usize,
}

impl FutureCollection {
    pub fn new(priority: usize) -> Self {
        Self {
            slab: PinSlab::new(),
            // vec: VecDeque::new(),
            pages: vec![],
            priority,
        }
    }
    /// Our pages hold 64 contiguous future wakers, so we can do simple arithmetic to access the
    /// correct page as well as the index within page.
    /// Given the `key` representing a future, return a reference to that page, `Arc<WakerPage>`. And
    /// the index _within_ that page (usize).
    pub fn page(&self, key: Key) -> (&Arc<WakerPage>, usize) {
        let (_, page_idx, subpage_idx) = unpack_key(key);
        (&self.pages[page_idx], subpage_idx)
    }

    /// Insert a future into our scheduler returning an integer key representing this future. This
    /// key is used to index into the slab for accessing the future.
    pub fn insert<F: Future<Output = ()> + 'static + Send>(&mut self, future: F) -> Key {
        let key = self.slab.insert(Arc::new(Task::new(future, self.priority)));
        // Add a new page to hold this future's status if the current page is filled.
        while key >= self.pages.len() * WAKER_PAGE_SIZE {
            self.pages.push(WakerPage::new());
        }
        let (page, subpage_idx) = self.page(key);
        page.initialize(subpage_idx);
        // self.vec.push_back(key);
        key
    }

    pub fn remove(&mut self, key: Key, _remove_vec: bool) {
        let (page, subpage_idx) = self.page(key);
        page.clear(subpage_idx);
        self.slab.remove(key);
        // Efficiency: remove should be called rarely
        // if remove_vec {
        //     // self.vec.retain(|&x| x != key);
        // }
    }
}

pub struct TaskCollection {
    future_collections: Vec<RefCell<FutureCollection>>,
    task_num: AtomicUsize,
    generator: Option<Mutex<Box<dyn Generator<Yield = Option<Key>, Return = ()>>>>,
}

impl TaskCollection {
    pub fn new() -> Arc<Self> {
        let mut task_collection = Arc::new(TaskCollection {
            future_collections: Vec::with_capacity(MAX_PRIORITY),
            task_num: AtomicUsize::new(0),
            generator: None,
        });
        // SAFETY: no other Arc or Weak pointers
        let tc_clone = task_collection.clone();
        let mut tc = unsafe { Arc::get_mut_unchecked(&mut task_collection) };
        for priority in 0..MAX_PRIORITY {
            tc.future_collections
                .push(RefCell::new(FutureCollection::new(priority)));
        }
        tc.generator = Some(Mutex::new(Box::new(TaskCollection::generator(tc_clone))));
        task_collection
    }

    // /// return the `key` corresponding priority, `WakerPage` and subpage_idx.
    // /// key layout:
    // /// 0-6: subpage_idx in page.
    // /// 6-59: page index in `TaskCollection.pages`.
    // /// 59-64: priority.
    // fn parse_key(&self, key: Key) -> (usize, &Arc<WakerPage>, usize) {
    //     let (priority, page_idx, subpage_idx) = unpack_key(key);
    //     let inner = self.get_mut_inner(priority);
    //     (priority, &inner.pages[page_idx], subpage_idx)
    // }

    /// 插入一个Future, 其优先级为 DEFAULT_PRIORITY
    pub fn add_task<F: Future<Output = ()> + 'static + Send>(&self, future: F) -> usize {
        self.priority_add_task(DEFAULT_PRIORITY, future)
    }

    /// remove the task correponding to the key.
    pub fn remove_task(&self, key: Key) {
        debug!("remove task key = 0x{:x?}", key);
        let mut inner = self.get_mut_inner(key >> PRIORITY_SHIFT);
        inner.remove(unmask_priority(key), true);
        self.task_num.fetch_sub(1, Ordering::Relaxed);
    }

    fn priority_add_task<F: Future<Output = ()> + 'static + Send>(
        &self,
        priority: usize,
        future: F,
    ) -> Key {
        debug_assert!(priority == DEFAULT_PRIORITY);
        let key = self.future_collections[priority]
            .borrow_mut()
            .insert(future);
        debug_assert!(key < TASK_NUM_PER_PRIORITY);
        self.task_num.fetch_add(1, Ordering::Relaxed);
        key | (priority << PRIORITY_SHIFT)
    }

    fn get_mut_inner(&self, priority: usize) -> RefMut<'_, FutureCollection> {
        self.future_collections[priority].borrow_mut()
    }

    pub fn task_num(&self) -> usize {
        self.task_num.load(Ordering::Relaxed)
    }

    pub fn take_task(&self) -> Option<(Arc<Task>, Waker, DroperRef)> {
        let mut generator = self.generator.as_ref().unwrap().lock();
        match unsafe { Pin::new_unchecked(generator.as_mut()) }.resume(()) {
            GeneratorState::Yielded(key) => {
                if let Some(key) = key {
                    let (priority, page_idx, subpage_idx) = unpack_key(key);
                    let mut inner = self.get_mut_inner(priority);
                    let waker_ref = inner.pages[page_idx].make_waker(subpage_idx);
                    let droper = waker_ref.clone();
                    let waker =
                        unsafe { Waker::from_raw(waker_ref.into_raw()) };
                    let task = inner.slab.get(unmask_priority(key)).unwrap();
                    Some((task.clone(), waker, droper))
                } else {
                    None
                }
            }
            _ => panic!("unexpected value from resume"),
        }
    }

    pub fn generator(self: Arc<Self>) -> impl Generator<Yield = Option<Key>, Return = ()> {
        static move || {
            loop {
                let priority = DEFAULT_PRIORITY;
                loop {
                    let mut found_key: Option<Key> = None;
                    let mut inner = self.get_mut_inner(priority);
                    for page_idx in 0..inner.pages.len() {
                        let page = &inner.pages[page_idx];
                        let (notified, dropped) = (page.take_notified(), page.take_dropped());
                        debug!("notified={}", notified);
                        if notified != 0 {
                            for subpage_idx in BitIter::from(notified) {
                                // the key corresponding to the task
                                found_key = Some(pack_key(priority, page_idx, subpage_idx));
                                drop(inner);
                                yield found_key;
                                inner = self.get_mut_inner(priority);
                            }
                        }
                        debug!("droped = {}", dropped);
                        if dropped != 0 {
                            for subpage_idx in BitIter::from(dropped) {
                                // the key corresponding to the task
                                let key = pack_key(priority, page_idx, subpage_idx);
                                inner.remove(key, true);
                            }
                        }
                    }
                    if found_key.is_none() {
                        break;
                    }
                }
                yield None;
            }
        }
    }
}

pub use key::*;

pub mod key {
    pub type Key = usize;
    pub const PRIORITY_SHIFT: usize = 58;
    pub const TASK_NUM_PER_PRIORITY: usize = 1 << PRIORITY_SHIFT;
    pub const MAX_PRIORITY: usize = 1 << 5;
    pub const DEFAULT_PRIORITY: usize = 4;

    pub const PAGE_INDEX_SHIFT: usize = 6;
    pub const TASK_NUM_PER_PAGE: usize = 1 << PAGE_INDEX_SHIFT;

    pub fn unpack_key(key: Key) -> (usize, usize, usize) {
        let subpage_idx = key & 0x3F;
        let page_idx = (key << 5) >> 11;
        let priority = key >> PRIORITY_SHIFT;
        (priority, page_idx, subpage_idx)
    }

    pub fn pack_key(priority: usize, page_idx: usize, subpage_idx: usize) -> Key {
        priority << PRIORITY_SHIFT | page_idx << PAGE_INDEX_SHIFT | subpage_idx
    }

    pub fn unmask_priority(key: Key) -> usize {
        key & !(0x1F << PRIORITY_SHIFT)
    }
}
