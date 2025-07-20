use std::{
    collections::VecDeque,
    os::unix::thread,
    sync::{Arc, Mutex},
    thread::spawn,
};

use criterion::{BenchmarkId, Criterion, SamplingMode, criterion_group, criterion_main};
use utils::LockFreeDeque;

fn test() {
    let deque: LockFreeDeque<usize, 100> = LockFreeDeque::new();
    for i in 0..99 {
        deque.push_back(i);
    }
    for i in 0..99 {
        assert_eq!(deque.pop_front(), Some(i));
    }
}

fn lock_free_se(data_num: usize, thread_num: usize) {
    // assert!(thread_num % 4 == 0);
    // assert!(data_num * thread_num / 2 <= 10000);
    let deque: Arc<LockFreeDeque<usize, 10001>> = Arc::new(LockFreeDeque::new());
    let mut handles = vec![];
    for _ in 0..(thread_num / 2) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            for _ in 0..data_num {
                deque_clone.push_back(1);
            }
        }));
    }
    for _ in 0..(thread_num / 2) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            let mut i = 0;
            while i < data_num {
                if deque_clone.pop_front().is_some() {
                    i += 1;
                }
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
}

fn lock_free_de(data_num: usize, thread_num: usize) {
    // assert!(thread_num % 4 == 0);
    // assert!(data_num * thread_num / 2 <= 10000);
    let deque: Arc<LockFreeDeque<usize, 10001>> = Arc::new(LockFreeDeque::new());
    let mut handles = vec![];
    for _ in 0..(thread_num / 4) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            for _ in 0..data_num {
                deque_clone.push_front(1);
            }
        }));
    }
    for _ in 0..(thread_num / 4) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            for _ in 0..data_num {
                deque_clone.push_back(1);
            }
        }));
    }
    for _ in 0..(thread_num / 4) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            let mut i = 0;
            while i < data_num {
                if deque_clone.pop_front().is_some() {
                    i += 1;
                }
            }
        }));
    }
    for _ in 0..(thread_num / 4) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            let mut i = 0;
            while i < data_num {
                if deque_clone.pop_back().is_some() {
                    i += 1;
                }
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
}

fn mutex_se(data_num: usize, thread_num: usize) {
    // assert!(thread_num % 4 == 0);
    // assert!(data_num * thread_num / 2 <= 10000);
    let deque: Arc<Mutex<VecDeque<usize>>> = Arc::new(Mutex::new(VecDeque::new()));
    let mut handles = vec![];
    for _ in 0..(thread_num / 2) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            for _ in 0..data_num {
                deque_clone.lock().unwrap().push_back(1);
            }
        }));
    }
    for _ in 0..(thread_num / 2) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            let mut i = 0;
            while i < data_num {
                if deque_clone.lock().unwrap().pop_front().is_some() {
                    i += 1;
                }
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
}

fn mutex_de(data_num: usize, thread_num: usize) {
    // assert!(thread_num % 4 == 0);
    // assert!(data_num * thread_num / 2 <= 10000);
    let deque: Arc<Mutex<VecDeque<usize>>> = Arc::new(Mutex::new(VecDeque::new()));
    let mut handles = vec![];
    for _ in 0..(thread_num / 4) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            for _ in 0..data_num {
                deque_clone.lock().unwrap().push_front(1);
            }
        }));
    }
    for _ in 0..(thread_num / 4) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            for _ in 0..data_num {
                deque_clone.lock().unwrap().push_back(1);
            }
        }));
    }
    for _ in 0..(thread_num / 4) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            let mut i = 0;
            while i < data_num {
                if deque_clone.lock().unwrap().pop_front().is_some() {
                    i += 1;
                }
            }
        }));
    }
    for _ in 0..(thread_num / 4) {
        let deque_clone = deque.clone();
        handles.push(spawn(move || {
            let mut i = 0;
            while i < data_num {
                if deque_clone.lock().unwrap().pop_back().is_some() {
                    i += 1;
                }
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
}

pub fn thread_num_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("thread_num_bench_deque");
    group.sampling_mode(SamplingMode::Flat);
    let data_num = 100;
    for thread_num in [4, 8, 16, 32, 64].iter() {
        // group.bench_with_input(
        //     BenchmarkId::new("LockFreeDeque-DoubleEnded", thread_num),
        //     thread_num,
        //     |b, i| b.iter(|| lock_free_de(data_num, *i)),
        // );
        // group.bench_with_input(
        //     BenchmarkId::new("Mutex+VecDeque-DoubleEnded", thread_num),
        //     thread_num,
        //     |b, i| b.iter(|| mutex_de(data_num, *i)),
        // );
        group.bench_with_input(
            BenchmarkId::new("LockFreeDeque-SingleEnded", thread_num),
            thread_num,
            |b, i| b.iter(|| lock_free_se(data_num, *i)),
        );
        group.bench_with_input(
            BenchmarkId::new("Mutex+VecDeque-SingleEnded", thread_num),
            thread_num,
            |b, i| b.iter(|| mutex_se(data_num, *i)),
        );
    }
}

pub fn data_num_benchmark_32(c: &mut Criterion) {
    let mut group = c.benchmark_group("data_num_bench_deque_32");
    group.sampling_mode(SamplingMode::Flat);
    let thread_num = 32;
    for data_num in [50, 100, 200, 300, 400, 500].iter() {
        // group.bench_with_input(
        //     BenchmarkId::new("LockFreeDeque-DoubleEnded", thread_num),
        //     data_num,
        //     |b, i| b.iter(|| lock_free_de(*i, thread_num)),
        // );
        // group.bench_with_input(
        //     BenchmarkId::new("Mutex+VecDeque-DoubleEnded", thread_num),
        //     data_num,
        //     |b, i| b.iter(|| mutex_de(*i, thread_num)),
        // );
        group.bench_with_input(
            BenchmarkId::new("LockFreeDeque-SingleEnded", data_num),
            data_num,
            |b, i| b.iter(|| lock_free_se(*i, thread_num)),
        );
        group.bench_with_input(
            BenchmarkId::new("Mutex+VecDeque-SingleEnded", data_num),
            data_num,
            |b, i| b.iter(|| mutex_se(*i, thread_num)),
        );
    }
}

pub fn data_num_benchmark_16(c: &mut Criterion) {
    let mut group = c.benchmark_group("data_num_bench_deque_16");
    group.sampling_mode(SamplingMode::Flat);
    let thread_num = 16;
    for data_num in [50, 100, 200, 300, 400, 500].iter() {
        // group.bench_with_input(
        //     BenchmarkId::new("LockFreeDeque-DoubleEnded", thread_num),
        //     data_num,
        //     |b, i| b.iter(|| lock_free_de(*i, thread_num)),
        // );
        // group.bench_with_input(
        //     BenchmarkId::new("Mutex+VecDeque-DoubleEnded", thread_num),
        //     data_num,
        //     |b, i| b.iter(|| mutex_de(*i, thread_num)),
        // );
        group.bench_with_input(
            BenchmarkId::new("LockFreeDeque-SingleEnded", data_num),
            data_num,
            |b, i| b.iter(|| lock_free_se(*i, thread_num)),
        );
        group.bench_with_input(
            BenchmarkId::new("Mutex+VecDeque-SingleEnded", data_num),
            data_num,
            |b, i| b.iter(|| mutex_se(*i, thread_num)),
        );
    }
}

criterion_group!(
    benches,
    thread_num_benchmark,
    data_num_benchmark_32,
    data_num_benchmark_16
);
criterion_main!(benches);
