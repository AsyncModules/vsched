use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    let idle_task = Task::new_init("idle".into()).into_ref();
    init_vsched(0, idle_task);
    println!("{:?}", vsched_map.percpu(0).idle_task);
}
