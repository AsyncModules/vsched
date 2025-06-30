#![no_std]
#![allow(unused)]

// cfg_if::cfg_if! {
//     if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
//         mod riscv;
//         pub use self::riscv::*;
//     }
// }

mod riscv;
pub use self::riscv::*;
