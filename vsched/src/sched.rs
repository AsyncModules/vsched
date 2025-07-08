use crate::api::BaseTaskRef;
use crate::percpu::PerCPU;
use config::AxCpuMask;
use scheduler::BaseScheduler;
use crate::task::TaskState;

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
// The modulo operation is safe here because `axconfig::SMP` is always greater than 1 with "smp" enabled.
#[allow(clippy::modulo_one)]
#[inline]
pub(crate) fn select_run_queue_index(cpumask: AxCpuMask) -> usize {
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
pub(crate) fn select_run_queue(task: BaseTaskRef) -> &'static PerCPU {
    
    // When SMP is enabled, select the run queue based on the task's CPU affinity and load balance.
    let index = select_run_queue_index(task.as_ref().cpumask());
    get_run_queue(index)
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
        task: &BaseTaskRef,
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
    pub fn add_task(&self, task: BaseTaskRef) {
        assert!(task.as_ref().is_ready());
        self.scheduler.add_task(task);
    }

    /// Unblock one task by inserting it into the run queue.
    ///
    /// This function does nothing if the task is not in [`TaskState::Blocked`],
    /// which means the task is already unblocked by other cores.
    pub fn unblock_task(&self, task: BaseTaskRef, resched: bool, src_cpu_id: usize) {
        let _task_id = task.as_ref().id();
        // Try to change the state of the task from `Blocked` to `Ready`,
        // if successful, the task will be put into this run queue,
        // otherwise, the task is already unblocked by other cores.
        // Note:
        // target task can not be insert into the run queue until it finishes its scheduling process.
        if self.put_task_with_state(&task, TaskState::Blocked, resched) {
            // Since now, the task to be unblocked is in the `Ready` state.
            // Note: when the task is unblocked on another CPU's run queue,
            // we just ingiore the `resched` flag.
            if resched && src_cpu_id == self.cpu_id {
                // TODO: 增加判断当前任务的条件
                unsafe { 
                    get_run_queue(src_cpu_id)
                    .current_task
                    .as_ref_unchecked()
                    .as_ref()
                    .set_preempt_pending(true);
                };
            }
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

    pub fn set_current_priority(&self, prio: isize) -> bool {
        self.scheduler
            .set_priority(unsafe { self.current_task.as_ref_unchecked() }, prio)
    }

    /// Core reschedule subroutine.
    /// Pick the next task to run and switch to it.
    pub(crate) fn resched(&self) {
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

    pub(crate) fn switch_to(&self, prev_task: &BaseTaskRef, next_task: BaseTaskRef) {
        next_task.as_ref().set_preempt_pending(false);
        next_task.as_ref().set_state(TaskState::Running);
        if prev_task.ptr_eq(&next_task) {
            return;
        }

        // Claim the task as running, we do this before switching to it
        // such that any running task will have this set.
        next_task.as_ref().set_on_cpu(true);

        unsafe {
            let prev_ctx_ptr = prev_task.as_ref().ctx_mut_ptr();
            let next_ctx_ptr = next_task.as_ref().ctx_mut_ptr();
            // TODO:
            // If the next task is a coroutine, this will set the kstack and ctx.
            next_task.as_ref().set_kstack();

            // Store the weak pointer of **prev_task** in percpu variable `PREV_TASK`.
            self.prev_task.replace(prev_task.clone());

            *self.current_task.as_mut_unchecked() = next_task;
            // CurrentTask::set_current(prev_task, next_task);

            (*prev_ctx_ptr).switch_to(&*next_ctx_ptr);

            // Current it's **next_task** running on this CPU, clear the `prev_task`'s `on_cpu` field
            // to indicate that it has finished its scheduling process and no longer running on this CPU.
            self.prev_task.as_ref_unchecked().as_ref().set_on_cpu(false);
        }
    }

    /// Core reschedule subroutine.
    /// Pick the next task to run and switch to it.
    /// This function is only used in `YieldFuture`, `ExitFuture`,
    /// `SleepUntilFuture` and `BlockedReschedFuture`.
    pub(crate) fn resched_f(&self) -> bool {
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
        
        next_task.as_ref().set_preempt_pending(false);
        next_task.as_ref().set_state(TaskState::Running);
        if prev_task.ptr_eq(&next_task) {
            return true;
        }

        // Claim the task as running, we do this before switching to it
        // such that any running task will have this set.
        next_task.as_ref().set_on_cpu(true);

        unsafe {
            
            self.prev_task.replace(prev_task.clone());
                

            // // The strong reference count of `prev_task` will be decremented by 1,
            // // but won't be dropped until `gc_entry()` is called.
            // assert!(Arc::strong_count(prev_task.as_task_ref()) > 1);
            // assert!(Arc::strong_count(&next_task) >= 1);

            // Directly change the `CurrentTask` and return `Pending`.
            // CurrentTask::set_current(prev_task, next_task);
            *self.current_task.as_mut_unchecked() = next_task;
            false
        }
    }
}



