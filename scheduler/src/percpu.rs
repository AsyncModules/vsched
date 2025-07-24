use crate::{BaseTaskRef, Scheduler};
use config::PAGES_SIZE_4K;
use core::{
    cell::UnsafeCell,
    mem::{size_of, MaybeUninit},
};

pub const fn percpu_size_4k_aligned<T>() -> usize {
    const MASK: usize = !(PAGES_SIZE_4K - 1);
    (size_of::<PerCPU<T>>() + PAGES_SIZE_4K - 1) & MASK
}

#[repr(C)]
pub struct PerCPU<T> {
    /// The ID of the CPU this run queue is associated with.
    pub cpu_id: usize,
    pub current_task: UnsafeCell<BaseTaskRef<T>>,
    pub idle_task: BaseTaskRef<T>,
    /// Stores the weak reference to the previous task that is running on this CPU.
    pub prev_task: UnsafeCell<MaybeUninit<BaseTaskRef<T>>>,
    /// The core scheduler of this run queue.
    pub scheduler: Scheduler<T>,
}

impl<T> PerCPU<T> {
    pub fn new(cpu_id: usize, idle_task: BaseTaskRef<T>, boot_task: BaseTaskRef<T>) -> Self {
        Self {
            cpu_id,
            current_task: UnsafeCell::new(boot_task.clone()),
            idle_task: idle_task,
            prev_task: UnsafeCell::new(MaybeUninit::new(boot_task)),
            scheduler: Scheduler::new(),
        }
    }
}
