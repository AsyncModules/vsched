//! `SharedScheduler` via vDSO.
#![no_std]
#![feature(unsafe_cell_access)]

mod api;
mod sched;
pub use api::*;

#[cfg(all(target_os = "linux", not(test)))]
mod lang_item {
    #[panic_handler]
    fn panic(_info: &core::panic::PanicInfo) -> ! {
        loop {}
    }
}
