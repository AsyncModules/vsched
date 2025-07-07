use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    let idle_task = Task::new(
        || loop {
            println!("idle task");
        },
        "idle".into(),
        config::TASK_STACK_SIZE,
    )
    .into_ref();
    init_vsched(0, idle_task);
    println!("{:?}", vsched_map.percpu(0).idle_task);
}
