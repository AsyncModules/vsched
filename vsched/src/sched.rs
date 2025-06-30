use crate::api::AxTaskRef;
use crate::percpu::PerCPU;
use core::task::Poll;
use scheduler::BaseScheduler;
use task_inner::TaskState;

/// Selects the run queue index based on a CPU set bitmap and load balancing.
///
/// This function filters the available run queues based on the provided `cpumask` and
/// selects the run queue index for the next task. The selection is based on a round-robin algorithm.
///
/// ## Arguments
///
/// * `cpumask` - A bitmap representing the CPUs that are eligible for task execution.
///
/// ## Returns
///
/// The index (cpu_id) of the selected run queue.
///
/// ## Panics
///
/// This function will panic if `cpu_mask` is empty, indicating that there are no available CPUs for task execution.
///
#[cfg(feature = "smp")]
// The modulo operation is safe here because `axconfig::SMP` is always greater than 1 with "smp" enabled.
#[allow(clippy::modulo_one)]
#[inline]
fn select_run_queue_index(cpumask: AxCpuMask) -> usize {
    use core::sync::atomic::{AtomicUsize, Ordering};
    static RUN_QUEUE_INDEX: AtomicUsize = AtomicUsize::new(0);

    assert!(!cpumask.is_empty(), "No available CPU for task execution");

    // Round-robin selection of the run queue index.
    loop {
        let index = RUN_QUEUE_INDEX.fetch_add(1, Ordering::SeqCst) % config::SMP;
        if cpumask.get(index) {
            return index;
        }
    }
}

/// Retrieves a `'static` reference to the run queue corresponding to the given index.
///
/// This function asserts that the provided index is within the range of available CPUs
/// and returns a reference to the corresponding run queue.
///
/// ## Arguments
///
/// * `index` - The index of the run queue to retrieve.
///
/// ## Returns
///
/// A reference to the `AxRunQueue` corresponding to the provided index.
///
/// ## Panics
///
/// This function will panic if the index is out of bounds.
///
#[inline]
pub fn get_run_queue(index: usize) -> &'static PerCPU {
    let per_cpu_base = crate::get_data_base() as *mut PerCPU;
    let per_cpu = unsafe { &*per_cpu_base.add(index) };
    per_cpu
}

/// Selects the appropriate run queue for the provided task.
///
/// * In a single-core system, this function always returns a reference to the global run queue.
/// * In a multi-core system, this function selects the run queue based on the task's CPU affinity and load balance.
///
/// ## Arguments
///
/// * `task` - A reference to the task for which a run queue is being selected.
///
/// ## Returns
///
/// * [`AxRunQueueRef`] - a static reference to the selected [`AxRunQueue`] (current or remote).
///
/// ## TODO
///
/// 1. Implement better load balancing across CPUs for more efficient task distribution.
/// 2. Use a more generic load balancing algorithm that can be customized or replaced.
///
#[inline]
pub(crate) fn select_run_queue(task: AxTaskRef) -> &'static PerCPU {
    #[cfg(not(feature = "smp"))]
    {
        let _ = task;
        // When SMP is disabled, all tasks are scheduled on the same global run queue.
        get_run_queue(0)
    }
    #[cfg(feature = "smp")]
    {
        // When SMP is enabled, select the run queue based on the task's CPU affinity and load balance.
        let index = select_run_queue_index(task.cpumask());
        get_run_queue(index)
    }
}

/// Management operations for run queue, including adding tasks, unblocking tasks, etc.
impl PerCPU {
    /// Puts target task into current run queue with `Ready` state
    /// if its state matches `current_state` (except idle task).
    ///
    /// If `preempt`, keep current task's time slice, otherwise reset it.
    ///
    /// Returns `true` if the target task is put into this run queue successfully,
    /// otherwise `false`.
    fn put_task_with_state(
        &self,
        task: &AxTaskRef,
        current_state: TaskState,
        preempt: bool,
    ) -> bool {
        // If the task's state matches `current_state`, set its state to `Ready` and
        // put it back to the run queue (except idle task).
        if task
            .as_ref()
            .transition_state(current_state, TaskState::Ready)
            && !task.as_ref().is_idle()
        {
            // If the task is blocked, wait for the task to finish its scheduling process.
            // See `unblock_task()` for details.
            if current_state == TaskState::Blocked {
                // Wait for next task's scheduling process to complete.
                // If the owning (remote) CPU is still in the middle of schedule() with
                // this task (next task) as prev, wait until it's done referencing the task.
                //
                // Pairs with the `clear_prev_task_on_cpu()`.
                //
                // Note:
                // 1. This should be placed after the judgement of `TaskState::Blocked,`,
                //    because the task may have been woken up by other cores.
                // 2. This can be placed in the front of `switch_to()`
                #[cfg(feature = "smp")]
                while task.as_ref().on_cpu() {
                    // Wait for the task to finish its scheduling process.
                    core::hint::spin_loop();
                }
            }
            // TODO: priority
            self.scheduler.put_prev_task(task.clone(), preempt);
            true
        } else {
            false
        }
    }

    /// Adds a task to the scheduler.
    ///
    /// This function is used to add a new task to the scheduler.
    pub fn add_task(&self, task: AxTaskRef) {
        assert!(task.as_ref().is_ready());
        self.scheduler.add_task(task);
    }

    /// Unblock one task by inserting it into the run queue.
    ///
    /// This function does nothing if the task is not in [`TaskState::Blocked`],
    /// which means the task is already unblocked by other cores.
    pub fn unblock_task(&mut self, task: AxTaskRef, resched: bool, this_cpu_id: usize) {
        let _task_id = task.as_ref().id();
        // Try to change the state of the task from `Blocked` to `Ready`,
        // if successful, the task will be put into this run queue,
        // otherwise, the task is already unblocked by other cores.
        // Note:
        // target task can not be insert into the run queue until it finishes its scheduling process.
        if self.put_task_with_state(&task, TaskState::Blocked, resched) {
            // Since now, the task to be unblocked is in the `Ready` state.
            let cpu_id = self.cpu_id;
            // Note: when the task is unblocked on another CPU's run queue,
            // we just ingiore the `resched` flag.
            if resched && cpu_id == this_cpu_id {
                // TODO: 增加判断当前任务的条件
                #[cfg(feature = "preempt")]
                get_run_queue(this_cpu_id)
                    .current_task
                    .load()
                    .as_ref()
                    .set_preempt_pending(true);
                // #[cfg(feature = "preempt")]
                // crate::current().set_preempt_pending(true);
            }
        }
    }

    #[cfg(feature = "irq")]
    pub fn scheduler_timer_tick(&self) {
        let curr = &self.current_task.load().as_ref();
        if !curr.is_idle() && self.scheduler.task_tick(curr) {
            #[cfg(feature = "preempt")]
            curr.set_preempt_pending(true);
        }
    }

    /// Yield the current task and reschedule.
    /// This function will put the current task into this run queue with `Ready` state,
    /// and reschedule to the next task on this run queue.
    pub fn yield_current(&self) {
        let curr = unsafe { self.current_task.as_ref_unchecked() };
        assert!(curr.as_ref().is_running());

        self.put_task_with_state(curr, TaskState::Running, false);

        self.resched();
    }

    /// Migrate the current task to a new run queue matching its CPU affinity and reschedule.
    /// This function will spawn a new `migration_task` to perform the migration, which will set
    /// current task to `Ready` state and select a proper run queue for it according to its CPU affinity,
    /// switch to the migration task immediately after migration task is prepared.
    ///
    /// Note: the ownership if migrating task (which is current task) is handed over to the migration task,
    /// before the migration task inserted it into the target run queue.
    #[cfg(feature = "smp")]
    pub fn migrate_current(&self, migration_task: AxTaskRef) {
        let curr = self.current_task.load();
        assert!(curr.as_ref().is_running());

        // Mark current task's state as `Ready`,
        // but, do not put current task to the scheduler of this run queue.
        curr.as_ref().set_state(TaskState::Ready);

        // Call `switch_to` to reschedule to the migration task that performs the migration directly.
        self.switch_to(curr, migration_task);
    }

    /// Preempts the current task and reschedules.
    /// This function is used to preempt the current task and reschedule
    /// to next task on current run queue.
    ///
    /// This function is called by `current_check_preempt_pending` with IRQs and preemption disabled.
    ///
    /// Note:
    /// preemption may happened in `enable_preempt`, which is called
    /// each time a [`kspin::NoPreemptGuard`] is dropped.
    #[cfg(feature = "preempt")]
    pub fn preempt_resched(&mut self) {
        // There is no need to disable IRQ and preemption here, because
        // they both have been disabled in `current_check_preempt_pending`.
        let curr = self.current_task.load();
        assert!(curr.as_ref().is_running());

        // When we call `preempt_resched()`, both IRQs and preemption must
        // have been disabled by `kernel_guard::NoPreemptIrqSave`. So we need
        // to set `current_disable_count` to 1 in `can_preempt()` to obtain
        // the preemption permission.
        let can_preempt = curr.as_ref().can_preempt(1);

        if can_preempt {
            self.put_task_with_state(curr.clone(), TaskState::Running, true);
            self.resched();
        } else {
            curr.as_ref().set_preempt_pending(true);
        }
    }

    /// Exit the current task with the specified exit code.
    /// This function will never return.
    pub fn exit_current(&self, exit_code: i32) -> ! {
        let curr = unsafe { self.current_task.as_ref_unchecked() };
        assert!(
            curr.as_ref().is_running(),
            "task is not running: {:?}",
            curr.as_ref().state()
        );
        assert!(!curr.as_ref().is_idle());
        if curr.as_ref().is_init() {
            // Safety: it is called from `current_run_queue::<NoPreemptIrqSave>().exit_current(exit_code)`,
            // which disabled IRQs and preemption.
            while !self.exit_tasks.is_empty() {
                self.exit_tasks.pop_front();
            }
            // hal::misc::terminate();
        } else {
            curr.as_ref().set_state(TaskState::Exited);

            // Notify the joiner task.
            curr.as_ref().notify_exit(exit_code);

            // // Safety: it is called from `current_run_queue::<NoPreemptIrqSave>().exit_current(exit_code)`,
            // // which disabled IRQs and preemption.
            // unsafe {
            //     // Push current task to the `EXITED_TASKS` list, which will be consumed by the GC task.
            //     EXITED_TASKS.current_ref_mut_raw().push_back(curr.clone());
            //     // Wake up the GC task to drop the exited tasks.
            //     WAIT_FOR_EXIT.current_ref_mut_raw().notify_one(false);
            // }

            // Schedule to next task.
            self.resched();
        }
        unreachable!("task exited!");
    }

    // /// Block the current task, put current task into the wait queue and reschedule.
    // /// Mark the state of current task as `Blocked`, set the `in_wait_queue` flag as true.
    // /// Note:
    // ///     1. The caller must hold the lock of the wait queue.
    // ///     2. The caller must ensure that the current task is in the running state.
    // ///     3. The caller must ensure that the current task is not the idle task.
    // ///     4. The lock of the wait queue will be released explicitly after current task is pushed into it.
    // pub fn blocked_resched(&mut self, mut wq_guard: WaitQueueGuard) {
    //     let curr = self.current_task.load();
    //     assert!(curr.as_ref().is_running());
    //     assert!(!curr.as_ref().is_idle());
    //     // we must not block current task with preemption disabled.
    //     // Current expected preempt count is 2.
    //     // 1 for `NoPreemptIrqSave`, 1 for wait queue's `SpinNoIrq`.
    //     #[cfg(feature = "preempt")]
    //     assert!(curr.as_ref().can_preempt(2));

    //     // Mark the task as blocked, this has to be done before adding it to the wait queue
    //     // while holding the lock of the wait queue.
    //     curr.as_ref().set_state(TaskState::Blocked);
    //     curr.as_ref().set_in_wait_queue(true);

    //     wq_guard.push_back(curr.clone());
    //     // Drop the lock of wait queue explictly.
    //     drop(wq_guard);

    //     // Current task's state has been changed to `Blocked` and added to the wait queue.
    //     // Note that the state may have been set as `Ready` in `unblock_task()`,
    //     // see `unblock_task()` for details.

    //     self.resched();
    // }

    // #[cfg(feature = "irq")]
    // pub fn sleep_until(&mut self, deadline: axhal::time::TimeValue) {
    //     let curr = self.current_task.load();
    //     assert!(curr.as_ref().is_running());
    //     assert!(!curr.as_ref().is_idle());

    //     let now = axhal::time::wall_time();
    //     if now < deadline {
    //         crate::timers::set_alarm_wakeup(deadline, curr.clone());
    //         curr.as_ref().set_state(TaskState::Blocked);
    //         self.resched();
    //     }
    // }

    pub fn set_current_priority(&self, prio: isize) -> bool {
        self.scheduler
            .set_priority(unsafe { self.current_task.as_ref_unchecked() }, prio)
    }

    /// Core reschedule subroutine.
    /// Pick the next task to run and switch to it.
    fn resched(&self) {
        let next = self.scheduler.pick_next_task().unwrap_or_else(|| 
            // Safety: IRQs must be disabled at this time.
            self.idle_task.clone()
        );
        assert!(
            next.as_ref().is_ready(),
            "next {:?} is not ready: {:?}",
            next.as_ref().id(),
            next.as_ref().state()
        );
        self.switch_to(unsafe { self.current_task.as_ref_unchecked() }, next);
    }

    fn switch_to(&self, prev_task: &AxTaskRef, next_task: AxTaskRef) {
        // Make sure that IRQs are disabled by kernel guard or other means.
        #[cfg(all(not(test), feature = "irq"))] // Note: irq is faked under unit tests.
        assert!(
            !axhal::arch::irqs_enabled(),
            "IRQs must be disabled during scheduling"
        );
        #[cfg(feature = "preempt")]
        next_task.as_ref().set_preempt_pending(false);
        next_task.as_ref().set_state(TaskState::Running);
        if prev_task.ptr_eq(&next_task) {
            return;
        }

        // Claim the task as running, we do this before switching to it
        // such that any running task will have this set.
        #[cfg(feature = "smp")]
        next_task.as_ref().set_on_cpu(true);

        unsafe {
            let prev_ctx_ptr = prev_task.as_ref().ctx_mut_ptr();
            let next_ctx_ptr = next_task.as_ref().ctx_mut_ptr();
            // // If the next task is a coroutine, this will set the kstack and ctx.
            // next_task.as_ref().set_kstack();

            // Store the weak pointer of **prev_task** in percpu variable `PREV_TASK`.
            #[cfg(feature = "smp")]
            {
                self.prev_task.replace(prev_task);
                // *PREV_TASK.current_ref_mut_raw() = Arc::downgrade(prev_task.as_task_ref());
            }

            // // The strong reference count of `prev_task` will be decremented by 1,
            // // but won't be dropped until `gc_entry()` is called.
            // assert!(Arc::strong_count(prev_task.as_task_ref()) > 1);
            // assert!(Arc::strong_count(&next_task) >= 1);

            *self.current_task.as_mut_unchecked() = next_task;
            // CurrentTask::set_current(prev_task, next_task);

            (*prev_ctx_ptr).switch_to(&*next_ctx_ptr);

            // Current it's **next_task** running on this CPU, clear the `prev_task`'s `on_cpu` field
            // to indicate that it has finished its scheduling process and no longer running on this CPU.
            #[cfg(feature = "smp")]
            clear_prev_task_on_cpu();
        }
    }

    /// Core reschedule subroutine.
    /// Pick the next task to run and switch to it.
    /// This function is only used in `YieldFuture`, `ExitFuture`,
    /// `SleepUntilFuture` and `BlockedReschedFuture`.
    fn resched_f(&mut self) -> Poll<()> {
        let next_task = self.scheduler.pick_next_task().unwrap_or_else(|| 
            // Safety: IRQs must be disabled at this time.
            self.idle_task.clone()
        );
        assert!(
            next_task.as_ref().is_ready(),
            "next {:?} is not ready: {:?}",
            next_task.as_ref().id(),
            next_task.as_ref().state()
        );
        let prev_task = unsafe { self.current_task.as_ref_unchecked() };
        // Make sure that IRQs are disabled by kernel guard or other means.
        #[cfg(all(not(test), feature = "irq"))] // Note: irq is faked under unit tests.
        assert!(
            !axhal::arch::irqs_enabled(),
            "IRQs must be disabled during scheduling"
        );
        #[cfg(feature = "preempt")]
        next_task.as_ref().set_preempt_pending(false);
        next_task.as_ref().set_state(TaskState::Running);
        if prev_task.ptr_eq(&next_task) {
            return Poll::Ready(());
        }

        // Claim the task as running, we do this before switching to it
        // such that any running task will have this set.
        #[cfg(feature = "smp")]
        next_task.as_ref().set_on_cpu(true);

        unsafe {
            // Store the weak pointer of **prev_task** in percpu variable `PREV_TASK`.
            #[cfg(feature = "smp")]
            {
                self.prev_task.replace(prev_task);
                // *PREV_TASK.current_ref_mut_raw() = Arc::downgrade(prev_task.as_task_ref());
            }

            // // The strong reference count of `prev_task` will be decremented by 1,
            // // but won't be dropped until `gc_entry()` is called.
            // assert!(Arc::strong_count(prev_task.as_task_ref()) > 1);
            // assert!(Arc::strong_count(&next_task) >= 1);

            // Directly change the `CurrentTask` and return `Pending`.
            // CurrentTask::set_current(prev_task, next_task);
            *self.current_task.as_mut_unchecked() = next_task;
            Poll::Pending
        }
    }
}

// impl AxRunQueue {
// /// Create a new run queue for the specified CPU.
// /// The run queue is initialized with a per-CPU gc task in its scheduler.
// fn new(cpu_id: usize) -> Self {
//     let gc_task = TaskInner::new(gc_entry, "gc".into(), axconfig::TASK_STACK_SIZE).into_arc();
//     // gc task should be pinned to the current CPU.
//     gc_task.set_cpumask(AxCpuMask::one_shot(cpu_id));

//     let mut scheduler = Scheduler::new();
//     scheduler.add_task(gc_task);
//     Self {
//         cpu_id,
//         scheduler: SpinRaw::new(scheduler),
//     }
// }
// }

// fn gc_entry() {
//     loop {
//         // Drop all exited tasks and recycle resources.
//         let n = EXITED_TASKS.with_current(|exited_tasks| exited_tasks.len());
//         for _ in 0..n {
//             // Do not do the slow drops in the critical section.
//             let task = EXITED_TASKS.with_current(|exited_tasks| exited_tasks.pop_front());
//             if let Some(task) = task {
//                 if Arc::strong_count(&task) == 1 {
//                     // If I'm the last holder of the task, drop it immediately.
//                     drop(task);
//                 } else {
//                     // Otherwise (e.g, `switch_to` is not compeleted, held by the
//                     // joiner, etc), push it back and wait for them to drop first.
//                     EXITED_TASKS.with_current(|exited_tasks| exited_tasks.push_back(task));
//                 }
//             }
//         }
//         // Note: we cannot block current task with preemption disabled,
//         // use `current_ref_raw` to get the `WAIT_FOR_EXIT`'s reference here to avoid the use of `NoPreemptGuard`.
//         // Since gc task is pinned to the current CPU, there is no affection if the gc task is preempted during the process.
//         unsafe { WAIT_FOR_EXIT.current_ref_raw() }.wait();
//     }
// }

// /// The task routine for migrating the current task to the correct CPU.
// ///
// /// It calls `select_run_queue` to get the correct run queue for the task, and
// /// then puts the task to the scheduler of target run queue.
// #[cfg(feature = "smp")]
// pub(crate) fn migrate_entry(migrated_task: AxTaskRef) {
//     select_run_queue::<kernel_guard::NoPreemptIrqSave>(&migrated_task)
//         .inner
//         .scheduler
//         .lock()
//         .put_prev_task(migrated_task, false)
// }

// /// Clear the `on_cpu` field of previous task running on this CPU.
// #[cfg(feature = "smp")]
// pub(crate) unsafe fn clear_prev_task_on_cpu() {
//     unsafe {
//         PREV_TASK
//             .current_ref_raw()
//             .upgrade()
//             .expect("Invalid prev_task pointer or prev_task has been dropped")
//             .set_on_cpu(false);
//     }
// }

// pub(crate) fn init() {
//     let cpu_id = this_cpu_id();

//     // Create the `idle` task (not current task).
//     const IDLE_TASK_STACK_SIZE: usize = 4096;
//     let idle_task = TaskInner::new(|| crate::run_idle(), "idle".into(), IDLE_TASK_STACK_SIZE);
//     // idle task should be pinned to the current CPU.
//     idle_task.set_cpumask(AxCpuMask::one_shot(cpu_id));
//     IDLE_TASK.with_current(|i| {
//         i.init_once(idle_task.into_arc());
//     });

//     // Put the subsequent execution into the `main` task.
//     let main_task = TaskInner::new_init("main".into()).into_arc();
//     main_task.set_state(TaskState::Running);
//     unsafe { CurrentTask::init_current(main_task) }

//     RUN_QUEUE.with_current(|rq| {
//         rq.init_once(AxRunQueue::new(cpu_id));
//     });
//     unsafe {
//         RUN_QUEUES[cpu_id].write(RUN_QUEUE.current_ref_mut_raw());
//     }
// }

// pub(crate) fn init_secondary() {
//     let cpu_id = this_cpu_id();

//     // Put the subsequent execution into the `idle` task.
//     let idle_task = TaskInner::new_init("idle".into()).into_arc();
//     idle_task.set_state(TaskState::Running);
//     IDLE_TASK.with_current(|i| {
//         i.init_once(idle_task.clone());
//     });
//     unsafe { CurrentTask::init_current(idle_task) }

//     RUN_QUEUE.with_current(|rq| {
//         rq.init_once(AxRunQueue::new(cpu_id));
//     });
//     unsafe {
//         RUN_QUEUES[cpu_id].write(RUN_QUEUE.current_ref_mut_raw());
//     }
// }

// /// The `YieldFuture` used when yielding the current task and reschedule.
// /// When polling this future, the current task will be put into the run queue
// /// with `Ready` state and reschedule to the next task on the run queue.
// ///
// /// The polling operation is as the same as the
// /// `current_run_queue::<NoPreemptIrqSave>().yield_current()` function.
// ///
// /// SAFETY:
// /// Due to this future is constructed with `current_run_queue::<NoPreemptIrqSave>()`,
// /// the operation about manipulating the RunQueue and the switching to next task is
// /// safe(The `IRQ` and `Preempt` are disabled).
// pub(crate) struct YieldFuture<'a, G: BaseGuard> {
//     current_run_queue: CurrentRunQueueRef<'a, G>,
//     flag: bool,
// }

// impl<'a, G: BaseGuard> YieldFuture<'a, G> {
//     pub(crate) fn new() -> Self {
//         Self {
//             current_run_queue: current_run_queue::<G>(),
//             flag: false,
//         }
//     }
// }

// impl<'a, G: BaseGuard> Unpin for YieldFuture<'a, G> {}

// impl<'a, G: BaseGuard> Future for YieldFuture<'a, G> {
//     type Output = ();
//     fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
//         let Self {
//             current_run_queue,
//             flag,
//         } = self.get_mut();
//         if !(*flag) {
//             *flag = !*flag;
//             let curr = &current_run_queue.current_task;
//             assert!(curr.is_running());
//             current_run_queue
//                 .inner
//                 .put_task_with_state(curr.clone(), TaskState::Running, false);
//             current_run_queue.inner.resched_f()
//         } else {
//             Poll::Ready(())
//         }
//     }
// }

// /// Due not manually release the `current_run_queue.state`,
// /// otherwise it will cause double release.
// impl<'a, G: BaseGuard> Drop for YieldFuture<'a, G> {
//     fn drop(&mut self) {}
// }

// /// The `ExitFuture` used when exiting the current task
// /// with the specified exit code, which is always return `Poll::Pending`.
// ///
// /// The polling operation is as the same as the
// /// `current_run_queue::<NoPreemptIrqSave>().exit_current()` function.
// ///
// /// SAFETY: as the same as the `YieldFuture`. However, It wrap the `CurrentRunQueueRef`
// /// with `ManuallyDrop`, otherwise the `IRQ` and `Preempt` state of other
// /// tasks(maybe `main` or `gc` task) which recycle the exited task(which used this future)
// /// will be error due to automatically drop the `CurrentRunQueueRef.
// /// The `CurrentRunQueueRef` should never be drop.
// pub(crate) struct ExitFuture<'a, G: BaseGuard> {
//     current_run_queue: core::mem::ManuallyDrop<CurrentRunQueueRef<'a, G>>,
//     exit_code: i32,
// }

// impl<'a, G: BaseGuard> ExitFuture<'a, G> {
//     pub(crate) fn new(exit_code: i32) -> Self {
//         Self {
//             current_run_queue: core::mem::ManuallyDrop::new(current_run_queue::<G>()),
//             exit_code,
//         }
//     }
// }

// impl<'a, G: BaseGuard> Unpin for ExitFuture<'a, G> {}

// impl<'a, G: BaseGuard> Future for ExitFuture<'a, G> {
//     type Output = ();
//     fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
//         let Self {
//             current_run_queue,
//             exit_code,
//         } = self.get_mut();
//         let exit_code = *exit_code;
//         let curr = &current_run_queue.current_task;
//         assert!(curr.is_running(), "task is not running: {:?}", curr.state());
//         assert!(!curr.is_idle());
//         curr.set_state(TaskState::Exited);

//         // Notify the joiner task.
//         curr.notify_exit(exit_code);

//         // Safety: it is called from `current_run_queue::<NoPreemptIrqSave>().exit_current(exit_code)`,
//         // which disabled IRQs and preemption.
//         unsafe {
//             // Push current task to the `EXITED_TASKS` list, which will be consumed by the GC task.
//             EXITED_TASKS.current_ref_mut_raw().push_back(curr.clone());
//             // Wake up the GC task to drop the exited tasks.
//             WAIT_FOR_EXIT.current_ref_mut_raw().notify_one(false);
//         }

//         assert!(current_run_queue.inner.resched_f().is_pending());
//         Poll::Pending
//     }
// }

// #[cfg(feature = "irq")]
// pub(crate) struct SleepUntilFuture<'a, G: BaseGuard> {
//     current_run_queue: CurrentRunQueueRef<'a, G>,
//     deadline: axhal::time::TimeValue,
//     flag: bool,
// }

// #[cfg(feature = "irq")]
// impl<'a, G: BaseGuard> SleepUntilFuture<'a, G> {
//     pub fn new(deadline: axhal::time::TimeValue) -> Self {
//         Self {
//             current_run_queue: current_run_queue::<G>(),
//             deadline,
//             flag: false,
//         }
//     }
// }

// #[cfg(feature = "irq")]
// impl<'a, G: BaseGuard> Unpin for SleepUntilFuture<'a, G> {}

// #[cfg(feature = "irq")]
// impl<'a, G: BaseGuard> Future for SleepUntilFuture<'a, G> {
//     type Output = ();
//     fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
//         let Self {
//             current_run_queue,
//             deadline,
//             flag,
//         } = self.get_mut();
//         if !(*flag) {
//             *flag = !*flag;
//             let deadline = *deadline;
//             let curr = &current_run_queue.current_task;
//             assert!(curr.is_running());
//             assert!(!curr.is_idle());

//             let now = axhal::time::wall_time();
//             if now < deadline {
//                 crate::timers::set_alarm_wakeup(deadline, curr.clone());
//                 curr.set_state(TaskState::Blocked);
//                 assert!(current_run_queue.inner.resched_f().is_pending());
//                 Poll::Pending
//             } else {
//                 Poll::Ready(())
//             }
//         } else {
//             Poll::Ready(())
//         }
//     }
// }

// #[cfg(feature = "irq")]
// impl<'a, G: BaseGuard> Drop for SleepUntilFuture<'a, G> {
//     fn drop(&mut self) {}
// }

// /// The `BlockedReschedFuture` used when blocking the current task.
// ///
// /// When polling this future, current task will be put into the wait queue and reschedule,
// /// the state of current task will be marked as `Blocked`, set the `in_wait_queue` flag as true.
// /// Note:
// ///     1. When polling this future, the wait queue is locked.
// ///     2. When polling this future, the current task is in the running state.
// ///     3. When polling this future, the current task is not the idle task.
// ///     4. The lock of the wait queue will be released explicitly after current task is pushed into it.
// ///
// /// SAFETY:
// /// as the same as the `YieldFuture`. Due to the `WaitQueueGuard` is not implemented
// /// the `Send` trait, this future must hold the reference about the `WaitQueue` instead
// /// of the `WaitQueueGuard`.
// pub(crate) struct BlockedReschedFuture<'a, G: BaseGuard> {
//     current_run_queue: CurrentRunQueueRef<'a, G>,
//     wq: &'a WaitQueue,
//     flag: bool,
// }

// impl<'a, G: BaseGuard> BlockedReschedFuture<'a, G> {
//     pub fn new(current_run_queue: CurrentRunQueueRef<'a, G>, wq: &'a WaitQueue) -> Self {
//         Self {
//             current_run_queue,
//             wq,
//             flag: false,
//         }
//     }
// }

// impl<'a, G: BaseGuard> Unpin for BlockedReschedFuture<'a, G> {}

// impl<'a, G: BaseGuard> Future for BlockedReschedFuture<'a, G> {
//     type Output = ();
//     fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
//         let Self {
//             current_run_queue,
//             wq,
//             flag,
//         } = self.get_mut();
//         if !(*flag) {
//             *flag = !*flag;
//             let mut wq_guard = wq.queue.lock();
//             let curr = &current_run_queue.current_task;
//             assert!(curr.is_running());
//             assert!(!curr.is_idle());
//             // we must not block current task with preemption disabled.
//             // Current expected preempt count is 2.
//             // 1 for `NoPreemptIrqSave`, 1 for wait queue's `SpinNoIrq`.
//             #[cfg(feature = "preempt")]
//             assert!(curr.can_preempt(2));

//             // Mark the task as blocked, this has to be done before adding it to the wait queue
//             // while holding the lock of the wait queue.
//             curr.set_state(TaskState::Blocked);
//             curr.set_in_wait_queue(true);

//             wq_guard.push_back(curr.clone());
//             // Drop the lock of wait queue explictly.
//             drop(wq_guard);

//             // Current task's state has been changed to `Blocked` and added to the wait queue.
//             // Note that the state may have been set as `Ready` in `unblock_task()`,
//             // see `unblock_task()` for details.

//             assert!(current_run_queue.inner.resched_f().is_pending());
//             Poll::Pending
//         } else {
//             Poll::Ready(())
//         }
//     }
// }

// impl<'a, G: BaseGuard> Drop for BlockedReschedFuture<'a, G> {
//     fn drop(&mut self) {}
// }
