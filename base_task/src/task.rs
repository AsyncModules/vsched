#[cfg(feature = "alloc")]
use {
    crate::TaskExt,
    crate::wait_queue::WaitQueue,
    alloc::{boxed::Box, string::String},
    config::AxCpuMask,
    core::fmt,
    core::sync::atomic::AtomicI32,
    crossbeam::atomic::AtomicCell,
    // #[cfg(feature = "tls")]
    // axhal::tls::TlsArea
    memory_addr::align_up_4k,
};

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use core::{alloc::Layout, cell::UnsafeCell, ptr::NonNull};
// #[cfg(feature = "preempt")]
use core::sync::atomic::AtomicUsize;
use memory_addr::VirtAddr;

use hal::TaskContext;

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

#[cfg(not(feature = "alloc"))]
#[repr(C)]
pub struct TaskInner {
    alloc_stack: Option<usize>,
    coroutine_schedule: Option<usize>,
    id: TaskId,
    is_idle: bool,
    is_init: bool,
    state: AtomicU8,
    /// Used to indicate whether the task is running on a CPU.
    // #[cfg(feature = "smp")]
    on_cpu: AtomicBool,
    /// Mark whether the task is in the wait queue.
    in_wait_queue: AtomicBool,
    /// A ticket ID used to identify the timer event.
    /// Set by `set_timer_ticket()` when creating a timer event in `set_alarm_wakeup()`,
    /// expired by setting it as zero in `timer_ticket_expired()`, which is called by `cancel_events()`.
    // #[cfg(feature = "irq")]
    timer_ticket_id: AtomicU64,
    // #[cfg(feature = "preempt")]
    need_resched: AtomicBool,
    // #[cfg(feature = "preempt")]
    preempt_disable_count: AtomicUsize,
    kstack: UnsafeCell<Option<TaskStack>>,
    ctx: UnsafeCell<TaskContext>,
}

#[cfg(feature = "alloc")]
#[repr(C)]
pub struct TaskInner {
    alloc_stack: Option<usize>,
    coroutine_schedule: Option<usize>,
    id: TaskId,
    is_idle: bool,
    is_init: bool,
    state: AtomicU8,
    /// Used to indicate whether the task is running on a CPU.
    // #[cfg(feature = "smp")]
    on_cpu: AtomicBool,
    /// Mark whether the task is in the wait queue.
    in_wait_queue: AtomicBool,
    /// A ticket ID used to identify the timer event.
    /// Set by `set_timer_ticket()` when creating a timer event in `set_alarm_wakeup()`,
    /// expired by setting it as zero in `timer_ticket_expired()`, which is called by `cancel_events()`.
    // #[cfg(feature = "irq")]
    timer_ticket_id: AtomicU64,
    // #[cfg(feature = "preempt")]
    need_resched: AtomicBool,
    // #[cfg(feature = "preempt")]
    preempt_disable_count: AtomicUsize,
    kstack: UnsafeCell<Option<TaskStack>>,
    ctx: UnsafeCell<TaskContext>,
    ext: TaskInnerExt,
}

#[cfg(feature = "alloc")]
#[repr(C)]
pub struct TaskInnerExt {
    name: String,
    exit_code: AtomicI32,
    wait_for_exit: WaitQueue,
    entry: Option<*mut dyn FnOnce()>,
    /// CPU affinity mask.
    cpumask: AtomicCell<AxCpuMask>,
    // #[cfg(feature = "tls")]
    // tls: TlsArea,
    /// The future of coroutine task.
    future: UnsafeCell<Option<core::pin::Pin<Box<dyn Future<Output = ()> + Send + 'static>>>>,
    task_ext: TaskExt,
}

impl TaskId {
    #[cfg(feature = "alloc")]
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
    #[cfg(feature = "alloc")]
    fn new_common(id: TaskId, name: String) -> Self {
        Self {
            alloc_stack: None,
            coroutine_schedule: None,
            id,
            is_idle: false,
            is_init: false,
            state: AtomicU8::new(TaskState::Ready as u8),
            // By default, the task is allowed to run on all CPUs.
            // #[cfg(feature = "irq")]
            timer_ticket_id: AtomicU64::new(0),
            // #[cfg(feature = "smp")]
            on_cpu: AtomicBool::new(false),
            in_wait_queue: AtomicBool::new(false),
            // #[cfg(feature = "preempt")]
            need_resched: AtomicBool::new(false),
            // #[cfg(feature = "preempt")]
            preempt_disable_count: AtomicUsize::new(0),
            kstack: UnsafeCell::new(None),
            ctx: UnsafeCell::new(TaskContext::new()),
            #[cfg(feature = "tls")]
            tls: TlsArea::alloc(),
            ext: TaskInnerExt {
                name,
                exit_code: AtomicI32::new(0),
                wait_for_exit: WaitQueue::new(),
                entry: None,
                cpumask: AtomicCell::new(AxCpuMask::full()),
                // #[cfg(feature = "tls")]
                // tls: TlsArea,
                future: UnsafeCell::new(None),
                task_ext: TaskExt::empty(),
            },
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
    #[cfg(feature = "alloc")]
    pub fn new_init(name: String) -> Self {
        let mut t = Self::new_common(TaskId::new(), name);
        t.is_init = true;
        // #[cfg(feature = "smp")]
        t.set_on_cpu(true);
        if t.ext.name == "idle" {
            t.is_idle = true;
        }
        t
    }

    /// Create a new task with the given entry function and stack size.
    #[cfg(feature = "alloc")]
    pub fn new<F>(entry: F, task_entry: usize, name: String, stack_size: usize) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        let mut t = Self::new_common(TaskId::new(), name);
        debug!("new task: {}", t.id_name());
        let kstack = TaskStack::alloc(align_up_4k(stack_size));

        // #[cfg(feature = "tls")]
        // let tls = VirtAddr::from(t.tls.tls_ptr() as usize);
        // #[cfg(not(feature = "tls"))]
        // let tls = VirtAddr::from(0);

        t.ext.entry = Some(Box::into_raw(Box::new(entry)));
        t.ctx_mut().init(task_entry as usize, kstack.top());
        t.kstack = UnsafeCell::new(Some(kstack));
        if t.ext.name == "idle" {
            t.is_idle = true;
        }
        t
    }

    /// Create a new task with the given future.
    #[cfg(feature = "alloc")]
    pub fn new_f<F>(future: F, name: String) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut t = Self::new_common(TaskId::new(), name);
        debug!("new task: {}", t.id_name());
        t.ext.future = UnsafeCell::new(Some(Box::pin(future)));
        t
    }

    /// Gets the entry of the task.
    #[cfg(feature = "alloc")]
    pub const fn entry(&self) -> &Option<*mut dyn FnOnce()> {
        &self.ext.entry
    }

    /// Gets the future of the task.
    #[cfg(feature = "alloc")]
    pub const fn future(
        &self,
    ) -> &mut Option<core::pin::Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        unsafe { self.ext.future.as_mut_unchecked() }
    }

    /// Gets the ID of the task.
    pub const fn id(&self) -> TaskId {
        self.id
    }

    /// Gets the name of the task.
    #[cfg(feature = "alloc")]
    pub fn name(&self) -> &str {
        self.ext.name.as_str()
    }

    /// Get a combined string of the task ID and name.
    #[cfg(feature = "alloc")]
    pub fn id_name(&self) -> alloc::string::String {
        alloc::format!("Task({}, {:?})", self.id.as_u64(), self.ext.name)
    }

    /// Wait for the task to exit, and return the exit code.
    ///
    /// It will return immediately if the task has already exited (but not dropped).
    #[cfg(feature = "alloc")]
    pub fn join(&self) -> Option<i32> {
        unsafe extern "Rust" {
            fn join(task: &TaskInner);
        }
        unsafe {
            join(self);
        }
        Some(self.exit_code())
    }

    /// Wait for the task to exit, and return the exit code.
    ///
    /// It will return immediately if the task has already exited (but not dropped).
    #[cfg(feature = "alloc")]
    pub async fn join_f(&self) -> Option<i32> {
        unsafe extern "C" {
            static JOIN_FUTURE: usize;
        }
        use alloc::boxed::Box;
        use core::{future::Future, pin::Pin};
        type BoxJoinFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

        unsafe {
            let join_fut: fn(task: &TaskInner) -> BoxJoinFuture = core::mem::transmute(JOIN_FUTURE);
            let join_fut = join_fut(self);
            join_fut.await
        }
        Some(self.exit_code())
    }

    /// Returns the pointer to the user-defined task extended data.
    ///
    /// # Safety
    ///
    /// The caller should not access the pointer directly, use [`TaskExtRef::task_ext`]
    /// or [`TaskExtMut::task_ext_mut`] instead.
    ///
    /// [`TaskExtRef::task_ext`]: crate::task_ext::TaskExtRef::task_ext
    /// [`TaskExtMut::task_ext_mut`]: crate::task_ext::TaskExtMut::task_ext_mut
    #[cfg(feature = "alloc")]
    pub unsafe fn task_ext_ptr(&self) -> *mut u8 {
        self.ext.task_ext.as_ptr()
    }

    /// Initialize the user-defined task extended data.
    ///
    /// Returns a reference to the task extended data if it has not been
    /// initialized yet (empty), otherwise returns [`None`].
    #[cfg(feature = "alloc")]
    pub fn init_task_ext<T: Sized>(&mut self, data: T) -> Option<&T> {
        if self.ext.task_ext.is_empty() {
            self.ext.task_ext.write(data).map(|data| &*data)
        } else {
            None
        }
    }

    /// Setup the TaskStack alloc fn.
    pub fn set_alloc_stack_fn(&mut self, alloc_fn: usize) {
        self.alloc_stack = Some(alloc_fn);
    }

    /// Setup the coroutine entry.
    pub fn set_coroutine_schedule(&mut self, coroutine_schedule: usize) {
        self.coroutine_schedule = Some(coroutine_schedule);
    }

    /// Returns a mutable reference to the task context.
    #[inline]
    pub const fn ctx_mut(&mut self) -> &mut TaskContext {
        self.ctx.get_mut()
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
    pub const unsafe fn kernel_stack(&self) -> *mut Option<TaskStack> {
        self.kstack.get()
    }

    /// Once the `kstack` field is None, the task is a coroutine.
    /// The `kstack` and the `ctx` will be set up,
    /// so the next coroutine will start at `coroutine_schedule` function.
    ///
    /// This function is only used before switching task.
    #[inline]
    pub fn set_kstack(&self) {
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

    /// Gets the cpu affinity mask of the task.
    ///
    /// Returns the cpu affinity mask of the task in type [`AxCpuMask`].
    #[cfg(feature = "alloc")]
    #[inline]
    pub fn cpumask(&self) -> AxCpuMask {
        self.ext.cpumask.load()
    }

    /// Sets the cpu affinity mask of the task.
    ///
    /// # Arguments
    /// `cpumask` - The cpu affinity mask to be set in type [`AxCpuMask`].
    #[cfg(feature = "alloc")]
    #[inline]
    pub fn set_cpumask(&self, cpumask: AxCpuMask) {
        self.ext.cpumask.store(cpumask);
    }

    #[cfg(feature = "alloc")]
    #[inline]
    pub fn select_run_queue_index(&self) -> usize {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static RUN_QUEUE_INDEX: AtomicUsize = AtomicUsize::new(0);

        let cpumask = self.cpumask();
        assert!(!cpumask.is_empty(), "No available CPU for task execution");

        // Round-robin selection of the run queue index.
        loop {
            let index = RUN_QUEUE_INDEX.fetch_add(1, Ordering::SeqCst) % config::SMP;
            if cpumask.get(index) {
                return index;
            }
        }
    }
}

// private methods
impl TaskInner {
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

    #[inline]
    pub fn in_wait_queue(&self) -> bool {
        self.in_wait_queue.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_in_wait_queue(&self, in_wait_queue: bool) {
        self.in_wait_queue.store(in_wait_queue, Ordering::Release);
    }

    /// Returns task's current timer ticket ID.
    #[inline]
    // #[cfg(feature = "irq")]
    pub fn timer_ticket(&self) -> u64 {
        self.timer_ticket_id.load(Ordering::Acquire)
    }

    /// Set the timer ticket ID.
    #[inline]
    // #[cfg(feature = "irq")]
    pub fn set_timer_ticket(&self, timer_ticket_id: u64) {
        // CAN NOT set timer_ticket_id to 0,
        // because 0 is used to indicate the timer event is expired.
        assert!(timer_ticket_id != 0);
        self.timer_ticket_id
            .store(timer_ticket_id, Ordering::Release);
    }

    /// Expire timer ticket ID by setting it to 0,
    /// it can be used to identify one timer event is triggered or expired.
    #[cfg(feature = "alloc")]
    #[inline]
    // #[cfg(feature = "irq")]
    pub fn timer_ticket_expired(&self) {
        self.timer_ticket_id.store(0, Ordering::Release);
    }

    #[inline]
    // #[cfg(feature = "preempt")]
    pub fn set_preempt_pending(&self, pending: bool) {
        self.need_resched.store(pending, Ordering::Release)
    }

    #[inline]
    // #[cfg(feature = "preempt")]
    pub fn need_resched(&self) -> bool {
        self.need_resched.load(Ordering::Acquire)
    }

    #[inline]
    // #[cfg(feature = "preempt")]
    pub fn can_preempt(&self, current_disable_count: usize) -> bool {
        self.preempt_disable_count.load(Ordering::Acquire) == current_disable_count
    }

    #[inline]
    // #[cfg(feature = "preempt")]
    pub fn disable_preempt(&self) {
        self.preempt_disable_count.fetch_add(1, Ordering::Release);
    }

    #[inline]
    // #[cfg(feature = "preempt")]
    pub fn enable_preempt(&self, resched: bool) {
        unsafe extern "C" {
            fn current_check_preempt_pending();
        }
        if self.preempt_disable_count.fetch_sub(1, Ordering::Release) == 1 && resched {
            // If current task is pending to be preempted, do rescheduling.
            unsafe {
                current_check_preempt_pending();
            }
        }
    }

    #[cfg(feature = "alloc")]
    #[inline]
    pub fn exit_code(&self) -> i32 {
        self.ext.exit_code.load(Ordering::Acquire)
    }

    #[cfg(feature = "alloc")]
    #[inline]
    pub fn set_exit_code(&self, exit_code: i32) {
        self.ext.exit_code.store(exit_code, Ordering::Release);
    }

    #[cfg(feature = "alloc")]
    #[inline]
    pub fn wait_queue(&self) -> &WaitQueue {
        &self.ext.wait_for_exit
    }

    #[inline]
    pub const unsafe fn ctx_mut_ptr(&self) -> *mut TaskContext {
        self.ctx.get()
    }

    /// Returns whether the task is running on a CPU.
    ///
    /// It is used to protect the task from being moved to a different run queue
    /// while it has not finished its scheduling process.
    /// The `on_cpu field is set to `true` when the task is preparing to run on a CPU,
    /// and it is set to `false` when the task has finished its scheduling process in `clear_prev_task_on_cpu()`.
    // #[cfg(feature = "smp")]
    #[inline]
    pub fn on_cpu(&self) -> bool {
        self.on_cpu.load(Ordering::Acquire)
    }

    /// Sets whether the task is running on a CPU.
    // #[cfg(feature = "smp")]
    #[inline]
    pub fn set_on_cpu(&self, on_cpu: bool) {
        self.on_cpu.store(on_cpu, Ordering::Release)
    }
}

#[cfg(feature = "alloc")]
impl fmt::Debug for TaskInner {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TaskInner")
            .field("id", &self.id)
            .field("name", &self.ext.name)
            .field("state", &self.state())
            .finish()
    }
}

#[cfg(feature = "alloc")]
impl Drop for TaskInner {
    fn drop(&mut self) {
        debug!("task drop: {}", self.id_name());
    }
}

#[derive(Debug)]
pub struct TaskStack {
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

#[cfg(feature = "alloc")]
impl Drop for TaskStack {
    fn drop(&mut self) {
        error!("{:?}", self);
        unsafe { alloc::alloc::dealloc(self.ptr.as_ptr(), self.layout) }
    }
}
