use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    init_vsched();
    println!("{:?}", vsched_map.percpu(get_cpu_id()).idle_task);
    exit(0)
}
