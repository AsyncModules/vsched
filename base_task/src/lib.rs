#![no_std]
#![feature(linkage)]
#![feature(unsafe_cell_access)]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(feature = "alloc")]
#[macro_use]
extern crate log;

mod task;
#[cfg(feature = "alloc")]
mod task_ext;
#[cfg(feature = "alloc")]
mod wait_queue;

pub use task::*;
#[cfg(feature = "alloc")]
pub use task_ext::*;

pub use scheduler::{BaseScheduler, percpu_size_4k_aligned};

pub type AxTask = scheduler::BaseTask<TaskInner>;
pub type TaskRef = scheduler::BaseTaskRef<TaskInner>;
pub type PerCPU = scheduler::PerCPU<TaskInner>;
pub type Scheduler = scheduler::Scheduler<TaskInner>;
