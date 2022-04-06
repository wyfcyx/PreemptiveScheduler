use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{RawWaker, RawWakerVTable};

#[derive(Debug)]
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
#[derive(Debug)]
#[repr(align(64))]
pub struct WakerPage {
    /// A 64 element bit vector representing the futures for this page which have been notified
    /// by a wake and are ready to be polled again. The ith bit represents the ith future in the
    /// corresponding memory slab.
    notified: AtomicU64SC,
    // completed: AtomicU64SC,
    dropped: AtomicU64SC,
    // borrowed: AtomicU64SC,
}

impl WakerPage {
    pub fn new_inner() -> Self {
        WakerPage {
            notified: AtomicU64SC::new(0),
            // completed: AtomicU64SC::new(0),
            dropped: AtomicU64SC::new(0),
            // borrowed: AtomicU64SC::new(0),
        }
    }

    pub fn new() -> Arc<Self> {
        Arc::new(WakerPage::new_inner())
    }

    pub fn initialize(&self, idx: usize) {
        debug_assert!(idx < 64);
        self.notified.fetch_or(1 << idx);
        // self.completed.fetch_and(!(1 << idx));
        self.dropped.fetch_and(!(1 << idx));
    }

    pub fn mark_dropped(&self, idx: usize) {
        debug_assert!(idx < 64);
        self.dropped.fetch_or(1 << idx);
    }

    // pub fn mark_complete(&self, idx: usize) {
    //     debug_assert!(idx < 64);
    //     self.completed.fetch_or(1 << idx);
    // }

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
        // notified &= !self.completed.load();
        notified &= !self.dropped.load();
        // notified &= !self.borrowed.load();
        notified
    }

    pub fn take_dropped(&self) -> u64 {
        self.dropped.swap(0)
    }

    pub fn clear(&self, idx: usize) {
        debug_assert!(idx < 64);
        let mask = !(1 << idx);
        self.notified.fetch_and(mask);
        // self.completed.fetch_and(mask);
        self.dropped.fetch_and(mask);
    }

    pub fn make_waker(self: &Arc<Self>, idx: usize) -> WakerRef {
        WakerRef {
            page: self.clone(),
            idx,
        }
    }
}

pub type DroperRef = WakerRef;

pub struct WakerRef {
    page: Arc<WakerPage>,
    idx: usize,
}

impl WakerRef {
    pub fn wake_by_ref(&self) {
        self.page.notify(self.idx);
    }

    pub fn wake(self) {
        self.wake_by_ref();
    }

    pub fn drop_by_ref(&self) {
        self.page.mark_dropped(self.idx)
    }

    pub fn into_raw(self) -> RawWaker {
        let WakerRef { page, idx } = self;
        let ptr = Arc::into_raw(page);
        assert!((ptr as usize % 64) == 0 && idx < 64);
        RawWaker::new((ptr as usize + idx) as _, &raw_waker::VTABLE)
    }

    pub fn form_raw(data: *const ()) -> Self {
        let idx = data as usize & 0x3F;
        let ptr = data as usize & !(0x3F);
        WakerRef {
            page: unsafe { Arc::from_raw(ptr as _) },
            idx,
        }
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

mod raw_waker {
    use super::*;

    fn waker_ref_clone(data: *const ()) -> RawWaker {
        let waker = WakerRef::form_raw(data);
        let cw = waker.clone();
        core::mem::forget(waker);
        cw.into_raw()
    }

    fn waker_ref_wake(data: *const ()) {
        let waker = WakerRef::form_raw(data);
        waker.wake();
    }

    fn waker_ref_wake_by_ref(data: *const ()) {
        let waker = WakerRef::form_raw(data);
        waker.wake_by_ref();
        core::mem::forget(waker);
    }

    fn waker_ref_drop(data: *const ()) {
        let waker = WakerRef::form_raw(data);
        drop(waker)
    }

    pub(super) const VTABLE: RawWakerVTable = RawWakerVTable::new(
        waker_ref_clone,
        waker_ref_wake,
        waker_ref_wake_by_ref,
        waker_ref_drop,
    );
}
