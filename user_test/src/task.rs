use std::sync::Arc;

use task_inner::{AxTaskRef, TaskInner};

fn test() {
    let task = Arc::into_raw(Arc::new(TaskInner::new(
        Box::into_raw(Box::new(task_entry)),
        "sdadf",
        config::TASK_STACK_SIZE,
    )));
}

fn task_entry() {
    println!("sdafa");
}
