use core::mem::MaybeUninit;

use crate::sched::select_run_queue_index;
use crate::task::TaskInner;
use crate::{percpu::PerCPU, sched::get_run_queue, select_run_queue};
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

/// Safety:
///     the offset of this function in the `.text`
///     section must be little than 0x1000.
///     The `#[inline(never)]` attribute and the
///     offset requirement can make it work ok.
pub fn get_data_base() -> usize {
    let pc = unsafe { hal::asm::get_pc() };
    const VSCHED_DATA_SIZE: usize = config::SMP
        * ((core::mem::size_of::<crate::percpu::PerCPU>() + config::PAGES_SIZE_4K - 1)
            & (!(config::PAGES_SIZE_4K - 1)));
    (pc & config::DATA_SEC_MASK) - VSCHED_DATA_SIZE
}

#[unsafe(no_mangle)]
pub extern "C" fn prev_task(cpu_id: usize) -> &'static WeakBaseTaskRef {
    unsafe {
        crate::get_run_queue(cpu_id)
            .prev_task
            .as_ref_unchecked()
            .assume_init_ref()
    }
}

/// Gets the current task.
///
/// # Panics
///
/// Panics if the current task is not initialized.
#[unsafe(no_mangle)]
pub extern "C" fn current(cpu_id: usize) -> &'static BaseTaskRef {
    unsafe { crate::get_run_queue(cpu_id).current_task.as_ref_unchecked() }
}

/// Initializes the task scheduler (for the primary CPU).
#[unsafe(no_mangle)]
pub extern "C" fn init_vsched(cpu_id: usize, idle_task: BaseTaskRef, boot_task: BaseTaskRef) {
    let per_cpu_base = get_data_base() as *mut MaybeUninit<PerCPU>;
    unsafe {
        let per_cpu = per_cpu_base.add(cpu_id);
        *per_cpu = MaybeUninit::new(PerCPU::new(cpu_id, idle_task, boot_task));
    }
}

/// Spawns a new task with the default parameters.
///
/// The default task name is an empty string. The default task stack size is
/// [`axconfig::TASK_STACK_SIZE`].
///
/// Returns the task reference.
#[unsafe(no_mangle)]
pub extern "C" fn spawn(task_ref: BaseTaskRef) -> BaseTaskRef {
    select_run_queue(&task_ref).add_task(task_ref.clone());
    task_ref
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
#[unsafe(no_mangle)]
pub extern "C" fn unblock_task(task: BaseTaskRef, resched: bool, src_cpu_id: usize) {
    let dst_cpu_id = select_run_queue_index(task.cpumask());
    get_run_queue(dst_cpu_id).unblock_task(task, resched, src_cpu_id);
}
