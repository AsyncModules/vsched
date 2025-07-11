use base_task::{BaseTaskRef, Scheduler, TaskExtRef, TaskState, WeakBaseTaskRef};
use config::AxCpuMask;
use core::cell::UnsafeCell;
use core::pin::Pin;
use core::str::from_utf8;
use core::task::{Context, Poll};
use memmap2::MmapMut;
use page_table_entry::MappingFlags;
use std::cell::RefCell;
use std::io::Read;
use std::mem::MaybeUninit;
use std::sync::Mutex;
use std::thread_local;
use std::{collections::VecDeque, sync::atomic::AtomicUsize};
pub use vsched_apis::*;

use xmas_elf::program::SegmentData;

use crate::{Task, WaitQueue, WaitQueueGuard};

const VSCHED: &[u8] = core::include_bytes!("../../libvsched.so");

static CPU_ID_ALLOCATOR: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    pub static CPU_ID: RefCell<usize> = RefCell::new(0);
}

pub fn get_cpu_id() -> usize {
    CPU_ID.with(|cpu_id| *cpu_id.borrow())
}

pub struct Vsched {
    #[allow(unused)]
    map: MmapMut,
}

impl Vsched {
    pub fn percpu(&self, index: usize) -> &PerCPU {
        let base = self.map.as_ptr() as *const PerCPU;
        unsafe { &*base.add(index) }
    }
}

pub fn map_vsched() -> Result<Vsched, ()> {
    let mut vsched_map = MmapMut::map_anon(VSCHED_DATA_SIZE + 0x40000).unwrap();
    log::info!(
        "vsched_map base: [{:p}, {:p}]",
        vsched_map.as_ptr(),
        unsafe { vsched_map.as_ptr().add(VSCHED_DATA_SIZE + 0x40000) }
    );
    let vsched_so = &mut vsched_map[VSCHED_DATA_SIZE..];
    #[allow(const_item_mutation)]
    VSCHED.read(vsched_so).unwrap();

    let vsched_elf = xmas_elf::ElfFile::new(vsched_so).expect("Error parsing app ELF file.");
    if let Some(interp) = vsched_elf
        .program_iter()
        .find(|ph| ph.get_type() == Ok(xmas_elf::program::Type::Interp))
    {
        let interp = match interp.get_data(&vsched_elf) {
            Ok(SegmentData::Undefined(data)) => data,
            _ => panic!("Invalid data in Interp Elf Program Header"),
        };

        let interp_path = from_utf8(interp).expect("Interpreter path isn't valid UTF-8");
        // remove trailing '\0'
        let _interp_path = interp_path.trim_matches(char::from(0)).to_string();
        log::debug!("Interpreter path: {:?}", _interp_path);
    }
    let elf_base_addr = Some(vsched_so.as_ptr() as usize);
    // let relocate_pairs = elf_parser::get_relocate_pairs(&elf, elf_base_addr);
    let segments = elf_parser::get_elf_segments(&vsched_elf, elf_base_addr);
    let relocate_pairs = elf_parser::get_relocate_pairs(&vsched_elf, elf_base_addr);
    for segment in segments {
        log::debug!(
            "{:?}, {:#x}, {:?}",
            segment.vaddr,
            segment.size,
            segment.flags
        );
        let mut flag = libc::PROT_READ;
        if segment.flags.contains(MappingFlags::EXECUTE) {
            flag |= libc::PROT_EXEC;
        }
        if segment.flags.contains(MappingFlags::WRITE) {
            flag |= libc::PROT_WRITE;
        }
        unsafe {
            if libc::mprotect(segment.vaddr.as_usize() as _, segment.size, flag)
                == libc::MAP_FAILED as _
            {
                log::error!("mprotect res failed");
                return Err(());
            }
        };
    }

    for relocate_pair in relocate_pairs {
        let src: usize = relocate_pair.src.into();
        let dst: usize = relocate_pair.dst.into();
        let count = relocate_pair.count;
        log::info!(
            "Relocate: src: 0x{:x}, dst: 0x{:x}, count: {}",
            src,
            dst,
            count
        );
        unsafe { core::ptr::copy_nonoverlapping(src.to_ne_bytes().as_ptr(), dst as *mut u8, count) }
    }

    unsafe { vsched_apis::init_vsched_vtable(elf_base_addr.unwrap() as _, &vsched_elf) };

    Ok(Vsched { map: vsched_map })
}

fn gc_entry() {
    loop {
        let mut exited_tasks = EXITED_TASKS.lock().unwrap();
        let n = exited_tasks.len();
        for _ in 0..n {
            if let Some(task) = exited_tasks.pop_front() {
                if task.strong_count() == 1 {
                    drop(task);
                } else {
                    exited_tasks.push_back(task);
                }
            }
        }
        drop(exited_tasks);
        WAIT_FOR_EXIT.wait();
    }
}

pub fn run_idle() {
    loop {
        vsched_apis::yield_now(get_cpu_id());
    }
}

pub fn init_vsched() {
    CPU_ID.set(CPU_ID_ALLOCATOR.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let main_task = Task::new_init("main".into());
    main_task.set_cpumask(AxCpuMask::one_shot(get_cpu_id()));
    let idle_task = Task::new(|| run_idle(), "idle".into(), config::TASK_STACK_SIZE);
    idle_task.set_cpumask(AxCpuMask::one_shot(get_cpu_id()));
    vsched_apis::init_vsched(get_cpu_id(), idle_task, main_task);
    let gc_task = Task::new(gc_entry, "gc".into(), config::TASK_STACK_SIZE);
    gc_task.set_cpumask(AxCpuMask::one_shot(get_cpu_id()));
    vsched_apis::spawn(gc_task);
}

pub fn init_vsched_secondary() {
    CPU_ID.set(CPU_ID_ALLOCATOR.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let idle_task = Task::new_init("idle".into());
    idle_task.set_cpumask(AxCpuMask::one_shot(get_cpu_id()));
    vsched_apis::init_vsched(get_cpu_id(), idle_task.clone(), idle_task);
}

pub fn blocked_resched(mut wq_guard: WaitQueueGuard) {
    let curr = vsched_apis::current(get_cpu_id());
    assert!(curr.is_running());
    assert!(!curr.is_idle());

    curr.set_state(base_task::TaskState::Blocked);
    curr.task_ext().set_in_wait_queue(true);
    wq_guard.push_back(curr.clone());
    drop(wq_guard);

    log::debug!("task blocked {:?}", curr.task_ext().name());
    vsched_apis::resched(get_cpu_id());
}

static EXITED_TASKS: Mutex<VecDeque<BaseTaskRef>> = Mutex::new(VecDeque::new());
static WAIT_FOR_EXIT: WaitQueue = WaitQueue::new();

pub fn exit(exit_code: i32) -> ! {
    let curr = vsched_apis::current(get_cpu_id());
    assert!(curr.is_running());
    assert!(!curr.is_idle());
    log::debug!("{:?} is exited", curr.task_ext().name());
    if curr.is_init() {
        EXITED_TASKS.lock().unwrap().clear();
        unsafe { libc::exit(0) };
    } else {
        curr.set_state(base_task::TaskState::Exited);
        curr.task_ext().notify_exit(exit_code);
        EXITED_TASKS.lock().unwrap().push_back(curr.clone());
        WAIT_FOR_EXIT.notify_one(false);
    }

    vsched_apis::resched(get_cpu_id());
    unreachable!()
}

/// Current coroutine task gives up the CPU time voluntarily, and switches to another
/// ready task.
#[inline]
pub async fn yield_now_f() {
    YieldFuture::new().await;
}

/// The `YieldFuture` used when yielding the current task and reschedule.
/// When polling this future, the current task will be put into the run queue
/// with `Ready` state and reschedule to the next task on the run queue.
///
/// The polling operation is as the same as the
/// `current_run_queue::<NoPreemptIrqSave>().yield_current()` function.
///
/// SAFETY:
/// Due to this future is constructed with `current_run_queue::<NoPreemptIrqSave>()`,
/// the operation about manipulating the RunQueue and the switching to next task is
/// safe(The `IRQ` and `Preempt` are disabled).
pub(crate) struct YieldFuture {
    flag: bool,
}

impl YieldFuture {
    pub(crate) fn new() -> Self {
        Self { flag: false }
    }
}

impl Unpin for YieldFuture {}

impl Future for YieldFuture {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { flag } = self.get_mut();
        if !(*flag) {
            *flag = !*flag;
            let curr = vsched_apis::current(get_cpu_id());
            log::trace!("task yield: {}", curr.task_ext().id_name());
            assert!(curr.is_running());
            if vsched_apis::yield_f(get_cpu_id()) {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        } else {
            Poll::Ready(())
        }
    }
}

/// Due not manually release the `current_run_queue.state`,
/// otherwise it will cause double release.
impl Drop for YieldFuture {
    fn drop(&mut self) {}
}

/// Exits the current coroutine task.
pub async fn exit_f(exit_code: i32) {
    ExitFuture::new(exit_code).await;
}

/// The `ExitFuture` used when exiting the current task
/// with the specified exit code, which is always return `Poll::Pending`.
///
/// The polling operation is as the same as the
/// `current_run_queue::<NoPreemptIrqSave>().exit_current()` function.
///
/// SAFETY: as the same as the `YieldFuture`. However, It wrap the `CurrentRunQueueRef`
/// with `ManuallyDrop`, otherwise the `IRQ` and `Preempt` state of other
/// tasks(maybe `main` or `gc` task) which recycle the exited task(which used this future)
/// will be error due to automatically drop the `CurrentRunQueueRef.
/// The `CurrentRunQueueRef` should never be drop.
pub(crate) struct ExitFuture {
    exit_code: i32,
}

impl ExitFuture {
    pub(crate) fn new(exit_code: i32) -> Self {
        Self { exit_code }
    }
}

impl Unpin for ExitFuture {}

impl Future for ExitFuture {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { exit_code } = self.get_mut();
        let exit_code = *exit_code;
        let curr = vsched_apis::current(get_cpu_id());
        log::debug!(
            "task exit: {}, exit_code={}",
            curr.task_ext().id_name(),
            exit_code
        );
        assert!(curr.is_running(), "task is not running: {:?}", curr.state());
        assert!(!curr.is_idle());
        curr.set_state(TaskState::Exited);

        // Notify the joiner task.
        curr.task_ext().notify_exit(exit_code);

        // Push current task to the `EXITED_TASKS` list, which will be consumed by the GC task.
        EXITED_TASKS.lock().unwrap().push_back(curr.clone());
        // Wake up the GC task to drop the exited tasks.
        WAIT_FOR_EXIT.notify_one(false);
        assert!(vsched_apis::resched_f(get_cpu_id()));
        Poll::Pending
    }
}

/// The `BlockedReschedFuture` used when blocking the current task.
///
/// When polling this future, current task will be put into the wait queue and reschedule,
/// the state of current task will be marked as `Blocked`, set the `in_wait_queue` flag as true.
/// Note:
///     1. When polling this future, the wait queue is locked.
///     2. When polling this future, the current task is in the running state.
///     3. When polling this future, the current task is not the idle task.
///     4. The lock of the wait queue will be released explicitly after current task is pushed into it.
///
/// SAFETY:
/// as the same as the `YieldFuture`. Due to the `WaitQueueGuard` is not implemented
/// the `Send` trait, this future must hold the reference about the `WaitQueue` instead
/// of the `WaitQueueGuard`.
pub(crate) struct BlockedReschedFuture<'a> {
    wq: &'a WaitQueue,
    flag: bool,
}

impl<'a> BlockedReschedFuture<'a> {
    pub fn new(wq: &'a WaitQueue) -> Self {
        Self { wq, flag: false }
    }
}

impl<'a> Unpin for BlockedReschedFuture<'a> {}

impl<'a> Future for BlockedReschedFuture<'a> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { wq, flag } = self.get_mut();
        if !(*flag) {
            *flag = !*flag;
            let mut wq_guard = wq.queue.lock().unwrap();
            let curr = vsched_apis::current(get_cpu_id());
            assert!(curr.is_running());
            assert!(!curr.is_idle());
            // we must not block current task with preemption disabled.
            // Current expected preempt count is 2.
            // 1 for `NoPreemptIrqSave`, 1 for wait queue's `SpinNoIrq`.
            #[cfg(feature = "preempt")]
            assert!(curr.can_preempt(2));

            // Mark the task as blocked, this has to be done before adding it to the wait queue
            // while holding the lock of the wait queue.
            curr.set_state(TaskState::Blocked);
            curr.task_ext().set_in_wait_queue(true);

            wq_guard.push_back(curr.clone());
            // Drop the lock of wait queue explictly.
            drop(wq_guard);

            // Current task's state has been changed to `Blocked` and added to the wait queue.
            // Note that the state may have been set as `Ready` in `unblock_task()`,
            // see `unblock_task()` for details.

            log::debug!("task block: {}", curr.task_ext().id_name());
            assert!(vsched_apis::resched_f(get_cpu_id()));
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

impl<'a> Drop for BlockedReschedFuture<'a> {
    fn drop(&mut self) {}
}

const VSCHED_DATA_SIZE: usize = config::SMP
    * ((core::mem::size_of::<PerCPU>() + config::PAGES_SIZE_4K - 1)
        & (!(config::PAGES_SIZE_4K - 1)));

#[allow(unused)]
#[repr(C)]
pub struct PerCPU {
    /// The ID of the CPU this run queue is associated with.
    pub cpu_id: usize,
    /// The core scheduler of this run queue.
    /// Since irq and preempt are preserved by the kernel guard hold by `AxRunQueueRef`,
    /// we just use a simple raw spin lock here.
    pub scheduler: Scheduler,

    pub current_task: UnsafeCell<BaseTaskRef>,

    pub idle_task: BaseTaskRef,
    /// Stores the weak reference to the previous task that is running on this CPU.
    pub prev_task: UnsafeCell<MaybeUninit<WeakBaseTaskRef>>,
}
