use crate::BaseScheduler;
use core::ops::Deref;
use core::ptr::NonNull;
use heapless::mpmc::MpMcQueue;

pub struct FifoTask<T> {
    inner: T,
}

impl<T> FifoTask<T> {
    /// Creates a new [`RRTask`] from the inner task struct.
    pub const fn new(inner: T) -> Self {
        Self { inner }
    }

    /// Returns a reference to the inner task struct.
    pub const fn inner(&self) -> &T {
        &self.inner
    }
}

impl<T> Deref for FifoTask<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[repr(transparent)]
#[derive(Copy)]
pub struct FiFoTaskRef<T> {
    inner: NonNull<FifoTask<T>>,
}

impl<T> Clone for FiFoTaskRef<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner }
    }
}

impl<T> FiFoTaskRef<T> {
    pub const EMPTY: Self = Self {
        inner: NonNull::dangling(),
    };

    #[allow(unused)]
    pub fn new(inner: NonNull<FifoTask<T>>) -> Self {
        Self { inner }
    }

    #[allow(unused)]
    pub fn as_ref(&self) -> &FifoTask<T> {
        unsafe { self.inner.as_ref() }
    }

    #[allow(unused)]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.inner.as_ptr() == other.inner.as_ptr()
    }
}

/// A simple FIFO (First-In-First-Out) cooperative scheduler.
///
/// When a task is added to the scheduler, it's placed at the end of the ready
/// queue. When picking the next task to run, the head of the ready queue is
/// taken.
///
/// As it's a cooperative scheduler, it does nothing when the timer tick occurs.
///
/// It internally uses a linked list as the ready queue.
pub struct FifoScheduler<T, const CAPACITY: usize> {
    ready_queue: MpMcQueue<FiFoTaskRef<T>, CAPACITY>,
}

impl<T, const CAPACITY: usize> FifoScheduler<T, CAPACITY> {
    /// Creates a new empty [`FifoScheduler`].
    pub const fn new() -> Self {
        Self {
            ready_queue: MpMcQueue::new(),
        }
    }
    /// get the name of scheduler
    pub fn scheduler_name() -> &'static str {
        "FIFO"
    }
}

impl<T, const CAPACITY: usize> BaseScheduler for FifoScheduler<T, CAPACITY> {
    type SchedItem = FiFoTaskRef<T>;

    fn init(&mut self) {}

    fn add_task(&self, task: Self::SchedItem) {
        let _ = self.ready_queue.enqueue(task);
    }

    fn pick_next_task(&self) -> Option<Self::SchedItem> {
        self.ready_queue.dequeue()
    }

    fn put_prev_task(&self, prev: Self::SchedItem, _preempt: bool) {
        let _ = self.ready_queue.enqueue(prev);
    }

    fn task_tick(&self, _current: &Self::SchedItem) -> bool {
        false // no reschedule
    }

    fn set_priority(&self, _task: &Self::SchedItem, _prio: isize) -> bool {
        false
    }
}
