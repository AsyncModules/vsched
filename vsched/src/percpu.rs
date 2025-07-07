use crate::api::{BaseTaskRef, Scheduler};
use core::cell::UnsafeCell;

pub(crate) struct PerCPU {
    /// The ID of the CPU this run queue is associated with.
    pub(crate) cpu_id: usize,
    /// The core scheduler of this run queue.
    /// Since irq and preempt are preserved by the kernel guard hold by `AxRunQueueRef`,
    /// we just use a simple raw spin lock here.
    pub(crate) scheduler: Scheduler,

    pub(crate) current_task: UnsafeCell<BaseTaskRef>,

    pub(crate) idle_task: BaseTaskRef,
    /// Stores the weak reference to the previous task that is running on this CPU.
    pub(crate) prev_task: UnsafeCell<BaseTaskRef>,
}

impl PerCPU {
    pub fn new(cpu_id: usize, idle_task: BaseTaskRef) -> Self {
        Self {
            cpu_id,
            scheduler: Scheduler::new(),
            current_task: UnsafeCell::new(idle_task.clone()),
            idle_task,
            prev_task: UnsafeCell::new(BaseTaskRef::EMPTY),
        }
    }
}
