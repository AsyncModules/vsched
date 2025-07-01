use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU64, Ordering};
use core::{alloc::Layout, cell::UnsafeCell, ptr::NonNull};

#[cfg(feature = "preempt")]
use core::sync::atomic::AtomicUsize;

use crossbeam::atomic::AtomicCell;
use hal::TaskContext;
#[cfg(feature = "tls")]
use hal::tls::TlsArea;
use memory_addr::VirtAddr;

pub type AxCpuMask = cpumask::CpuMask<{ config::SMP }>;

/// A unique identifier for a thread.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TaskId(u64);

/// The possible states of a task.
#[repr(u8)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TaskState {
    /// Task is running on some CPU.
    Running = 1,
    /// Task is ready to run on some scheduler's ready queue.
    Ready = 2,
    /// Task is blocked (in the wait queue or timer list),
    /// and it has finished its scheduling process, it can be wake up by `notify()` on any run queue safely.
    Blocked = 3,
    /// Task is exited and waiting for being dropped.
    Exited = 4,
}

/// The inner task structure.
pub struct TaskInner {
    id: TaskId,
    is_idle: bool,
    is_init: bool,

    entry: Option<*mut dyn FnOnce()>,
    state: AtomicU8,

    /// CPU affinity mask.
    cpumask: AtomicCell<AxCpuMask>,

    /// Mark whether the task is in the wait queue.
    in_wait_queue: AtomicBool,

    /// Used to indicate whether the task is running on a CPU.
    #[cfg(feature = "smp")]
    on_cpu: AtomicBool,

    /// A ticket ID used to identify the timer event.
    /// Set by `set_timer_ticket()` when creating a timer event in `set_alarm_wakeup()`,
    /// expired by setting it as zero in `timer_ticket_expired()`, which is called by `cancel_events()`.
    #[cfg(feature = "irq")]
    timer_ticket_id: AtomicU64,

    #[cfg(feature = "preempt")]
    need_resched: AtomicBool,
    #[cfg(feature = "preempt")]
    preempt_disable_count: AtomicUsize,

    exit_code: AtomicI32,
    wait_for_exit: WaitQueue,
    kstack: UnsafeCell<Option<TaskStack>>,
    ctx: UnsafeCell<TaskContext>,
    task_ext: AxTaskExt,

    #[cfg(feature = "tls")]
    tls: TlsArea,

    /// The future of coroutine task.
    pub(crate) future: UnsafeCell<Option<*mut (dyn Future<Output = ()> + Send + 'static)>>,
}

impl TaskId {
    fn new() -> Self {
        static ID_COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// Convert the task ID to a `u64`.
    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

impl From<u8> for TaskState {
    #[inline]
    fn from(state: u8) -> Self {
        match state {
            1 => Self::Running,
            2 => Self::Ready,
            3 => Self::Blocked,
            4 => Self::Exited,
            _ => unreachable!(),
        }
    }
}

unsafe impl Send for TaskInner {}
unsafe impl Sync for TaskInner {}

impl TaskInner {
    /// Gets the ID of the task.
    #[inline]
    pub const fn id(&self) -> TaskId {
        self.id
    }

    /// Returns a mutable reference to the task context.
    #[inline]
    pub const fn ctx_mut(&mut self) -> &mut TaskContext {
        self.ctx.get_mut()
    }

    #[inline]
    pub const unsafe fn ctx_mut_ptr(&self) -> *mut TaskContext {
        self.ctx.get()
    }
    #[inline]
    pub fn state(&self) -> TaskState {
        self.state.load(Ordering::Acquire).into()
    }

    #[inline]
    pub fn set_state(&self, state: TaskState) {
        self.state.store(state as u8, Ordering::Release)
    }

    /// Transition the task state from `current_state` to `new_state`,
    /// Returns `true` if the current state is `current_state` and the state is successfully set to `new_state`,
    /// otherwise returns `false`.
    #[inline]
    pub fn transition_state(&self, current_state: TaskState, new_state: TaskState) -> bool {
        self.state
            .compare_exchange(
                current_state as u8,
                new_state as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    #[inline]
    pub fn is_running(&self) -> bool {
        matches!(self.state(), TaskState::Running)
    }

    #[inline]
    pub fn is_ready(&self) -> bool {
        matches!(self.state(), TaskState::Ready)
    }

    #[inline]
    pub const fn is_init(&self) -> bool {
        self.is_init
    }

    #[inline]
    pub const fn is_idle(&self) -> bool {
        self.is_idle
    }

    /// Gets the cpu affinity mask of the task.
    ///
    /// Returns the cpu affinity mask of the task in type [`AxCpuMask`].
    #[inline]
    pub fn cpumask(&self) -> AxCpuMask {
        self.cpumask.load()
    }

    /// Sets the cpu affinity mask of the task.
    ///
    /// # Arguments
    /// `cpumask` - The cpu affinity mask to be set in type [`AxCpuMask`].
    #[inline]
    pub fn set_cpumask(&self, cpumask: AxCpuMask) {
        self.cpumask.store(cpumask);
    }

    /// Returns whether the task is running on a CPU.
    ///
    /// It is used to protect the task from being moved to a different run queue
    /// while it has not finished its scheduling process.
    /// The `on_cpu field is set to `true` when the task is preparing to run on a CPU,
    /// and it is set to `false` when the task has finished its scheduling process in `clear_prev_task_on_cpu()`.
    #[cfg(feature = "smp")]
    #[inline]
    pub fn on_cpu(&self) -> bool {
        self.on_cpu.load(Ordering::Acquire)
    }
}

impl TaskInner {
    /// Returns the top address of the kernel stack.
    #[inline]
    pub const fn kernel_stack_top(&self) -> Option<VirtAddr> {
        match unsafe { &*self.kstack.get() } {
            Some(s) => Some(s.top()),
            None => None,
        }
    }

    /// Get the mut ref about the `kstack` field.
    #[inline]
    const unsafe fn kernel_stack(&self) -> *mut Option<TaskStack> {
        self.kstack.get()
    }

    // TODO
    // /// Once the `kstack` field is None, the task is a coroutine.
    // /// The `kstack` and the `ctx` will be set up,
    // /// so the next coroutine will start at `coroutine_schedule` function.
    // ///
    // /// This function is only used before switching task.
    // pub(crate) fn set_kstack(&self) {
    //     let kstack = unsafe { &mut *self.kernel_stack() };
    //     if kstack.is_none() && !self.is_init && !self.is_idle {
    //         let stack = alloc_stack_for_coroutine();
    //         let kstack_top = stack.top();
    //         *kstack = Some(stack);
    //         let ctx = unsafe { &mut *self.ctx_mut_ptr() };
    //         #[cfg(feature = "tls")]
    //         let tls = VirtAddr::from(self.tls.tls_ptr() as usize);
    //         #[cfg(not(feature = "tls"))]
    //         let tls = VirtAddr::from(0);
    //         ctx.init(coroutine_schedule as usize, kstack_top, tls);
    //     }
    // }
}

// private methods
impl TaskInner {
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
    pub(crate) fn set_preempt_pending(&self, pending: bool) {
        self.need_resched.store(pending, Ordering::Release)
    }

    #[inline]
    #[cfg(feature = "preempt")]
    pub(crate) fn can_preempt(&self, current_disable_count: usize) -> bool {
        self.preempt_disable_count.load(Ordering::Acquire) == current_disable_count
    }

    #[inline]
    #[cfg(feature = "preempt")]
    pub(crate) fn disable_preempt(&self) {
        self.preempt_disable_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Sets whether the task is running on a CPU.
    #[cfg(feature = "smp")]
    #[inline]
    pub(crate) fn set_on_cpu(&self, on_cpu: bool) {
        self.on_cpu.store(on_cpu, Ordering::Release)
    }
}

struct TaskStack {
    ptr: NonNull<u8>,
    layout: Layout,
}

impl TaskStack {
    pub const fn top(&self) -> VirtAddr {
        unsafe { core::mem::transmute(self.ptr.as_ptr().add(self.layout.size())) }
    }
}

/// A wrapper of pointer to the task extended data.
pub(crate) struct AxTaskExt {
    ptr: *mut u8,
}

use crate::AxTaskRef;
use config::RQ_CAP;
use scheduler::LockFreeDeque;

/// Only the structure not the method
pub struct WaitQueue {
    pub queue: LockFreeDeque<AxTaskRef, RQ_CAP>,
}

impl WaitQueue {
    /// Creates an empty wait queue.
    pub const fn new() -> Self {
        Self {
            queue: LockFreeDeque::new(),
        }
    }

    /// Returns the number of tasks in the wait queue.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Returns true if the wait queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}
