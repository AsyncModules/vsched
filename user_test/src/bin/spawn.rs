use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    core::mem::forget(vsched_map);
    let idle_task = Task::new_init("idle".into());
    init_vsched(0, idle_task);
    for _ in 0..config::RQ_CAP {
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
