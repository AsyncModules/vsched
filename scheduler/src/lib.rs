//! Various scheduler algorithms in a unified interface.
//!
//! Currently supported algorithms:
//!
//! - [`FifoScheduler`]: FIFO (First-In-First-Out) scheduler (cooperative).
//! - [`RRScheduler`]: Round-robin scheduler (preemptive).
//! - [`CFScheduler`]: Completely Fair Scheduler (preemptive).

#![cfg_attr(not(test), no_std)]
#![feature(unsafe_cell_access)]

mod cfs;
mod fifo;
mod round_robin;

#[cfg(test)]
extern crate alloc;

#[cfg(test)]
mod tests;

pub use cfs::{CFSTask, CFSTaskRef, CFScheduler};
pub use fifo::{FiFoTaskRef, FifoScheduler, FifoTask};
pub use round_robin::{RRScheduler, RRTask, RRTaskRef};

/// The base scheduler trait that all schedulers should implement.
///
/// All tasks in the scheduler are considered runnable. If a task is go to
/// sleep, it should be removed from the scheduler.
pub trait BaseScheduler {
    /// Type of scheduled entities. Often a task struct.
    type SchedItem;

    /// Initializes the scheduler.
    fn init(&mut self);

    /// Adds a task to the scheduler.
    fn add_task(&self, task: Self::SchedItem);

    /// Picks the next task to run, it will be removed from the scheduler.
    /// Returns [`None`] if there is not runnable task.
    fn pick_next_task(&self) -> Option<Self::SchedItem>;

    /// Puts the previous task back to the scheduler. The previous task is
    /// usually placed at the end of the ready queue, making it less likely
    /// to be re-scheduled.
    ///
    /// `preempt` indicates whether the previous task is preempted by the next
    /// task. In this case, the previous task may be placed at the front of the
    /// ready queue.
    fn put_prev_task(&self, prev: Self::SchedItem, preempt: bool);

    /// Advances the scheduler state at each timer tick. Returns `true` if
    /// re-scheduling is required.
    ///
    /// `current` is the current running task.
    fn task_tick(&self, current: &Self::SchedItem) -> bool;

    /// set priority for a task
    fn set_priority(&self, task: &Self::SchedItem, prio: isize) -> bool;
}
