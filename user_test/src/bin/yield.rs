use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    core::mem::forget(vsched_map);
    init_vsched();
    vsched_apis::spawn(Task::new(
        || {
            println!("into spawned task inner");
        },
        "spawn_test".into(),
        config::TASK_STACK_SIZE,
    ));
    vsched_apis::yield_now(get_cpu_id());
    println!("back to idle task");
    exit(0)
}
