use std::sync::atomic::AtomicUsize;

use user_test::*;
fn main() {
    let vsched_map = map_vsched().unwrap();
    core::mem::forget(vsched_map);
    static BOOT_COUNT: AtomicUsize = AtomicUsize::new(1);
    let _thread_handle = std::thread::spawn(|| {
        init_vsched_secondary();
        BOOT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        run_idle();
    });

    init_vsched();
    while BOOT_COUNT.load(std::sync::atomic::Ordering::Relaxed) < config::SMP {
        core::hint::spin_loop();
    }
    let task_handle = vsched_apis::spawn(Task::new(
        || {
            println!("into spawned task inner main thread spawned");
        },
        "main spawn_test".into(),
        config::TASK_STACK_SIZE,
    ));
    wait(task_handle).unwrap();
    println!("main task wait ok");
}
