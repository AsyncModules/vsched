use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    let idle_task = Task::new_init("idle".into());
    init_vsched(idle_task);
    println!("{:?}", vsched_map.percpu(0).idle_task);
}
