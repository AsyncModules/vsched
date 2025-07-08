use crate::task::TaskInner;
use crate::{get_data_base, percpu::PerCPU, sched::get_run_queue, select_run_queue};
use config::RQ_CAP;

cfg_if::cfg_if! {
    if #[cfg(feature = "sched-rr")] {
        const MAX_TIME_SLICE: usize = 5;
        pub type BaseTask = scheduler::RRTask<TaskInner, MAX_TIME_SLICE>;
        pub type BaseTaskRef = scheduler::RRTaskRef<TaskInner, MAX_TIME_SLICE>;
        pub type Scheduler = scheduler::RRScheduler<TaskInner, MAX_TIME_SLICE, RQ_CAP>;
    } else if #[cfg(feature = "sched-cfs")] {
        pub type BaseTask = scheduler::CFSTask<TaskInner>;
        pub type BaseTaskRef = scheduler::CFSTaskRef<TaskInner>;
        pub type Scheduler = scheduler::CFScheduler<TaskInner, RQ_CAP>;
    } else {
        // If no scheduler features are set, use FIFO as the default.
        pub type BaseTask = scheduler::FifoTask<TaskInner>;
        pub type BaseTaskRef = scheduler::FiFoTaskRef<TaskInner>;
        pub type Scheduler = scheduler::FifoScheduler<TaskInner, RQ_CAP>;
    }
}

/// Gets the current task.
///
/// # Panics
///
/// Panics if the current task is not initialized.
#[unsafe(no_mangle)]
pub extern "C" fn select_index(task: BaseTaskRef) -> usize {
    crate::sched::select_run_queue_index(task.as_ref().cpumask())
}

#[unsafe(no_mangle)]
pub extern "C" fn prev_task(cpu_id: usize) -> BaseTaskRef {
    unsafe {
        crate::get_run_queue(cpu_id)
            .prev_task
            .as_ref_unchecked()
            .clone()
    }
}

/// Gets the current task.
///
/// # Panics
///
/// Panics if the current task is not initialized.
#[unsafe(no_mangle)]
pub extern "C" fn current(cpu_id: usize) -> BaseTaskRef {
    unsafe {
        crate::get_run_queue(cpu_id)
            .current_task
            .as_ref_unchecked()
            .clone()
    }
}

/// Initializes the task scheduler (for the primary CPU).
#[unsafe(no_mangle)]
pub extern "C" fn init_vsched(cpu_id: usize, idle_task: BaseTaskRef) {
    let per_cpu_base = get_data_base() as *mut PerCPU;
    unsafe {
        let per_cpu = per_cpu_base.add(cpu_id);
        *per_cpu = PerCPU::new(cpu_id, idle_task);
    }
}

/// Initializes the task scheduler for secondary CPUs.
#[unsafe(no_mangle)]
pub extern "C" fn init_vsched_secondary(cpu_id: usize, idle_task: BaseTaskRef) {
    let per_cpu_base = get_data_base() as *mut PerCPU;
    unsafe {
        let per_cpu = per_cpu_base.add(cpu_id);
        *per_cpu = PerCPU::new(cpu_id, idle_task);
    }
}

/// Spawns a new task with the default parameters.
///
/// The default task name is an empty string. The default task stack size is
/// [`axconfig::TASK_STACK_SIZE`].
///
/// Returns the task reference.
#[unsafe(no_mangle)]
pub extern "C" fn spawn(task_ref: BaseTaskRef) {
    select_run_queue(task_ref.clone()).add_task(task_ref);
}

/// Set the priority for current task.
///
/// The range of the priority is dependent on the underlying scheduler. For
/// example, in the [CFS] scheduler, the priority is the nice value, ranging from
/// -20 to 19.
///
/// Returns `true` if the priority is set successfully.
///
/// [CFS]: https://en.wikipedia.org/wiki/Completely_Fair_Scheduler
#[unsafe(no_mangle)]
pub extern "C" fn set_priority(prio: isize, cpu_id: usize) -> bool {
    get_run_queue(cpu_id).set_current_priority(prio)
}

/// Current task gives up the CPU time voluntarily, and switches to another
/// ready task.
#[unsafe(no_mangle)]
pub extern "C" fn yield_now(cpu_id: usize) {
    get_run_queue(cpu_id).yield_current()
}

#[unsafe(no_mangle)]
pub extern "C" fn resched(cpu_id: usize) {
    get_run_queue(cpu_id).resched()
}

#[unsafe(no_mangle)]
pub extern "C" fn switch_to(cpu_id: usize, prev_task: &BaseTaskRef, next_task: BaseTaskRef) {
    get_run_queue(cpu_id).switch_to(prev_task, next_task)
}

#[unsafe(no_mangle)]
pub extern "C" fn resched_f(cpu_id: usize) -> bool {
    get_run_queue(cpu_id).resched_f()
}

/// Wake up a task to the distination cpu,
#[rustfmt::skip]
#[unsafe(no_mangle)]
pub extern "C" fn unblock_task(task: BaseTaskRef, resched: bool, src_cpu_id: usize, dst_cpu_id: usize) {
    get_run_queue(dst_cpu_id).unblock_task(task, resched, src_cpu_id);
}
