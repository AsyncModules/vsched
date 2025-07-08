use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    core::mem::forget(vsched_map);
    let idle_task = Task::new_init("idle".into()).into_ref();
    idle_task.as_ref().set_state(base_task::TaskState::Running);
    init_vsched(0, idle_task);
    vsched_apis::spawn(
        Task::new(
            || {
                println!("into spawned task inner");
            },
            "spawn_test".into(),
            config::TASK_STACK_SIZE,
        )
        .into_ref(),
    );
    vsched_apis::yield_now(0);
    println!("back to idle task");
}
