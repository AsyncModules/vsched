#![no_std]
#![feature(linkage)]

extern crate alloc;

#[macro_use]
extern crate log;

mod task;
mod task_ext;

pub use task::*;
pub use task_ext::*;

use config::RQ_CAP;

cfg_if::cfg_if! {
    if #[cfg(feature = "sched-rr")] {
        const MAX_TIME_SLICE: usize = 5;
        pub type BaseTask = scheduler::RRTask<TaskInner, MAX_TIME_SLICE>;
        pub type BaseTaskRef = scheduler::RRTaskRef<TaskInner, MAX_TIME_SLICE>;
        pub type WeakBaseTaskRef = scheduler::WeakRRTaskRef<TaskInner, MAX_TIME_SLICE>;

        pub type Scheduler = scheduler::RRScheduler<TaskInner, MAX_TIME_SLICE, RQ_CAP>;
    } else if #[cfg(feature = "sched-cfs")] {
        pub type BaseTask = scheduler::CFSTask<TaskInner>;
        pub type BaseTaskRef = scheduler::CFSTaskRef<TaskInner>;
        pub type WeakBaseTaskRef = scheduler::WeakCFSTaskRef<TaskInner>;
        pub type Scheduler = scheduler::CFScheduler<TaskInner, RQ_CAP>;
    } else {
        // If no scheduler features are set, use FIFO as the default.
        pub type BaseTask = scheduler::FifoTask<TaskInner>;
        pub type BaseTaskRef = scheduler::FiFoTaskRef<TaskInner>;
        pub type WeakBaseTaskRef = scheduler::WeakFiFoTaskRef<TaskInner>;
        pub type Scheduler = scheduler::FifoScheduler<TaskInner, RQ_CAP>;
    }
}
