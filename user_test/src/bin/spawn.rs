use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    core::mem::forget(vsched_map);
    init_vsched();
    // Due to the init_vsched will spawn the `gc` and `idle` task to the scheduler,
    // the number must be not greater than `RQ_CAP - 2`.
    for _ in 0..(config::RQ_CAP - 2) {
        vsched_apis::spawn(Task::new(
            || {
                println!("into spawned task inner");
            },
            "spawn_test".into(),
            config::TASK_STACK_SIZE,
        ));
    }
    println!("spawn test ok");
}
