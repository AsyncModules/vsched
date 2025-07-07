#![no_std]

pub const RQ_CAP: usize = 256;
pub const DATA_SEC_MASK: usize = 0xFFFF_FFFF_FFFF_0000;
pub const SMP: usize = 1;
pub const TASK_STACK_SIZE: usize = 0x4000;
pub const PAGES_SIZE_4K: usize = 0x1000;
pub type AxCpuMask = cpumask::CpuMask<SMP>;
