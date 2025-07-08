use crate::wait_queue::WaitQueue;
use core::cell::UnsafeCell;
#[cfg(feature = "irq")]
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;
use core::sync::atomic::{AtomicBool, AtomicI32};
use std::ptr::NonNull;
use std::sync::Arc;

use base_task::{BaseTask, BaseTaskRef, TaskExtRef, TaskInner, TaskStack, TaskState};

/// Task extended data for the monolithic kernel.
pub struct TaskExt {
    // 以下字段都需要在 TaskExt 中定义
    name: String,
    entry: Option<*mut dyn FnOnce()>,
    /// Mark whether the task is in the wait queue.
    in_wait_queue: AtomicBool,
    /// A ticket ID used to identify the timer event.
    /// Set by `set_timer_ticket()` when creating a timer event in `set_alarm_wakeup()`,
    /// expired by setting it as zero in `timer_ticket_expired()`, which is called by `cancel_events()`.
    #[cfg(feature = "irq")]
    timer_ticket_id: AtomicU64,

    #[cfg(feature = "preempt")]
    preempt_disable_count: AtomicUsize,
    exit_code: AtomicI32,
    wait_for_exit: WaitQueue,
    /// The future of coroutine task.
    pub future: UnsafeCell<Option<core::pin::Pin<Box<dyn Future<Output = ()> + Send + 'static>>>>,
}

impl TaskExt {
    /// Gets the name of the task.
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    #[inline]
    pub(crate) fn in_wait_queue(&self) -> bool {
        self.in_wait_queue.load(Ordering::Acquire)
    }

    #[inline]
    pub(crate) fn set_in_wait_queue(&self, in_wait_queue: bool) {
        self.in_wait_queue.store(in_wait_queue, Ordering::Release);
    }

    /// Returns task's current timer ticket ID.
    #[inline]
    #[cfg(feature = "irq")]
    pub(crate) fn timer_ticket(&self) -> u64 {
        self.timer_ticket_id.load(Ordering::Acquire)
    }

    /// Set the timer ticket ID.
    #[inline]
    #[cfg(feature = "irq")]
    pub(crate) fn set_timer_ticket(&self, timer_ticket_id: u64) {
        // CAN NOT set timer_ticket_id to 0,
        // because 0 is used to indicate the timer event is expired.
        assert!(timer_ticket_id != 0);
        self.timer_ticket_id
            .store(timer_ticket_id, Ordering::Release);
    }

    /// Expire timer ticket ID by setting it to 0,
    /// it can be used to identify one timer event is triggered or expired.
    #[inline]
    #[cfg(feature = "irq")]
    pub(crate) fn timer_ticket_expired(&self) {
        self.timer_ticket_id.store(0, Ordering::Release);
    }

    #[inline]
    #[cfg(feature = "preempt")]
    pub(crate) fn can_preempt(&self, current_disable_count: usize) -> bool {
        self.preempt_disable_count.load(Ordering::Acquire) == current_disable_count
    }

    #[inline]
    #[cfg(feature = "preempt")]
    pub(crate) fn disable_preempt(&self) {
        self.preempt_disable_count.fetch_add(1, Ordering::Release);
    }

    /// Notify all tasks that join on this task.
    pub fn notify_exit(&self, exit_code: i32) {
        self.exit_code.store(exit_code, Ordering::Release);
        self.wait_for_exit.notify_all(false);
    }

    pub fn new<F>(entry: F, name: String) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        Self {
            name,
            entry: Some(Box::into_raw(Box::new(entry))),
            in_wait_queue: AtomicBool::new(false),
            exit_code: AtomicI32::new(0),
            wait_for_exit: WaitQueue::new(),
            future: UnsafeCell::new(None),
        }
    }

    pub fn new_init(name: String) -> Self {
        Self {
            name,
            entry: None,
            in_wait_queue: AtomicBool::new(false),
            exit_code: AtomicI32::new(0),
            wait_for_exit: WaitQueue::new(),
            future: UnsafeCell::new(None),
        }
    }
}

base_task::def_task_ext!(TaskExt);

#[repr(transparent)]
pub struct Task {
    inner: BaseTask,
}

impl Task {
    fn task_ext_(&self) -> &TaskExt {
        self.inner.task_ext()
    }

    /// Wait for the task to exit, and return the exit code.
    ///
    /// It will return immediately if the task has already exited (but not dropped).
    pub fn join(&self) -> Option<i32> {
        self.task_ext_()
            .wait_for_exit
            .wait_until(|| self.inner.state() == TaskState::Exited);
        Some(self.task_ext_().exit_code.load(Ordering::Acquire))
    }

    // /// Wait for the task to exit, and return the exit code.
    // ///
    // /// It will return immediately if the task has already exited (but not dropped).
    // pub async fn join_f(&self) -> Option<i32> {
    //     self.task_ext_()
    //         .wait_for_exit
    //         .wait_until_f(|| self.inner.state() == TaskState::Exited)
    //         .await;
    //     Some(self.task_ext_().exit_code.load(Ordering::Acquire))
    // }

    pub fn new<F>(entry: F, name: String, stack_size: usize) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        let mut t = TaskInner::new();
        t.init_task_ext(TaskExt::new(entry, name));
        unsafe {
            *t.kernel_stack() = Some(TaskStack::alloc(stack_size));
        }
        let kstack_top = t.kernel_stack_top().unwrap();
        t.ctx_mut().init(task_entry as usize, kstack_top);

        Self {
            inner: BaseTask::new(t),
        }
    }

    pub fn new_init(name: String) -> Self {
        let mut t = TaskInner::new();
        t.set_init(true);
        t.set_on_cpu(true);
        if name == "idle" {
            t.set_idle(true);
        }
        t.init_task_ext(TaskExt::new_init(name));
        Self {
            inner: BaseTask::new(t),
        }
    }

    pub fn into_ref(self) -> BaseTaskRef {
        BaseTaskRef::new(NonNull::new(Arc::into_raw(Arc::new(self)) as _).unwrap())
    }
}

extern "C" fn task_entry() {
    let task = vsched_apis::current(0);
    if let Some(entry) = task.as_ref().task_ext().entry {
        unsafe { Box::from_raw(entry)() };
    }
    crate::vsched::exit(0);
}
