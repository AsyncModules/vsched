#![no_std]

use core::cell::UnsafeCell;
use core::cmp::Ordering as CmpOrdering;
use core::mem::MaybeUninit;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 静态分配的无锁 BTreeMap，支持多生产者多消费者
/// 完全无堆分配，适用于no_std环境
pub struct LockFreeBTreeMap<K, V, const CAPACITY: usize>
where
    K: Clone + Ord + Send + Sync,
    V: Clone,
{
    // 存储键值对的静态数组
    storage: [UnsafeCell<MaybeUninit<Entry<K, V>>>; CAPACITY],
    // 每个槽位的状态标记，包含版本号防止ABA问题
    slot_states: [AtomicSlotState; CAPACITY],
    // 当前有效元素数量
    len: AtomicUsize,
    // 全局版本号，用于插入排序和ABA防护
    global_version: AtomicUsize,
}

/// 键值对条目
#[derive(Clone)]
struct Entry<K, V> {
    key: K,
    value: V,
    // 插入时的版本号，用于辅助排序和去重
    insert_version: usize,
    // 槽位版本号，用于ABA防护
    slot_version: usize,
}

/// 原子槽位状态，包含状态和版本号
struct AtomicSlotState {
    // 高32位存储版本号，低32位存储状态
    // 这样可以原子性地更新状态和版本号
    state_and_version: AtomicUsize,
}

impl Clone for AtomicSlotState {
    fn clone(&self) -> Self {
        Self {
            state_and_version: AtomicUsize::new(self.state_and_version.load(Ordering::Relaxed)),
        }
    }
}

// 状态编码（低32位）
const STATE_EMPTY: u32 = 0;
const STATE_WRITING: u32 = 1;
const STATE_VALID: u32 = 2;
const STATE_DELETING: u32 = 3;

// 版本号偏移量
const VERSION_SHIFT: usize = 32;

impl AtomicSlotState {
    const fn new() -> Self {
        Self {
            state_and_version: AtomicUsize::new(0), // version=0, state=EMPTY
        }
    }

    fn load_state_version(&self) -> (u32, u32) {
        let combined = self.state_and_version.load(Ordering::Acquire);
        let state = (combined as u32) & 0xFFFFFFFF;
        let version = (combined >> VERSION_SHIFT) as u32;
        (state, version)
    }

    fn is_empty(&self) -> bool {
        let (state, _) = self.load_state_version();
        state == STATE_EMPTY
    }

    fn is_valid(&self) -> bool {
        let (state, _) = self.load_state_version();
        state == STATE_VALID
    }

    fn try_lock_for_write(&self, new_version: u32) -> bool {
        let expected = 0; // version=0, state=EMPTY
        let desired = ((new_version as usize) << VERSION_SHIFT) | (STATE_WRITING as usize);

        self.state_and_version
            .compare_exchange(expected, desired, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    fn try_lock_for_delete(&self) -> Option<u32> {
        let current = self.state_and_version.load(Ordering::Acquire);
        let state = (current as u32) & 0xFFFFFFFF;
        let version = (current >> VERSION_SHIFT) as u32;

        if state != STATE_VALID {
            return None;
        }

        let desired = ((version as usize) << VERSION_SHIFT) | (STATE_DELETING as usize);

        match self.state_and_version.compare_exchange(
            current,
            desired,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => Some(version),
            Err(_) => None,
        }
    }

    fn mark_valid(&self, version: u32) {
        let desired = ((version as usize) << VERSION_SHIFT) | (STATE_VALID as usize);
        self.state_and_version.store(desired, Ordering::Release);
    }

    fn mark_empty(&self) {
        self.state_and_version.store(0, Ordering::Release); // version=0, state=EMPTY
    }

    fn get_version_if_valid(&self) -> Option<u32> {
        let (state, version) = self.load_state_version();
        if state == STATE_VALID {
            Some(version)
        } else {
            None
        }
    }
}

/// 固定大小的栈分配候选项数组
/// 用于在first_key_value和pop_first中收集候选项
struct StackCandidates<K, V, const CAPACITY: usize> {
    data: [MaybeUninit<Candidate<K, V>>; CAPACITY],
    len: usize,
}

#[derive(Clone)]
struct Candidate<K, V> {
    key: K,
    value: V,
    insert_version: usize,
    index: usize,
    slot_version: u32,
}

impl<K, V, const CAPACITY: usize> StackCandidates<K, V, CAPACITY> {
    fn new() -> Self {
        Self {
            data: [const { MaybeUninit::uninit() }; CAPACITY],
            len: 0,
        }
    }

    fn push(&mut self, candidate: Candidate<K, V>) -> bool {
        if self.len < CAPACITY {
            self.data[self.len] = MaybeUninit::new(candidate);
            self.len += 1;
            true
        } else {
            false
        }
    }

    fn find_min_by<F>(&self, mut compare: F) -> Option<&Candidate<K, V>>
    where
        F: FnMut(&Candidate<K, V>, &Candidate<K, V>) -> CmpOrdering,
    {
        if self.len == 0 {
            return None;
        }

        let mut min_index = 0;
        unsafe {
            let mut min_candidate = self.data[0].assume_init_ref();

            for i in 1..self.len {
                let candidate = self.data[i].assume_init_ref();
                if compare(candidate, min_candidate) == CmpOrdering::Less {
                    min_candidate = candidate;
                    min_index = i;
                }
            }

            Some(self.data[min_index].assume_init_ref())
        }
    }

    fn clear(&mut self) {
        // 清理已初始化的候选项
        for i in 0..self.len {
            unsafe {
                self.data[i].assume_init_drop();
            }
        }
        self.len = 0;
    }
}

impl<K, V, const CAPACITY: usize> Drop for StackCandidates<K, V, CAPACITY> {
    fn drop(&mut self) {
        self.clear();
    }
}

impl<K, V, const CAPACITY: usize> LockFreeBTreeMap<K, V, CAPACITY>
where
    K: Clone + Ord + Send + Sync,
    V: Clone,
{
    const INIT_CELL: UnsafeCell<MaybeUninit<Entry<K, V>>> = UnsafeCell::new(MaybeUninit::uninit());
    const INIT_STATE: AtomicSlotState = AtomicSlotState::new();

    /// 创建新的 BTreeMap
    pub const fn new() -> Self {
        unsafe {
            Self {
                storage: [Self::INIT_CELL; CAPACITY],
                slot_states: [Self::INIT_STATE; CAPACITY],
                len: AtomicUsize::new(0),
                global_version: AtomicUsize::new(1), // 从1开始，0表示空槽位
            }
        }
    }

    /// 插入键值对
    /// 修复了竞争条件和ABA问题，无堆分配
    pub fn insert(&self, key: K, value: V) -> Option<Option<V>> {
        let insert_version = self.global_version.fetch_add(1, Ordering::Relaxed);
        let slot_version = (insert_version & 0xFFFFFFFF) as u32;

        // 最多重试CAPACITY次，避免无限循环
        for _retry in 0..CAPACITY {
            // 阶段1：尝试更新现有键
            for i in 0..CAPACITY {
                if let Some(version) = self.slot_states[i].get_version_if_valid() {
                    unsafe {
                        let entry_ptr = (*self.storage[i].get()).as_ptr();
                        if !entry_ptr.is_null() {
                            let entry = &*entry_ptr;
                            if entry.key == key && entry.slot_version == version as usize {
                                // 尝试锁定槽位进行更新
                                if let Some(locked_version) =
                                    self.slot_states[i].try_lock_for_delete()
                                {
                                    // 双重检查：确保版本号匹配，避免ABA问题
                                    if locked_version == version {
                                        let old_value = entry.value.clone();

                                        // 写入新条目
                                        let new_entry = Entry {
                                            key: key.clone(),
                                            value,
                                            insert_version,
                                            slot_version: slot_version as usize,
                                        };

                                        // 先写入数据，再更新状态
                                        ptr::write(
                                            (*self.storage[i].get()).as_mut_ptr(),
                                            new_entry,
                                        );
                                        self.slot_states[i].mark_valid(slot_version);

                                        return Some(Some(old_value));
                                    } else {
                                        // 版本不匹配，恢复状态并重试
                                        self.slot_states[i].mark_valid(locked_version);
                                        break; // 跳到下一轮重试
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // 阶段2：寻找空槽位插入新键
            for i in 0..CAPACITY {
                if self.slot_states[i].try_lock_for_write(slot_version) {
                    let entry = Entry {
                        key,
                        value,
                        insert_version,
                        slot_version: slot_version as usize,
                    };

                    unsafe {
                        ptr::write((*self.storage[i].get()).as_mut_ptr(), entry);
                    }

                    // 先写入数据，再更新状态
                    self.slot_states[i].mark_valid(slot_version);
                    self.len.fetch_add(1, Ordering::Relaxed);
                    return Some(None);
                }
            }
        }

        // 重试次数用完或容量已满
        None
    }

    /// 获取最小键值对（不删除）
    /// 使用栈分配的候选数组，无堆分配
    pub fn first_key_value(&self) -> Option<(K, V)> {
        let mut candidates = StackCandidates::<K, V, CAPACITY>::new();

        // 收集所有有效条目的快照
        for i in 0..CAPACITY {
            if let Some(version) = self.slot_states[i].get_version_if_valid() {
                unsafe {
                    let entry_ptr = (*self.storage[i].get()).as_ptr();
                    if !entry_ptr.is_null() {
                        let entry = &*entry_ptr;

                        // 双重检查版本号
                        if entry.slot_version == version as usize {
                            // 再次验证状态仍然有效
                            if self.slot_states[i].get_version_if_valid() == Some(version) {
                                let candidate = Candidate {
                                    key: entry.key.clone(),
                                    value: entry.value.clone(),
                                    insert_version: entry.insert_version,
                                    index: i,
                                    slot_version: version,
                                };

                                if !candidates.push(candidate) {
                                    // 候选数组已满，不应该发生
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        // 在收集的候选中找最小值
        candidates
            .find_min_by(|a, b| match a.key.cmp(&b.key) {
                CmpOrdering::Equal => a.insert_version.cmp(&b.insert_version),
                other => other,
            })
            .map(|candidate| (candidate.key.clone(), candidate.value.clone()))
    }

    /// 弹出最小键值对（删除并返回）
    /// 使用栈分配，修复了多消费者竞争条件
    pub fn pop_first(&self) -> Option<(K, V)> {
        const MAX_RETRIES: usize = 16; // 限制重试次数避免活锁

        for _retry in 0..MAX_RETRIES {
            let mut candidates = StackCandidates::<K, V, CAPACITY>::new();

            // 阶段1：收集所有有效候选项
            for i in 0..CAPACITY {
                if let Some(version) = self.slot_states[i].get_version_if_valid() {
                    unsafe {
                        let entry_ptr = (*self.storage[i].get()).as_ptr();
                        if !entry_ptr.is_null() {
                            let entry = &*entry_ptr;

                            if entry.slot_version == version as usize {
                                // 再次验证状态
                                if self.slot_states[i].get_version_if_valid() == Some(version) {
                                    let candidate = Candidate {
                                        key: entry.key.clone(),
                                        value: entry.value.clone(),
                                        insert_version: entry.insert_version,
                                        index: i,
                                        slot_version: version,
                                    };

                                    if !candidates.push(candidate) {
                                        // 候选数组已满
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // 阶段2：找到最小候选项并尝试删除
            if let Some(min_candidate) = candidates.find_min_by(|a, b| match a.key.cmp(&b.key) {
                CmpOrdering::Equal => a.insert_version.cmp(&b.insert_version),
                other => other,
            }) {
                let index = min_candidate.index;
                let expected_version = min_candidate.slot_version;
                let expected_key = min_candidate.key.clone();
                let expected_insert_version = min_candidate.insert_version;

                if let Some(locked_version) = self.slot_states[index].try_lock_for_delete() {
                    // 三重检查：版本号、键、插入版本都要匹配
                    if locked_version == expected_version {
                        unsafe {
                            let entry_ptr = (*self.storage[index].get()).as_ptr();
                            if !entry_ptr.is_null() {
                                let entry = &*entry_ptr;
                                if entry.key == expected_key
                                    && entry.insert_version == expected_insert_version
                                    && entry.slot_version == locked_version as usize
                                {
                                    let result = (entry.key.clone(), entry.value.clone());

                                    // 清理槽位
                                    ptr::drop_in_place((*self.storage[index].get()).as_mut_ptr());
                                    self.slot_states[index].mark_empty();
                                    self.len.fetch_sub(1, Ordering::Relaxed);

                                    return Some(result);
                                }
                            }
                        }
                    }
                    // 如果验证失败，恢复状态
                    self.slot_states[index].mark_valid(locked_version);
                }
                // 继续重试
            } else {
                // 没有找到任何有效元素
                return None;
            }
        }

        // 达到最大重试次数，可能是高并发冲突
        None
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.len.load(Ordering::Acquire) == 0
    }

    /// 获取当前元素数量（近似值，因为并发环境下可能不准确）
    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    /// 获取精确的元素数量（通过扫描所有槽位）
    /// 无堆分配版本
    pub fn exact_len(&self) -> usize {
        let mut count = 0;
        for i in 0..CAPACITY {
            if self.slot_states[i].is_valid() {
                count += 1;
            }
        }
        count
    }

    /// 清空所有元素
    pub fn clear(&self) {
        for i in 0..CAPACITY {
            if let Some(version) = self.slot_states[i].get_version_if_valid() {
                if let Some(locked_version) = self.slot_states[i].try_lock_for_delete() {
                    if locked_version == version {
                        unsafe {
                            let entry_ptr = (*self.storage[i].get()).as_mut_ptr();
                            if !entry_ptr.is_null() {
                                ptr::drop_in_place(entry_ptr);
                            }
                        }
                        self.slot_states[i].mark_empty();
                        self.len.fetch_sub(1, Ordering::Relaxed);
                    } else {
                        self.slot_states[i].mark_valid(locked_version);
                    }
                }
            }
        }
    }

    /// 遍历所有元素（按插入顺序，非键顺序）
    /// 提供一个无堆分配的遍历方法
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&K, &V),
    {
        for i in 0..CAPACITY {
            if let Some(version) = self.slot_states[i].get_version_if_valid() {
                unsafe {
                    let entry_ptr = (*self.storage[i].get()).as_ptr();
                    if !entry_ptr.is_null() {
                        let entry = &*entry_ptr;
                        if entry.slot_version == version as usize {
                            // 再次验证状态仍然有效
                            if self.slot_states[i].get_version_if_valid() == Some(version) {
                                f(&entry.key, &entry.value);
                            }
                        }
                    }
                }
            }
        }
    }
}

// 安全标记
unsafe impl<K, V, const CAPACITY: usize> Send for LockFreeBTreeMap<K, V, CAPACITY>
where
    K: Clone + Ord + Send + Sync,
    V: Clone,
{
}

unsafe impl<K, V, const CAPACITY: usize> Sync for LockFreeBTreeMap<K, V, CAPACITY>
where
    K: Clone + Ord + Send + Sync,
    V: Clone,
{
}

impl<K, V, const CAPACITY: usize> Drop for LockFreeBTreeMap<K, V, CAPACITY>
where
    K: Clone + Ord + Send + Sync,
    V: Clone,
{
    fn drop(&mut self) {
        // 清理所有有效的条目
        for i in 0..CAPACITY {
            if self.slot_states[i].is_valid() {
                unsafe {
                    let entry_ptr = (*self.storage[i].get()).as_mut_ptr();
                    if !entry_ptr.is_null() {
                        ptr::drop_in_place(entry_ptr);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_basic_operations() {
        let map: LockFreeBTreeMap<i32, &'static str, 10> = LockFreeBTreeMap::new();

        // 测试插入
        assert!(map.insert(3, "three").is_some());
        assert!(map.insert(1, "one").is_some());
        assert!(map.insert(2, "two").is_some());

        assert!(!map.is_empty());
        assert_eq!(map.exact_len(), 3);

        // 测试 first_key_value
        if let Some((key, value)) = map.first_key_value() {
            assert_eq!(key, 1);
            assert_eq!(value, "one");
        }

        // 测试 pop_first
        if let Some((key, value)) = map.pop_first() {
            assert_eq!(key, 1);
            assert_eq!(value, "one");
        }

        assert_eq!(map.exact_len(), 2);

        // 再次测试 first_key_value
        if let Some((key, value)) = map.first_key_value() {
            assert_eq!(key, 2);
            assert_eq!(value, "two");
        }
    }

    #[test]
    fn test_update_existing_key() {
        let map: LockFreeBTreeMap<i32, &'static str, 10> = LockFreeBTreeMap::new();

        // 插入初始值
        assert_eq!(map.insert(1, "one"), Some(None));

        // 更新现有键
        assert_eq!(map.insert(1, "ONE"), Some(Some("one")));

        // 验证更新后的值
        if let Some((key, value)) = map.first_key_value() {
            assert_eq!(key, 1);
            assert_eq!(value, "ONE");
        }
    }

    #[test]
    fn test_ordering() {
        let map: LockFreeBTreeMap<i32, &'static str, 20> = LockFreeBTreeMap::new();

        // 乱序插入
        let keys = [15, 3, 8, 1, 12, 6, 20, 4, 9, 2];
        for &key in &keys {
            map.insert(key, "value");
        }

        // 验证弹出顺序是有序的
        let mut popped_keys = [0; 10];
        let mut index = 0;
        while let Some((key, _)) = map.pop_first() {
            if index < 10 {
                popped_keys[index] = key;
                index += 1;
            }
        }

        // 验证是升序
        for i in 1..index {
            assert!(popped_keys[i - 1] < popped_keys[i]);
        }
    }

    #[test]
    fn test_capacity_limit() {
        let map: LockFreeBTreeMap<i32, &'static str, 3> = LockFreeBTreeMap::new();

        // 填满容量
        assert!(map.insert(1, "one").is_some());
        assert!(map.insert(2, "two").is_some());
        assert!(map.insert(3, "three").is_some());

        // 超出容量应该失败
        assert!(map.insert(4, "four").is_none());

        // 但更新现有键应该成功
        assert_eq!(map.insert(2, "TWO"), Some(Some("two")));
    }

    #[test]
    fn test_for_each() {
        let map: LockFreeBTreeMap<i32, &'static str, 10> = LockFreeBTreeMap::new();

        map.insert(3, "three");
        map.insert(1, "one");
        map.insert(2, "two");

        let mut count = 0;
        map.for_each(|_k, _v| {
            count += 1;
        });

        assert_eq!(count, 3);
    }

    #[test]
    fn test_clear() {
        let map: LockFreeBTreeMap<i32, &'static str, 10> = LockFreeBTreeMap::new();

        map.insert(1, "one");
        map.insert(2, "two");
        map.insert(3, "three");

        assert_eq!(map.exact_len(), 3);

        map.clear();

        assert_eq!(map.exact_len(), 0);
        assert!(map.is_empty());
    }

    // 模拟MPMC场景的测试（无真实多线程，但测试逻辑正确性）
    #[test]
    fn test_simulated_concurrent_operations() {
        let map: LockFreeBTreeMap<i32, &'static str, 100> = LockFreeBTreeMap::new();

        // 模拟多个生产者的操作序列
        for producer_id in 0..4 {
            for i in 0..20 {
                let key = producer_id * 20 + i;
                let value = if producer_id % 2 == 0 { "even" } else { "odd" };

                // 模拟并发插入
                if map.insert(key, value).is_none() {
                    // 容量满了，这在测试中不应该发生
                    panic!("Unexpected capacity overflow");
                }
            }
        }

        assert_eq!(map.exact_len(), 80);

        // 模拟多个消费者按序消费
        let mut consumed_keys = [0; 80];
        let mut consumed_count = 0;

        while let Some((key, _)) = map.pop_first() {
            if consumed_count < 80 {
                consumed_keys[consumed_count] = key;
                consumed_count += 1;
            }
        }

        assert_eq!(consumed_count, 80);

        // 验证消费的键是有序的
        for i in 1..consumed_count {
            assert!(consumed_keys[i - 1] < consumed_keys[i]);
        }

        assert!(map.is_empty());
    }

    #[test]
    fn test_stress_same_key_updates() {
        let map: LockFreeBTreeMap<i32, usize, 10> = LockFreeBTreeMap::new();

        let key = 42;
        let mut last_value = None;

        // 模拟多次更新同一个键
        for i in 0..50 {
            match map.insert(key, i) {
                Some(old_val) => {
                    last_value = Some(i);
                    // 验证返回的旧值是合理的
                    if let Some(old) = old_val {
                        assert!(old < i); // 旧值应该小于新值
                    }
                }
                None => {
                    // 不应该失败，因为容量足够
                    panic!("Unexpected insert failure");
                }
            }
        }

        // 最终应该只有一个元素
        assert_eq!(map.exact_len(), 1);

        // 验证最终值
        if let Some((k, v)) = map.first_key_value() {
            assert_eq!(k, key);
            assert_eq!(Some(v), last_value);
        }
    }
}

#[cfg(test)]
mod concurrent_test {
    use super::*;
    use core::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_mpmc() {
        let mut count = 1000;
        while count > 0 {
            count -= 1;
            let pad = 1000usize;

            let flag = Arc::new(AtomicI32::new(3));
            let flag_c = flag.clone();
            let flag1 = flag.clone();
            let flag2 = flag.clone();
            let flag3 = flag.clone();

            let p1 = Arc::new(LockFreeBTreeMap::<usize, usize, 4096>::new());
            let p2 = p1.clone();
            let p3 = p1.clone();
            let c1 = p1.clone();
            let c2 = p1.clone();

            let producer1 = thread::spawn(move || {
                for i in 0..pad {
                    let _ = p1.insert(i, i);
                }
                flag1.fetch_sub(1, Ordering::SeqCst);
            });
            let producer2 = thread::spawn(move || {
                for i in pad..(2 * pad) {
                    let _ = p2.insert(i, i);
                }
                flag2.fetch_sub(1, Ordering::SeqCst);
            });
            let producer3 = thread::spawn(move || {
                for i in (2 * pad)..(3 * pad) {
                    let _ = p3.insert(i, i);
                }
                flag3.fetch_sub(1, Ordering::SeqCst);
            });

            let consumer = thread::spawn(move || {
                let mut sum = 0;
                while flag_c.load(Ordering::SeqCst) != 0 || !c2.is_empty() {
                    if let Some((k, v)) = c2.pop_first() {
                        sum += v;
                    }
                }
                sum
            });

            let mut sum = 0;
            while flag.load(Ordering::SeqCst) != 0 || !c1.is_empty() {
                if let Some((k, v)) = c1.pop_first() {
                    sum += v;
                }
            }

            producer1.join().unwrap();
            producer2.join().unwrap();
            producer3.join().unwrap();

            let s = consumer.join().unwrap();
            sum += s;
            assert_eq!(sum, (0..(3 * pad)).sum());
        }
    }
}
