//! `SharedScheduler` via vDSO.
#![no_std]
#![feature(unsafe_cell_access)]

mod api;
mod percpu;
mod sched;
mod task;
pub use api::*;
use sched::*;

/// Safety:
///     the offset of this function in the `.text`
///     section must be little than 0x1000.
///     The `#[inline(never)]` attribute and the
///     offset requirement can make it work ok.
#[inline(never)]
pub(crate) fn get_data_base() -> usize {
    let pc = unsafe { hal::asm::get_pc() };
    const VSCHED_DATA_SIZE: usize =
        (config::SMP * core::mem::size_of::<crate::percpu::PerCPU>() + config::PAGES_SIZE_4K - 1)
            & (!(config::PAGES_SIZE_4K - 1));
    (pc & config::DATA_SEC_MASK) - VSCHED_DATA_SIZE
}

#[cfg(all(target_os = "linux", not(test)))]
mod lang_item {
    #[panic_handler]
    fn panic(_info: &core::panic::PanicInfo) -> ! {
        loop {}
    }
}
