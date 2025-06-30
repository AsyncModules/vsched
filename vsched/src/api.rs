use crate::{get_data_base, percpu::PerCPU, sched::get_run_queue, select_run_queue};
use config::RQ_CAP;
use task_inner::TaskInner;

cfg_if::cfg_if! {
    if #[cfg(feature = "sched_rr")] {
        const MAX_TIME_SLICE: usize = 5;
        pub(crate) type AxTaskRef = scheduler::RRTaskRef<TaskInner, MAX_TIME_SLICE>;
        pub(crate) type Scheduler = scheduler::RRScheduler<TaskInner, MAX_TIME_SLICE, RQ_CAP>;
    } else if #[cfg(feature = "sched_cfs")] {
        pub(crate) type AxTaskRef = scheduler::CFSTaskRef<TaskInner>;
        pub(crate) type Scheduler = scheduler::CFScheduler<TaskInner, RQ_CAP>;
    } else {
        // If no scheduler features are set, use FIFO as the default.
        pub(crate) type AxTaskRef = scheduler::FiFoTaskRef<TaskInner>;
        pub(crate) type Scheduler = scheduler::FifoScheduler<TaskInner, RQ_CAP>;
    }
}

/// Gets the current task.
///
/// # Panics
///
/// Panics if the current task is not initialized.
#[unsafe(no_mangle)]
pub extern "C" fn current(cpu_id: usize) -> AxTaskRef {
    unsafe {
        crate::get_run_queue(cpu_id)
            .current_task
            .as_ref_unchecked()
            .clone()
    }
}

/// Initializes the task scheduler (for the primary CPU).
#[unsafe(no_mangle)]
pub fn init_vsched(cpu_id: usize, idle_task: AxTaskRef) {
    let per_cpu_base = get_data_base() as *mut PerCPU;
    unsafe {
        let per_cpu = per_cpu_base.add(cpu_id);
        *per_cpu = PerCPU::new(cpu_id, idle_task);
    }
}

/// Initializes the task scheduler for secondary CPUs.
#[unsafe(no_mangle)]
pub fn init_vsched_secondary(cpu_id: usize, idle_task: AxTaskRef) {
    let per_cpu_base = get_data_base() as *mut PerCPU;
    unsafe {
        let per_cpu = per_cpu_base.add(cpu_id);
        *per_cpu = PerCPU::new(cpu_id, idle_task);
    }
}

// /// Handles periodic timer ticks for the task manager.
// ///
// /// For example, advance scheduler states, checks timed events, etc.
// #[cfg(feature = "irq")]
// #[doc(cfg(feature = "irq"))]
// pub fn on_timer_tick() {
//     use kernel_guard::NoOp;
//     crate::timers::check_events();
//     // Since irq and preemption are both disabled here,
//     // we can get current run queue with the default `kernel_guard::NoOp`.
//     current_run_queue::<NoOp>().scheduler_timer_tick();
// }

/// Spawns a new task with the default parameters.
///
/// The default task name is an empty string. The default task stack size is
/// [`axconfig::TASK_STACK_SIZE`].
///
/// Returns the task reference.
#[unsafe(no_mangle)]
pub extern "C" fn spawn(task_ref: AxTaskRef) {
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
pub fn set_priority(prio: isize, cpu_id: usize) -> bool {
    get_run_queue(cpu_id).set_current_priority(prio)
}

// /// Set the affinity for the current task.
// /// [`AxCpuMask`] is used to specify the CPU affinity.
// /// Returns `true` if the affinity is set successfully.
// ///
// /// TODO: support set the affinity for other tasks.
// pub fn set_current_affinity(cpumask: AxCpuMask, cpu_id: usize) -> bool {
//     if cpumask.is_empty() {
//         false
//     } else {
//         let curr = current(cpu_id);

//         curr.as_ref().set_cpumask(cpumask);
//         // After setting the affinity, we need to check if current cpu matches
//         // the affinity. If not, we need to migrate the task to the correct CPU.
//         #[cfg(feature = "smp")]
//         if !cpumask.get(cpu_id) {
//             const MIGRATION_TASK_STACK_SIZE: usize = 4096;
//             // Spawn a new migration task for migrating.
//             let migration_task = TaskInner::new(
//                 move || crate::run_queue::migrate_entry(curr),
//                 "migration-task".into(),
//                 MIGRATION_TASK_STACK_SIZE,
//             )
//             .into_arc();

//             // Migrate the current task to the correct CPU using the migration task.
//             current_run_queue::<NoPreemptIrqSave>().migrate_current(migration_task);

//             assert!(cpumask.get(axhal::cpu::this_cpu_id()), "Migration failed");
//         }
//         true
//     }
// }

/// Current task gives up the CPU time voluntarily, and switches to another
/// ready task.
#[unsafe(no_mangle)]
pub extern "C" fn yield_now(cpu_id: usize) {
    get_run_queue(cpu_id).yield_current()
}

// /// Current coroutine task gives up the CPU time voluntarily, and switches to another
// /// ready task.
// #[inline]
// pub async fn yield_now_f() {
//     crate::run_queue::YieldFuture::<NoPreemptIrqSave>::new().await;
// }

// /// Current coroutine task is going to sleep for the given duration.
// ///
// /// If the feature `irq` is not enabled, it uses busy-wait instead.
// pub async fn sleep_f(dur: core::time::Duration) {
//     sleep_until_f(axhal::time::wall_time() + dur).await;
// }

// /// Current task is going to sleep for the given duration.
// ///
// /// If the feature `irq` is not enabled, it uses busy-wait instead.
// pub fn sleep(dur: core::time::Duration) {
//     sleep_until(axhal::time::wall_time() + dur);
// }

// /// Current task is going to sleep, it will be woken up at the given deadline.
// ///
// /// If the feature `irq` is not enabled, it uses busy-wait instead.
// pub fn sleep_until(deadline: axhal::time::TimeValue) {
//     #[cfg(feature = "irq")]
//     current_run_queue::<NoPreemptIrqSave>().sleep_until(deadline);
//     #[cfg(not(feature = "irq"))]
//     axhal::time::busy_wait_until(deadline);
// }

// /// Current coroutine task is going to sleep, it will be woken up at the given deadline.
// ///
// /// If the feature `irq` is not enabled, it uses busy-wait instead.
// pub async fn sleep_until_f(deadline: axhal::time::TimeValue) {
//     #[cfg(feature = "irq")]
//     crate::run_queue::SleepUntilFuture::<NoPreemptIrqSave>::new(deadline).await;
//     #[cfg(not(feature = "irq"))]
//     axhal::time::busy_wait_until(deadline);
// }

/// Exits the current task.
#[unsafe(no_mangle)]
pub fn exit(exit_code: i32, cpu_id: usize) -> ! {
    get_run_queue(cpu_id).exit_current(exit_code)
}

// /// Exits the current coroutine task.
// pub async fn exit_f(exit_code: i32) {
//     crate::run_queue::ExitFuture::<NoPreemptIrqSave>::new(exit_code).await;
// }
