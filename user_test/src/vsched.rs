use base_task::{BaseTaskRef, Scheduler, TaskExtRef};
use config::AxCpuMask;
use core::cell::UnsafeCell;
use core::str::from_utf8;
use memmap2::MmapMut;
use page_table_entry::MappingFlags;
use std::cell::RefCell;
use std::io::Read;
use std::sync::{Arc, Mutex};
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
        println!("Interpreter path: {:?}", _interp_path);
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
                let arc_task = unsafe { Arc::from_raw(task.as_ref()) };
                if Arc::strong_count(&arc_task) == 1 {
                    drop(arc_task);
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
    main_task
        .as_ref()
        .set_cpumask(AxCpuMask::one_shot(get_cpu_id()));
    vsched_apis::init_vsched(get_cpu_id(), main_task);
    let gc_task = Task::new(gc_entry, "gc".into(), config::TASK_STACK_SIZE);
    gc_task
        .as_ref()
        .set_cpumask(AxCpuMask::one_shot(get_cpu_id()));
    vsched_apis::spawn(gc_task);
    let idle_task = Task::new(|| run_idle(), "idle".into(), config::TASK_STACK_SIZE);
    idle_task
        .as_ref()
        .set_cpumask(AxCpuMask::one_shot(get_cpu_id()));
    vsched_apis::spawn(idle_task);
}

pub fn init_vsched_secondary() {
    CPU_ID.set(CPU_ID_ALLOCATOR.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let idle_task = Task::new_init("idle".into());
    idle_task
        .as_ref()
        .set_cpumask(AxCpuMask::one_shot(get_cpu_id()));
    vsched_apis::init_vsched_secondary(get_cpu_id(), idle_task);
}

pub fn blocked_resched(mut wq_guard: WaitQueueGuard) {
    let curr = vsched_apis::current(get_cpu_id());
    assert!(curr.as_ref().is_running());
    assert!(!curr.as_ref().is_idle());

    curr.as_ref().set_state(base_task::TaskState::Blocked);
    curr.as_ref().task_ext().set_in_wait_queue(true);
    wq_guard.push_back(curr.clone());
    drop(wq_guard);

    log::debug!("task blocked {:?}", curr.as_ref().task_ext().name());
    vsched_apis::resched(get_cpu_id());
}

static EXITED_TASKS: Mutex<VecDeque<BaseTaskRef>> = Mutex::new(VecDeque::new());
static WAIT_FOR_EXIT: WaitQueue = WaitQueue::new();

pub fn exit(exit_code: i32) -> ! {
    let curr = vsched_apis::current(get_cpu_id());
    assert!(curr.as_ref().is_running());
    assert!(!curr.as_ref().is_idle());
    log::debug!("{:?} is exited", curr.as_ref().task_ext().name());
    if curr.as_ref().is_init() {
        EXITED_TASKS.lock().unwrap().clear();
    } else {
        curr.as_ref().set_state(base_task::TaskState::Exited);
        curr.as_ref().task_ext().notify_exit(exit_code);
        EXITED_TASKS.lock().unwrap().push_back(curr);
        WAIT_FOR_EXIT.notify_one(false);
    }
    vsched_apis::resched(get_cpu_id());
    unreachable!()
}

const VSCHED_DATA_SIZE: usize =
    (config::SMP * core::mem::size_of::<PerCPU>() + config::PAGES_SIZE_4K - 1)
        & (!(config::PAGES_SIZE_4K - 1));

#[allow(unused)]
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
    pub prev_task: UnsafeCell<BaseTaskRef>,
}
