//! `SharedScheduler` via vDSO.
#![no_std]
#![feature(unsafe_cell_access)]

mod api;
mod percpu;
mod sched;
#[allow(unused)]
mod task;
pub use api::*;
use sched::*;

pub(crate) fn get_data_base() -> usize {
    let pc = unsafe { hal::asm::get_pc() };
    pc & config::DATA_SEC_MASK
}

mod lang_item {
    #[panic_handler]
    fn panic(_info: &core::panic::PanicInfo) -> ! {
        loop {}
    }
}
