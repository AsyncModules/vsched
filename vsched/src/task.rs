use config::AxCpuMask;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use core::{alloc::Layout, cell::UnsafeCell, ptr::NonNull};

use crossbeam::atomic::AtomicCell;
use hal::TaskContext;
use memory_addr::VirtAddr;

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
    state: AtomicU8,
    /// CPU affinity mask.
    cpumask: AtomicCell<AxCpuMask>,
    /// Used to indicate whether the task is running on a CPU.
    on_cpu: AtomicBool,
    need_resched: AtomicBool,
    kstack: UnsafeCell<Option<TaskStack>>,
    ctx: UnsafeCell<TaskContext>,
    alloc_stack: Option<usize>,
    coroutine_schedule: Option<usize>,
    #[allow(unused)]
    task_ext: AxTaskExt,
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
    #[inline]
    pub fn on_cpu(&self) -> bool {
        self.on_cpu.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_preempt_pending(&self, pending: bool) {
        self.need_resched.store(pending, Ordering::Release)
    }

    /// Sets whether the task is running on a CPU.
    #[inline]
    pub(crate) fn set_on_cpu(&self, on_cpu: bool) {
        self.on_cpu.store(on_cpu, Ordering::Release)
    }

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

    /// Once the `kstack` field is None, the task is a coroutine.
    /// The `kstack` and the `ctx` will be set up,
    /// so the next coroutine will start at `coroutine_schedule` function.
    ///
    /// This function is only used before switching task.
    pub(crate) fn set_kstack(&self) {
        let kstack = unsafe { &mut *self.kernel_stack() };
        if kstack.is_none() && !self.is_init && !self.is_idle {
            let alloc_stack_fn: fn() -> TaskStack =
                unsafe { core::mem::transmute(self.alloc_stack.unwrap()) };
            let stack = alloc_stack_fn();
            let kstack_top = stack.top();
            *kstack = Some(stack);
            let ctx = unsafe { &mut *self.ctx_mut_ptr() };
            ctx.init(self.coroutine_schedule.unwrap(), kstack_top);
        }
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
    #[allow(unused)]
    ptr: *mut u8,
}
