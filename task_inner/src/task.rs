use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU64, Ordering};
use core::{alloc::Layout, cell::UnsafeCell, fmt, ptr::NonNull};

#[cfg(feature = "preempt")]
use core::sync::atomic::AtomicUsize;

use memory_addr::VirtAddr;

use crossbeam::atomic::AtomicCell;
use hal::TaskContext;
#[cfg(feature = "tls")]
use hal::tls::TlsArea;

pub type AxCpuMask = cpumask::CpuMask<{ config::SMP }>;

// use crate::WaitQueue;
use crate::task_ext::AxTaskExt;

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
    name: &'static str,
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
    // wait_for_exit: WaitQueue,
    kstack: UnsafeCell<Option<TaskStack>>,
    ctx: UnsafeCell<TaskContext>,
    task_ext: AxTaskExt,

    #[cfg(feature = "tls")]
    tls: TlsArea,

    /// The future of coroutine task.
    pub(crate) future:
        UnsafeCell<Option<core::pin::Pin<*mut (dyn Future<Output = ()> + Send + 'static)>>>,
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

    /// Notify all tasks that join on this task.
    pub fn notify_exit(&self, exit_code: i32) {
        self.exit_code.store(exit_code, Ordering::Release);
        // self.wait_for_exit.notify_all(false);
    }
}

#[cfg(feature = "alloc")]
impl TaskInner {
    /// Create a new task with the given entry function and stack size.
    pub fn new<F>(entry: F, name: String, stack_size: usize) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        let mut t = Self::new_common(TaskId::new(), name);
        let kstack = TaskStack::alloc(memory_addr::align_up_4k(stack_size));

        #[cfg(feature = "tls")]
        let tls = VirtAddr::from(t.tls.tls_ptr() as usize);
        #[cfg(not(feature = "tls"))]
        let tls = VirtAddr::from(0);

        t.entry = Some(Box::into_raw(Box::new(entry)));
        t.ctx_mut().init(task_entry as usize, kstack.top(), tls);
        t.kstack = UnsafeCell::new(Some(kstack));
        if t.name == "idle" {
            t.is_idle = true;
        }
        t
    }

    /// Create a new task with the given future.
    pub fn new_f<F>(future: F, name: String) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut t = Self::new_common(TaskId::new(), name);
        t.future = UnsafeCell::new(Some(Box::pin(async {
            future.await;
            crate::exit_f(0).await
        })));
        t
    }

    /// Gets the name of the task.
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// Get a combined string of the task ID and name.
    pub fn id_name(&self) -> alloc::string::String {
        alloc::format!("Task({}, {:?})", self.id.as_u64(), self.name)
    }

    // /// Wait for the task to exit, and return the exit code.
    // ///
    // /// It will return immediately if the task has already exited (but not dropped).
    // pub fn join(&self) -> Option<i32> {
    //     self.wait_for_exit
    //         .wait_until(|| self.state() == TaskState::Exited);
    //     Some(self.exit_code.load(Ordering::Acquire))
    // }

    // /// Wait for the task to exit, and return the exit code.
    // ///
    // /// It will return immediately if the task has already exited (but not dropped).
    // pub async fn join_f(&self) -> Option<i32> {
    //     self.wait_for_exit
    //         .wait_until_f(|| self.state() == TaskState::Exited)
    //         .await;
    //     Some(self.exit_code.load(Ordering::Acquire))
    // }

    /// Returns the pointer to the user-defined task extended data.
    ///
    /// # Safety
    ///
    /// The caller should not access the pointer directly, use [`TaskExtRef::task_ext`]
    /// or [`TaskExtMut::task_ext_mut`] instead.
    ///
    /// [`TaskExtRef::task_ext`]: crate::task_ext::TaskExtRef::task_ext
    /// [`TaskExtMut::task_ext_mut`]: crate::task_ext::TaskExtMut::task_ext_mut
    pub unsafe fn task_ext_ptr(&self) -> *mut u8 {
        self.task_ext.as_ptr()
    }

    /// Initialize the user-defined task extended data.
    ///
    /// Returns a reference to the task extended data if it has not been
    /// initialized yet (empty), otherwise returns [`None`].
    pub fn init_task_ext<T: Sized>(&mut self, data: T) -> Option<&T> {
        if self.task_ext.is_empty() {
            self.task_ext.write(data).map(|data| &*data)
        } else {
            None
        }
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
            let stack = alloc_stack_for_coroutine();
            let kstack_top = stack.top();
            *kstack = Some(stack);
            let ctx = unsafe { &mut *self.ctx_mut_ptr() };
            #[cfg(feature = "tls")]
            let tls = VirtAddr::from(self.tls.tls_ptr() as usize);
            #[cfg(not(feature = "tls"))]
            let tls = VirtAddr::from(0);
            ctx.init(coroutine_schedule as usize, kstack_top, tls);
        }
    }
}

// private methods
#[cfg(feature = "alloc")]
impl TaskInner {
    fn new_common(id: TaskId, name: String) -> Self {
        Self {
            id,
            name,
            is_idle: false,
            is_init: false,
            entry: None,
            state: AtomicU8::new(TaskState::Ready as u8),
            // By default, the task is allowed to run on all CPUs.
            cpumask: AtomicCell::new(AxCpuMask::full()),
            in_wait_queue: AtomicBool::new(false),
            #[cfg(feature = "irq")]
            timer_ticket_id: AtomicU64::new(0),
            #[cfg(feature = "smp")]
            on_cpu: AtomicBool::new(false),
            #[cfg(feature = "preempt")]
            need_resched: AtomicBool::new(false),
            #[cfg(feature = "preempt")]
            preempt_disable_count: AtomicUsize::new(0),
            exit_code: AtomicI32::new(0),
            // wait_for_exit: WaitQueue::new(),
            kstack: UnsafeCell::new(None),
            ctx: UnsafeCell::new(TaskContext::new()),
            task_ext: AxTaskExt::empty(),
            #[cfg(feature = "tls")]
            tls: TlsArea::alloc(),
            future: UnsafeCell::new(None),
        }
    }

    /// Creates an "init task" using the current CPU states, to use as the
    /// current task.
    ///
    /// As it is the current task, no other task can switch to it until it
    /// switches out.
    ///
    /// And there is no need to set the `entry`, `kstack` or `tls` fields, as
    /// they will be filled automatically when the task is switches out.
    pub(crate) fn new_init(name: String) -> Self {
        let mut t = Self::new_common(TaskId::new(), name);
        t.is_init = true;
        #[cfg(feature = "smp")]
        t.set_on_cpu(true);
        if t.name == "idle" {
            t.is_idle = true;
        }
        t
    }

    #[cfg(feature = "alloc")]
    pub(crate) fn into_arc(self) -> AxTaskRef {
        Arc::new(AxTask::new(self))
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

    #[inline]
    #[cfg(feature = "preempt")]
    pub(crate) fn enable_preempt(&self, resched: bool) {
        if self.preempt_disable_count.fetch_sub(1, Ordering::Relaxed) == 1 && resched {
            // If current task is pending to be preempted, do rescheduling.
            Self::current_check_preempt_pending();
        }
    }

    #[cfg(feature = "preempt")]
    fn current_check_preempt_pending() {
        use kernel_guard::NoPreemptIrqSave;
        let curr = crate::current();
        if curr.need_resched.load(Ordering::Acquire) && curr.can_preempt(0) {
            // Note: if we want to print log msg during `preempt_resched`, we have to
            // disable preemption here, because the axlog may cause preemption.
            let mut rq = crate::current_run_queue::<NoPreemptIrqSave>();
            if curr.need_resched.load(Ordering::Acquire) {
                rq.preempt_resched()
            }
        }
    }

    /// Sets whether the task is running on a CPU.
    #[cfg(feature = "smp")]
    #[inline]
    pub(crate) fn set_on_cpu(&self, on_cpu: bool) {
        self.on_cpu.store(on_cpu, Ordering::Release)
    }
}

impl fmt::Debug for TaskInner {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TaskInner")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("state", &self.state())
            .finish()
    }
}

#[cfg(feature = "alloc")]
impl Drop for TaskInner {
    fn drop(&mut self) {}
}

struct TaskStack {
    ptr: NonNull<u8>,
    layout: Layout,
}

impl TaskStack {
    #[cfg(feature = "alloc")]
    pub fn alloc(size: usize) -> Self {
        let layout = Layout::from_size_align(size, 16).unwrap();
        Self {
            ptr: NonNull::new(unsafe { alloc::alloc::alloc(layout) }).unwrap(),
            layout,
        }
    }

    pub const fn top(&self) -> VirtAddr {
        unsafe { core::mem::transmute(self.ptr.as_ptr().add(self.layout.size())) }
    }
}
