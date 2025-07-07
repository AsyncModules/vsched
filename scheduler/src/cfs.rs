use crossbeam::atomic::AtomicCell;

use crate::BaseScheduler;
use core::fmt::Debug;
use core::ops::Deref;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicIsize, Ordering};
use utils::LockFreeBTreeMap;

/// task for CFS
pub struct CFSTask<T> {
    inner: T,
    init_vruntime: AtomicIsize,
    delta: AtomicIsize,
    nice: AtomicIsize,
    id: AtomicIsize,
}

// https://elixir.bootlin.com/linux/latest/source/include/linux/sched/prio.h

const NICE_RANGE_POS: usize = 19; // MAX_NICE in Linux
const NICE_RANGE_NEG: usize = 20; // -MIN_NICE in Linux, the range of nice is [MIN_NICE, MAX_NICE]

// https://elixir.bootlin.com/linux/latest/source/kernel/sched/core.c

const NICE2WEIGHT_POS: [isize; NICE_RANGE_POS + 1] = [
    1024, 820, 655, 526, 423, 335, 272, 215, 172, 137, 110, 87, 70, 56, 45, 36, 29, 23, 18, 15,
];
const NICE2WEIGHT_NEG: [isize; NICE_RANGE_NEG + 1] = [
    1024, 1277, 1586, 1991, 2501, 3121, 3906, 4904, 6100, 7620, 9548, 11916, 14949, 18705, 23254,
    29154, 36291, 46273, 56483, 71755, 88761,
];

impl<T> CFSTask<T> {
    /// new with default values
    pub const fn new(inner: T) -> Self {
        Self {
            inner,
            init_vruntime: AtomicIsize::new(0_isize),
            delta: AtomicIsize::new(0_isize),
            nice: AtomicIsize::new(0_isize),
            id: AtomicIsize::new(0_isize),
        }
    }

    fn get_weight(&self) -> isize {
        let nice = self.nice.load(Ordering::Acquire);
        if nice >= 0 {
            NICE2WEIGHT_POS[nice as usize]
        } else {
            NICE2WEIGHT_NEG[(-nice) as usize]
        }
    }

    #[allow(unused)]
    fn get_id(&self) -> isize {
        self.id.load(Ordering::Acquire)
    }

    fn get_vruntime(&self) -> isize {
        if self.nice.load(Ordering::Acquire) == 0 {
            self.init_vruntime.load(Ordering::Acquire) + self.delta.load(Ordering::Acquire)
        } else {
            self.init_vruntime.load(Ordering::Acquire)
                + self.delta.load(Ordering::Acquire) * 1024 / self.get_weight()
        }
    }

    fn set_vruntime(&self, v: isize) {
        self.init_vruntime.store(v, Ordering::Release);
    }

    // Simple Implementation: no change in vruntime.
    // Only modifying priority of current process is supported currently.
    fn set_priority(&self, nice: isize) {
        let current_init_vruntime = self.get_vruntime();
        self.init_vruntime
            .store(current_init_vruntime, Ordering::Release);
        self.delta.store(0, Ordering::Release);
        self.nice.store(nice, Ordering::Release);
    }

    fn set_id(&self, id: isize) {
        self.id.store(id, Ordering::Release);
    }

    fn task_tick(&self) {
        self.delta.fetch_add(1, Ordering::Release);
    }

    /// Returns a reference to the inner task struct.
    pub const fn inner(&self) -> &T {
        &self.inner
    }
}

impl<T> Deref for CFSTask<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[repr(transparent)]
#[derive(Copy)]
pub struct CFSTaskRef<T> {
    inner: NonNull<CFSTask<T>>,
}

impl<T> Clone for CFSTaskRef<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner }
    }
}

impl<T> CFSTaskRef<T> {
    pub const EMPTY: Self = Self {
        inner: NonNull::dangling(),
    };

    #[allow(unused)]
    pub fn new(inner: NonNull<CFSTask<T>>) -> Self {
        Self { inner }
    }

    pub fn as_ref(&self) -> &CFSTask<T> {
        unsafe { self.inner.as_ref() }
    }

    #[allow(unused)]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.inner.as_ptr() == other.inner.as_ptr()
    }
}

impl<T: Debug> Debug for CFSTask<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CFSTask")
            .field("inner", self.inner())
            .finish()
    }
}

impl<T: Debug> Debug for CFSTaskRef<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CFSTaskRef")
            .field("inner", self.as_ref())
            .finish()
    }
}

/// A simple [Completely Fair Scheduler][1] (CFS).
///
/// [1]: https://en.wikipedia.org/wiki/Completely_Fair_Scheduler
pub struct CFScheduler<T, const CAPACITY: usize> {
    ready_queue: LockFreeBTreeMap<(isize, isize), CFSTaskRef<T>, CAPACITY>, // (vruntime, taskid)
    min_vruntime: AtomicCell<Option<isize>>,
    id_pool: AtomicIsize,
}

impl<T, const CAPACITY: usize> CFScheduler<T, CAPACITY> {
    /// Creates a new empty [`CFScheduler`].
    pub const fn new() -> Self {
        Self {
            ready_queue: LockFreeBTreeMap::new(),
            min_vruntime: AtomicCell::new(None),
            id_pool: AtomicIsize::new(0_isize),
        }
    }
    /// get the name of scheduler
    pub fn scheduler_name() -> &'static str {
        "Completely Fair"
    }
}

impl<T, const CAPACITY: usize> BaseScheduler for CFScheduler<T, CAPACITY> {
    type SchedItem = CFSTaskRef<T>;

    fn init(&mut self) {}

    fn add_task(&self, task: Self::SchedItem) {
        let raw_task = task.as_ref();
        if self.min_vruntime.load().is_none() {
            self.min_vruntime.store(Some(0_isize));
        }
        let vruntime = self.min_vruntime.load().unwrap();
        let taskid = self.id_pool.fetch_add(1, Ordering::Release);
        raw_task.set_vruntime(vruntime);
        raw_task.set_id(taskid);
        self.ready_queue.insert((vruntime, taskid), task);
        if let Some(((min_vruntime, _), _)) = self.ready_queue.first_key_value() {
            self.min_vruntime.store(Some(min_vruntime));
        } else {
            self.min_vruntime.store(None);
        }
    }

    fn pick_next_task(&self) -> Option<Self::SchedItem> {
        if let Some((_, v)) = self.ready_queue.pop_first() {
            Some(v)
        } else {
            None
        }
    }

    fn put_prev_task(&self, prev: Self::SchedItem, _preempt: bool) {
        let raw_prev_task = prev.as_ref();
        let taskid = self.id_pool.fetch_add(1, Ordering::Release);
        raw_prev_task.set_id(taskid);
        self.ready_queue
            .insert((raw_prev_task.get_vruntime(), taskid), prev);
    }

    fn task_tick(&self, current: &Self::SchedItem) -> bool {
        let raw_task = current.as_ref();
        raw_task.task_tick();
        if self.ready_queue.is_empty() {
            return false;
        }
        self.min_vruntime.load().is_none()
            || raw_task.get_vruntime() > self.min_vruntime.load().unwrap()
    }

    fn set_priority(&self, task: &Self::SchedItem, prio: isize) -> bool {
        if (-20..=19).contains(&prio) {
            task.as_ref().set_priority(prio);
            true
        } else {
            false
        }
    }
}
