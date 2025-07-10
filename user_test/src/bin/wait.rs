use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    core::mem::forget(vsched_map);
    init_vsched();
    let task1 = Task::new(
        || {
            println!("into spawned task inner");
        },
        "task__1".into(),
        config::TASK_STACK_SIZE,
    );
    let task1_clone = task1.clone();
    let task2 = Task::new(
        move || {
            println!("wait task start");
            task1_clone.task_ext().join();
            println!("wait task ok");
        },
        "task__2".into(),
        config::TASK_STACK_SIZE,
    );
    let t2 = vsched_apis::spawn(task2);
    let t1 = vsched_apis::spawn(task1);

    vsched_apis::yield_now(get_cpu_id());

    println!("back to idle task");
    t1.task_ext().join();
    t2.task_ext().join();
    exit(0)
}
