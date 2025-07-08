use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    core::mem::forget(vsched_map);
    let idle_task = Task::new_init("idle".into());
    idle_task.as_ref().set_state(base_task::TaskState::Running);
    init_vsched(idle_task);
    let task1 = Task::new(
        || {
            println!("into spawned task inner");
        },
        "spawn_test".into(),
        config::TASK_STACK_SIZE,
    );
    let task1_clone = task1.clone();
    let task2 = Task::new(
        move || {
            println!("wait task start");
            wait(task1_clone);
            println!("wait task ok");
        },
        "spawn_test".into(),
        config::TASK_STACK_SIZE,
    );
    vsched_apis::spawn(task2);
    vsched_apis::spawn(task1);

    vsched_apis::yield_now(0);
    println!("back to idle task");
}
