macro_rules! def_test_sched {
    ($name: ident, $scheduler: ty, $task: ty, $task_ref: ty) => {
        mod $name {
            use crate::*;
            use alloc::sync::Arc;

            #[test]
            fn test_sched() {
                const NUM_TASKS: usize = 11;

                let scheduler = <$scheduler>::new();
                for i in 0..NUM_TASKS {
                    scheduler.add_task(<$task_ref>::new(
                        core::ptr::NonNull::new(Arc::into_raw(Arc::new(<$task>::new(i))) as _)
                            .unwrap(),
                    ));
                }

                for i in 0..NUM_TASKS * 10 - 1 {
                    let next = scheduler.pick_next_task().unwrap();
                    assert_eq!(*next.as_ref().inner(), i % NUM_TASKS);
                    // pass a tick to ensure the order of tasks
                    scheduler.task_tick(&next);
                    scheduler.put_prev_task(next, false);
                }

                let mut n = 0;
                while scheduler.pick_next_task().is_some() {
                    n += 1;
                }
                assert_eq!(n, NUM_TASKS);
            }

            #[test]
            fn bench_yield() {
                const NUM_TASKS: usize = 208;
                const COUNT: usize = NUM_TASKS * 3;

                let scheduler = <$scheduler>::new();
                for i in 0..NUM_TASKS {
                    scheduler.add_task(<$task_ref>::new(
                        core::ptr::NonNull::new(Arc::into_raw(Arc::new(<$task>::new(i))) as _)
                            .unwrap(),
                    ));
                }

                let t0 = std::time::Instant::now();
                for _ in 0..COUNT {
                    let next = scheduler.pick_next_task().unwrap();
                    scheduler.put_prev_task(next, false);
                }
                let t1 = std::time::Instant::now();
                println!(
                    "  {}: task yield speed: {:?}/task",
                    stringify!($scheduler),
                    (t1 - t0) / (COUNT as u32)
                );
            }
        }
    };
}

def_test_sched!(
    fifo,
    FifoScheduler::<usize, 256>,
    FifoTask::<usize>,
    FiFoTaskRef::<usize>
);
def_test_sched!(
    rr,
    RRScheduler::<usize, 5, 256>,
    RRTask::<usize, 5>,
    RRTaskRef::<usize, 5>
);
def_test_sched!(
    cfs,
    CFScheduler::<usize, 256>,
    CFSTask::<usize>,
    CFSTaskRef<usize>
);
