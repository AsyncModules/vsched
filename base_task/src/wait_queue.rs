use crate::TaskRef;
use alloc::collections::VecDeque;
use kspin::SpinNoIrq;

pub struct WaitQueue {
    pub queue: SpinNoIrq<VecDeque<TaskRef>>,
}

impl WaitQueue {
    /// Creates an empty wait queue.
    pub const fn new() -> Self {
        Self {
            queue: SpinNoIrq::new(VecDeque::new()),
        }
    }
}
