use crate::BaseScheduler;
use core::fmt::Debug;
use core::ops::Deref;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicIsize, Ordering};
use utils::LockFreeDeque;

/// A task wrapper for the [`RRScheduler`].
///
/// It add a time slice counter to use in round-robin scheduling.
pub struct RRTask<T, const MAX_TIME_SLICE: usize> {
    inner: T,
    time_slice: AtomicIsize,
}

impl<T, const S: usize> RRTask<T, S> {
    /// Creates a new [`RRTask`] from the inner task struct.
    pub const fn new(inner: T) -> Self {
        Self {
            inner,
            time_slice: AtomicIsize::new(S as isize),
        }
    }

    fn time_slice(&self) -> isize {
        self.time_slice.load(Ordering::Acquire)
    }

    fn reset_time_slice(&self) {
        self.time_slice.store(S as isize, Ordering::Release);
    }

    /// Returns a reference to the inner task struct.
    pub const fn inner(&self) -> &T {
        &self.inner
    }
}

impl<T, const S: usize> Deref for RRTask<T, S> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[repr(C)]
pub struct RRTaskRef<T, const S: usize> {
    inner: NonNull<RRTask<T, S>>,
    clone_fn: Option<extern "C" fn(*const RRTask<T, S>)>,
    weak_clone_fn: Option<extern "C" fn(*const RRTask<T, S>) -> WeakRRTaskRef<T, S>>,
    drop_fn: Option<extern "C" fn(*const RRTask<T, S>)>,
    strong_count_fn: Option<extern "C" fn(*const RRTask<T, S>) -> usize>,
}

unsafe impl<T, const S: usize> Send for RRTaskRef<T, S> {}
unsafe impl<T, const S: usize> Sync for RRTaskRef<T, S> {}

impl<T, const S: usize> Clone for RRTaskRef<T, S> {
    fn clone(&self) -> Self {
        let ptr = self.inner.as_ptr();
        (self.clone_fn.unwrap())(ptr);
        Self {
            inner: self.inner.clone(),
            clone_fn: self.clone_fn.clone(),
            weak_clone_fn: self.weak_clone_fn.clone(),
            drop_fn: self.drop_fn.clone(),
            strong_count_fn: self.strong_count_fn.clone(),
        }
    }
}

impl<T, const S: usize> Drop for RRTaskRef<T, S> {
    fn drop(&mut self) {
        let ptr = self.inner.as_ptr();
        (self.drop_fn.unwrap())(ptr);
    }
}

impl<T, const S: usize> RRTaskRef<T, S> {
    pub const EMPTY: Self = Self {
        inner: NonNull::dangling(),
        clone_fn: None,
        weak_clone_fn: None,
        drop_fn: None,
        strong_count_fn: None,
    };

    pub fn new(
        inner: NonNull<RRTask<T, S>>,
        clone_fn: extern "C" fn(*const RRTask<T, S>),
        weak_clone_fn: extern "C" fn(*const RRTask<T, S>) -> WeakRRTaskRef<T, S>,
        drop_fn: extern "C" fn(*const RRTask<T, S>),
        strong_count_fn: extern "C" fn(*const RRTask<T, S>) -> usize,
    ) -> Self {
        Self {
            inner,
            clone_fn: Some(clone_fn),
            weak_clone_fn: Some(weak_clone_fn),
            drop_fn: Some(drop_fn),
            strong_count_fn: Some(strong_count_fn),
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.inner.as_ptr() == other.inner.as_ptr()
    }

    pub fn strong_count(&self) -> usize {
        (self.strong_count_fn.unwrap())(self.inner.as_ptr())
    }

    pub fn weak_clone(&self) -> WeakRRTaskRef<T, S> {
        (self.weak_clone_fn.unwrap())(self.inner.as_ptr())
    }
}

impl<T, const S: usize> Deref for RRTaskRef<T, S> {
    type Target = RRTask<T, S>;
    fn deref(&self) -> &Self::Target {
        unsafe { self.inner.as_ref() }
    }
}

impl<T: Debug, const S: usize> Debug for RRTask<T, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RRTask")
            .field("inner", self.inner())
            .finish()
    }
}

impl<T: Debug, const S: usize> Debug for RRTaskRef<T, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RRTaskRef").field("inner", self).finish()
    }
}

#[repr(C)]
pub struct WeakRRTaskRef<T, const S: usize> {
    inner: NonNull<RRTask<T, S>>,
}

impl<T, const S: usize> WeakRRTaskRef<T, S> {
    pub fn new(inner: NonNull<RRTask<T, S>>) -> Self {
        Self { inner }
    }
}

impl<T, const S: usize> Deref for WeakRRTaskRef<T, S> {
    type Target = RRTask<T, S>;
    fn deref(&self) -> &Self::Target {
        unsafe { self.inner.as_ref() }
    }
}

/// A simple [Round-Robin] (RR) preemptive scheduler.
///
/// It's very similar to the [`FifoScheduler`], but every task has a time slice
/// counter that is decremented each time a timer tick occurs. When the current
/// task's time slice counter reaches zero, the task is preempted and needs to
/// be rescheduled.
///
/// Unlike [`FifoScheduler`], it uses [`VecDeque`] as the ready queue. So it may
/// take O(n) time to remove a task from the ready queue.
///
/// [Round-Robin]: https://en.wikipedia.org/wiki/Round-robin_scheduling
/// [`FifoScheduler`]: crate::FifoScheduler
pub struct RRScheduler<T, const MAX_TIME_SLICE: usize, const CAPACITY: usize> {
    ready_queue: LockFreeDeque<RRTaskRef<T, MAX_TIME_SLICE>, CAPACITY>,
}

impl<T, const S: usize, const CAPACITY: usize> RRScheduler<T, S, CAPACITY> {
    /// Creates a new empty [`RRScheduler`].
    pub const fn new() -> Self {
        Self {
            ready_queue: LockFreeDeque::new(),
        }
    }
    /// get the name of scheduler
    pub fn scheduler_name() -> &'static str {
        "Round-robin"
    }
}

impl<T, const S: usize, const CAPACITY: usize> BaseScheduler for RRScheduler<T, S, CAPACITY> {
    type SchedItem = RRTaskRef<T, S>;

    fn init(&mut self) {}

    fn add_task(&self, task: Self::SchedItem) {
        let _ = self.ready_queue.push_back(task);
    }

    fn pick_next_task(&self) -> Option<Self::SchedItem> {
        self.ready_queue.pop_front()
    }

    fn put_prev_task(&self, prev: Self::SchedItem, preempt: bool) {
        if prev.time_slice() > 0 && preempt {
            let _ = self.ready_queue.push_front(prev);
        } else {
            prev.reset_time_slice();
            let _ = self.ready_queue.push_back(prev);
        }
    }

    fn task_tick(&self, current: &Self::SchedItem) -> bool {
        let old_slice = current.time_slice.fetch_sub(1, Ordering::Release);
        old_slice <= 1
    }

    fn set_priority(&self, _task: &Self::SchedItem, _prio: isize) -> bool {
        false
    }
}
