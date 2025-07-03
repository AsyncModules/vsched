#![no_std]

pub const RQ_CAP: usize = 256;
pub const DATA_SEC_MASK: usize = 0xFFFF_FFFF_C000_0000;
pub const SMP: usize = 1;
pub const TASK_STACK_SIZE: usize = 0x4000;
pub type AxCpuMask = cpumask::CpuMask<SMP>;
