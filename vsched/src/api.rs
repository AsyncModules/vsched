use core::mem::MaybeUninit;

use crate::sched::{get_data_base, get_run_queue};
use base_task::{BaseScheduler, PerCPU, TaskRef, TaskState, percpu_size_4k_aligned};

#[unsafe(no_mangle)]
pub extern "C" fn clear_prev_task_on_cpu(cpu_id: usize) {
    crate::sched::clear_prev_task_on_cpu(get_run_queue(cpu_id));
}

/// Gets the current task.
///
/// # Panics
///
/// Panics if the current task is not initialized.
#[unsafe(no_mangle)]
pub extern "C" fn current(cpu_id: usize) -> TaskRef {
    unsafe {
        get_run_queue(cpu_id)
            .current_task
            .as_ref_unchecked()
            .clone()
    }
}

/// Initializes the task scheduler (for the primary CPU).
#[unsafe(no_mangle)]
pub extern "C" fn init_vsched(cpu_id: usize, idle_task: TaskRef, boot_task: TaskRef) {
    let per_cpu_base = get_data_base() as *mut u8;
    unsafe {
        let per_cpu = per_cpu_base.add(cpu_id * percpu_size_4k_aligned::<base_task::TaskInner>())
            as *mut MaybeUninit<PerCPU>;
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
pub extern "C" fn spawn(cpu_id: usize, task_ref: TaskRef) {
    crate::sched::add_task(get_run_queue(cpu_id), task_ref);
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
    crate::sched::set_current_priority(get_run_queue(cpu_id), prio)
}

/// task tick
#[unsafe(no_mangle)]
pub extern "C" fn task_tick(cpu_id: usize, task_ref: &TaskRef) -> bool {
    crate::sched::task_tick(get_run_queue(cpu_id), task_ref)
}

/// migrate_entry
#[unsafe(no_mangle)]
pub extern "C" fn migrate_entry(cpu_id: usize, migrated_task: TaskRef) {
    get_run_queue(cpu_id)
        .scheduler
        .put_prev_task(migrated_task, false);
}

/// Current task gives up the CPU time voluntarily, and switches to another
/// ready task.
#[unsafe(no_mangle)]
pub extern "C" fn yield_now(cpu_id: usize) {
    crate::sched::yield_current(get_run_queue(cpu_id));
}

/// Preempt the current task
#[unsafe(no_mangle)]
pub extern "C" fn preempt_current(cpu_id: usize) {
    crate::sched::preempt_current(get_run_queue(cpu_id));
}

#[unsafe(no_mangle)]
pub extern "C" fn resched(cpu_id: usize) {
    crate::sched::resched(get_run_queue(cpu_id));
}

#[unsafe(no_mangle)]
pub extern "C" fn switch_to(cpu_id: usize, prev_task: &TaskRef, next_task: TaskRef) {
    crate::sched::switch_to(get_run_queue(cpu_id), prev_task, next_task);
}

#[unsafe(no_mangle)]
pub extern "C" fn resched_f(cpu_id: usize) -> bool {
    crate::sched::resched_f(get_run_queue(cpu_id))
}

/// Wake up a task to the distination cpu,
#[rustfmt::skip]
#[unsafe(no_mangle)]
pub extern "C" fn unblock_task(task: TaskRef, resched: bool, dst_cpu_id: usize, src_cpu_id: usize) {
    crate::sched::unblock_task(get_run_queue(dst_cpu_id), task, resched, src_cpu_id);
}

/// yield future
#[unsafe(no_mangle)]
pub extern "C" fn yield_f(cpu_id: usize) -> bool {
    let per_cpu = get_run_queue(cpu_id);
    let curr = unsafe { per_cpu.current_task.as_ref_unchecked() };
    assert!(curr.is_running());
    crate::sched::put_task_with_state(per_cpu, curr.clone(), TaskState::Running, false);
    crate::sched::resched_f(per_cpu)
}
