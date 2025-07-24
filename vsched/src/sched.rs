
use base_task::{percpu_size_4k_aligned, TaskRef, BaseScheduler, PerCPU, TaskInner, TaskState};
use core::mem::MaybeUninit;
/// Safety:
///     the offset of this function in the `.text`
///     section must be little than 0x1000.
///     The `#[inline(never)]` attribute and the
///     offset requirement can make it work ok.
#[inline(never)]
#[unsafe(link_section = ".text.start")]
pub fn get_data_base() -> usize {
    let pc = unsafe { hal::asm::get_pc() };
    const VSCHED_DATA_SIZE: usize = config::SMP * percpu_size_4k_aligned::<TaskInner>();
    (pc & config::DATA_SEC_MASK) - VSCHED_DATA_SIZE
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
    let per_cpu_base = get_data_base() as *mut u8;
    let per_cpu = unsafe { 
        &*(per_cpu_base.add(index * percpu_size_4k_aligned::<TaskInner>()) as *mut PerCPU) 
    };
    per_cpu
}

/// Puts target task into current run queue with `Ready` state
/// if its state matches `current_state` (except idle task).
///
/// If `preempt`, keep current task's time slice, otherwise reset it.
///
/// Returns `true` if the target task is put into this run queue successfully,
/// otherwise `false`.
pub(crate) fn put_task_with_state(
    percpu: &'static PerCPU,
    task: TaskRef,
    current_state: TaskState,
    preempt: bool,
) -> bool {
    // If the task's state matches `current_state`, set its state to `Ready` and
    // put it back to the run queue (except idle task).
    if task
        .transition_state(current_state, TaskState::Ready)
        && !task.is_idle()
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
            while task.on_cpu() {
                // Wait for the task to finish its scheduling process.
                core::hint::spin_loop();
            }
        }
        // TODO: priority
        percpu.scheduler.put_prev_task(task, preempt);
        true
    } else {
        false
    }
}

/// Adds a task to the scheduler.
///
/// This function is used to add a new task to the scheduler.
pub fn add_task(percpu: &'static PerCPU, task: TaskRef) {
    assert!(task.is_ready());
    percpu.scheduler.add_task(task);
}

/// Unblock one task by inserting it into the run queue.
///
/// This function does nothing if the task is not in [`TaskState::Blocked`],
/// which means the task is already unblocked by other cores.
pub fn unblock_task(percpu: &'static PerCPU, task: TaskRef, resched: bool, src_cpu_id: usize) {
    // Try to change the state of the task from `Blocked` to `Ready`,
    // if successful, the task will be put into this run queue,
    // otherwise, the task is already unblocked by other cores.
    // Note:
    // target task can not be insert into the run queue until it finishes its scheduling process.
    if put_task_with_state(percpu, task, TaskState::Blocked, resched) {
        // Since now, the task to be unblocked is in the `Ready` state.
        // Note: when the task is unblocked on another CPU's run queue,
        // we just ingiore the `resched` flag.
        if resched && src_cpu_id == percpu.cpu_id {
            // TODO: 增加判断当前任务的条件
            unsafe { 
                get_run_queue(src_cpu_id)
                .current_task
                .as_ref_unchecked()
                .set_preempt_pending(true);
            };
        }
    }
}

pub fn task_tick(percpu: &'static PerCPU, task: &TaskRef) -> bool {
    percpu.scheduler.task_tick(task)
}

/// Yield the current task and reschedule.
/// This function will put the current task into this run queue with `Ready` state,
/// and reschedule to the next task on this run queue.
pub fn yield_current(percpu: &'static PerCPU) {
    let curr = unsafe { percpu.current_task.as_ref_unchecked() };
    assert!(curr.is_running());
    put_task_with_state(percpu, curr.clone(), TaskState::Running, false);
    resched(percpu);
}

/// Yield the current task and reschedule.
/// This function will put the current task into this run queue with `Ready` state,
/// and reschedule to the next task on this run queue.
pub fn preempt_current(percpu: &'static PerCPU) {
    let curr = unsafe { percpu.current_task.as_ref_unchecked() };
    assert!(curr.is_running());
    put_task_with_state(percpu, curr.clone(), TaskState::Running, true);
    resched(percpu);
}

pub fn set_current_priority(percpu: &'static PerCPU, prio: isize) -> bool {
    percpu.scheduler
        .set_priority(unsafe { percpu.current_task.as_ref_unchecked() }, prio)
}

/// Core reschedule subroutine.
/// Pick the next task to run and switch to it.
pub(crate) fn resched(percpu: &'static PerCPU) {
    let next = percpu.scheduler.pick_next_task().unwrap_or_else(|| 
        // Safety: IRQs must be disabled at this time.
        percpu.idle_task.clone()
    );
    assert!(
        next.is_ready()
    );
    let prev_task = unsafe { percpu.current_task.as_ref_unchecked() };
    switch_to(percpu, prev_task, next);
}

pub(crate) fn switch_to(percpu: &'static PerCPU, prev_task: &TaskRef, next_task: TaskRef) {
    next_task.set_preempt_pending(false);
    next_task.set_state(TaskState::Running);
    if prev_task.ptr_eq(&next_task) {
        return;
    }

    // Claim the task as running, we do this before switching to it
    // such that any running task will have this set.
    next_task.set_on_cpu(true);

    unsafe {
        let prev_ctx_ptr = prev_task.ctx_mut_ptr();
        let next_ctx_ptr = next_task.ctx_mut_ptr();
        // TODO:
        // If the next task is a coroutine, this will set the kstack and ctx.
        next_task.set_kstack();

        // Store the weak pointer of **prev_task** in percpu variable `PREV_TASK`.
        percpu.prev_task.replace(MaybeUninit::new(prev_task.clone()));

        percpu.current_task.replace(next_task);


        (*prev_ctx_ptr).switch_to(&*next_ctx_ptr);

        // Current it's **next_task** running on this CPU, clear the `prev_task`'s `on_cpu` field
        // to indicate that it has finished its scheduling process and no longer running on this CPU.
        // 这里会因为任务进行了切换，运行到了其他的 CPU 上，但是 percpu 的数据还是之前运行的 CPU
        // 因此，清除前一个任务的 CPU 任务时，不是清除当前 CPU 上的前一个任务
    }
}

#[inline(never)]
pub(crate) fn clear_prev_task_on_cpu(percpu: &'static PerCPU) {
    unsafe { percpu.prev_task.as_mut_unchecked().assume_init_ref().set_on_cpu(false) };
}

/// Core reschedule subroutine.
/// Pick the next task to run and switch to it.
/// This function is only used in `YieldFuture`, `ExitFuture`,
/// `SleepUntilFuture` and `BlockedReschedFuture`.
pub(crate) fn resched_f(percpu: &'static PerCPU) -> bool {
    let next_task = percpu.scheduler.pick_next_task().unwrap_or_else(|| 
        // Safety: IRQs must be disabled at this time.
        percpu.idle_task.clone()
    );
    assert!(
        next_task.is_ready(),
    );
    let prev_task = unsafe { percpu.current_task.as_ref_unchecked() };
    
    next_task.set_preempt_pending(false);
    next_task.set_state(TaskState::Running);
    if prev_task.ptr_eq(&next_task) {
        return false;
    }

    // Claim the task as running, we do this before switching to it
    // such that any running task will have this set.
    next_task.set_on_cpu(true);

    unsafe {
        
        percpu.prev_task.replace(MaybeUninit::new(prev_task.clone()));                

        // // The strong reference count of `prev_task` will be decremented by 1,
        // // but won't be dropped until `gc_entry()` is called.
        // assert!(Arc::strong_count(prev_task.as_task_ref()) > 1);
        // assert!(Arc::strong_count(&next_task) >= 1);

        // Directly change the `CurrentTask` and return `Pending`.
        percpu.current_task.replace(next_task);
        true
    }
}




