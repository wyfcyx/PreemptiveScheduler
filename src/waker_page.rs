use alloc::boxed::Box;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{RawWaker, RawWakerVTable};

pub struct AtomicU64SC(AtomicU64);
pub const WAKER_PAGE_SIZE: usize = 64;

impl AtomicU64SC {
    #[inline(always)]
    #[allow(unused)]
    pub fn new(val: u64) -> Self {
        AtomicU64SC(AtomicU64::new(val))
    }

    #[inline(always)]
    #[allow(unused)]
    pub fn fetch_or(&self, val: u64) {
        self.0.fetch_or(val, Ordering::SeqCst);
    }

    #[inline(always)]
    #[allow(unused)]
    pub fn fetch_and(&self, val: u64) {
        self.0.fetch_and(val, Ordering::SeqCst);
    }

    #[inline(always)]
    #[allow(unused)]
    pub fn fetch_add(&self, val: u64) -> u64 {
        self.0.fetch_add(val, Ordering::SeqCst)
    }

    #[inline(always)]
    #[allow(unused)]
    pub fn fetch_sub(&self, val: u64) -> u64 {
        self.0.fetch_sub(val, Ordering::SeqCst)
    }

    #[inline(always)]
    #[allow(unused)]
    pub fn load(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    #[inline(always)]
    #[allow(unused)]
    pub fn swap(&self, val: u64) -> u64 {
        self.0.swap(val, Ordering::SeqCst)
    }

    #[inline(always)]
    #[allow(unused)]
    pub fn as_mut_ptr(&mut self) -> *mut u64 {
        self.0.as_mut_ptr()
    }
}

// pub struct SharedWaker(Arc<Waker>);

// impl Clone for SharedWaker {
//     fn clone(&self) -> Self {
//         Self(self.0.clone())
//     }
// }

// impl Default for SharedWaker {
//     fn default() -> Self {
//         Self::new()
//     }
// }

// impl SharedWaker {
//     #[allow(unused)]
//     pub fn new() -> Self {
//         Self(Arc::new(AtomicWaker::new()))
//     }

//     #[allow(unused)]
//     pub fn wake(&self) {
//         self.0.wake();
//     }
// }

/// A page is used by the scheduler to hold the current status of 64 different futures in the
/// scheduler. So we use 64bit integers where the ith bit represents the ith future. Pages are
/// arranged by the scheduler in a `pages` vector of pages which grows as needed allocating space
/// for 64 more futures at a time.
#[repr(align(64))]
pub struct WakerPage {
    /// A 64 element bit vector representing the futures for this page which have been notified
    /// by a wake and are ready to be polled again. The ith bit represents the ith future in the
    /// corresponding memory slab.
    notified: AtomicU64SC,
    completed: AtomicU64SC,
    dropped: AtomicU64SC,
    borrowed: AtomicU64SC,
}

impl WakerPage {
    pub fn new_inner() -> Self {
        WakerPage {
            notified: AtomicU64SC::new(0),
            completed: AtomicU64SC::new(0),
            dropped: AtomicU64SC::new(0),
            borrowed: AtomicU64SC::new(0),
        }
    }

    pub fn new() -> Arc<Self> {
        Arc::new(WakerPage::new_inner())
    }

    pub fn initialize(&self, ix: usize) {
        debug_assert!(ix < 64);
        self.notified.fetch_or(1 << ix);
        self.completed.fetch_and(!(1 << ix));
        self.dropped.fetch_and(!(1 << ix));
    }

    pub fn mark_dropped(&self, ix: usize) {
        debug_assert!(ix < 64);
        self.dropped.fetch_or(1 << ix);
    }

    pub fn mark_complete(&self, ix: usize) {
        debug_assert!(ix < 64);
        self.completed.fetch_or(1 << ix);
    }

    pub fn notify(&self, offset: usize) {
        debug_assert!(offset < 64);
        self.notified.fetch_or(1 << offset);
    }

    /// Return a bit vector representing the futures in this page which are ready to be
    /// polled again.
    pub fn take_notified(&self) -> u64 {
        // Unset all ready bits, since spurious notifications for completed futures would lead
        // us to poll them after completion.
        let mut notified = self.notified.swap(0);
        notified &= !self.completed.load();
        notified &= !self.dropped.load();
        notified &= !self.borrowed.load();
        notified
    }

    pub fn take_dropped(&self) -> u64 {
        self.dropped.swap(0)
    }

    pub fn clear(&self, ix: usize) {
        debug_assert!(ix < 64);
        let mask = !(1 << ix);
        self.notified.fetch_and(mask);
        self.completed.fetch_and(mask);
        self.dropped.fetch_and(mask);
    }
}

pub struct WakerRef {
    page: Arc<WakerPage>,
    idx: usize,
}

impl WakerRef {
    fn wake_by_ref(&self) {
        self.page.notify(self.idx);
    }

    fn wake(self) {
        self.wake_by_ref();
    }

    fn into_raw(self) -> RawWaker {
        let ptr = &self as *const WakerRef as *const ();
        core::mem::forget(self);
        RawWaker::new(ptr, &VTABLE)
    }
}

impl Clone for WakerRef {
    fn clone(&self) -> Self {
        WakerRef {
            page: self.page.clone(),
            idx: self.idx,
        }
    }
}

fn waker_ref_clone(ptr: *const ()) -> RawWaker {
    let waker = unsafe { &*(ptr as *const WakerRef) };
    let clone_waker = Box::new(WakerRef {
        page: waker.page.clone(),
        idx: waker.idx,
    });
    Box::into_inner(clone_waker).into_raw()
}

fn waker_ref_wake(ptr: *const ()) {
    let waker = unsafe { &*(ptr as *const WakerRef) };
    waker.wake()
}

fn waker_ref_wake_by_ref(ptr: *const ()) {
    let waker = unsafe { &*(ptr as *const WakerRef) };
    waker.wake()
}

fn waker_ref_drop(ptr: *const ()) {
    let waker = unsafe { &*(ptr as *const WakerRef) };
    drop(waker)
}

const VTABLE: RawWakerVTable = RawWakerVTable::new(
    waker_ref_clone,
    waker_ref_wake,
    waker_ref_wake_by_ref,
    waker_ref_drop,
);
