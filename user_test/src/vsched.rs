use base_task::{BaseTaskRef, Scheduler};
use core::cell::UnsafeCell;
use libloading::{Library, Symbol};
use memmap2::MmapMut;

pub fn map_vsched() {
    let _data_base = map_vsched_data().unwrap();
    map_vsched_text().unwrap();
}

fn map_vsched_data() -> Result<usize, usize> {
    let data_map = MmapMut::map_anon(VSCHED_DATA_SIZE).unwrap();
    println!("data base: {:p}", data_map.as_ptr());
    let data_base = data_map.as_ptr() as _;
    core::mem::forget(data_map);
    Ok(data_base)
}

fn map_vsched_text() -> Result<usize, usize> {
    let vsched_path = "./libvsched.so";

    let vsched_file = unsafe { Library::new(vsched_path).unwrap() };
    let symbol: Symbol<unsafe extern "C" fn(usize)> =
        unsafe { vsched_file.get(b"yield_now").unwrap() };
    unsafe { symbol(0) };
    Ok(0)
}

const VSCHED_DATA_SIZE: usize =
    (config::SMP * core::mem::size_of::<PerCPU>() + config::PAGES_SIZE_4K - 1)
        & (!(config::PAGES_SIZE_4K - 1));

#[allow(unused)]
struct PerCPU {
    /// The ID of the CPU this run queue is associated with.
    cpu_id: usize,
    /// The core scheduler of this run queue.
    /// Since irq and preempt are preserved by the kernel guard hold by `AxRunQueueRef`,
    /// we just use a simple raw spin lock here.
    scheduler: Scheduler,

    current_task: UnsafeCell<BaseTaskRef>,

    idle_task: BaseTaskRef,
    /// Stores the weak reference to the previous task that is running on this CPU.
    prev_task: UnsafeCell<BaseTaskRef>,
}
