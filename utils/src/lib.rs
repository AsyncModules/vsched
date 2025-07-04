#![cfg_attr(not(test), no_std)]

mod deque;
pub use deque::LockFreeDeque;
mod btreemap;
pub use btreemap::LockFreeBTreeMap;
