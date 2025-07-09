# `vsched`[^1] 模块的实现

## 模块内部实现

目录如下：

```shell
├── Cargo.toml
└── src
    ├── api.rs
    ├── lib.rs
    ├── percpu.rs
    ├── sched.rs
    └── task.rs
```

1. `api.rs` 中实现了模块内部向外暴露的接口[^2]。
2. `lib.rs` 中实现了获取数据段基址的方法 `get_data_base`，这里通过 `#[inline(never)]` 以及确保其在代码段中的偏移小于 0x1000 来获取到数据段的基址。
3. `percpu.rs` 中定义了 percpu 调度需要的 `PerCPU` 数据结构。
4. `sched.rs` 中提供了 `api.rs` 中暴露的接口的具体实现。
5. `task.rs` 中定义了最基础的任务控制块的结构 `TaskInner`[^3]。

### 接口

**vsched 模块暴露的接口的具体实现见[此处](https://github.com/AsyncModules/vsched/tree/main/vsched)。**

#### 模块初始化相关的接口

1. `pub extern "C" fn init_vsched(cpu_id: usize, idle_task: BaseTaskRef);`
2. `pub extern "C" fn init_vsched_secondary(cpu_id: usize, idle_task: BaseTaskRef);`

这两个接口用于初始化 vsched 模块使用的 PerCPU 数据，包括 cpu_id，就绪任务队列，当前任务等。



#### 任务状态相关的接口

1. `pub extern "C" fn spawn(task_ref: BaseTaskRef) -> BaseTaskRef;`：创建 $\rightarrow$ 就绪
2. `pub extern "C" fn yield_now(cpu_id: usize);`：运行 $\rightarrow$ 就绪
3. `pub extern "C" fn unblock_task(task: BaseTaskRef, resched: bool, src_cpu_id: usize, dst_cpu_id: usize);`：阻塞 $\rightarrow$ 就绪
4. `pub extern "C" fn resched(cpu_id: usize);`：这个接口会导致当前任务运行 $\rightarrow$ 阻塞/退出态，下一个任务就绪 $\rightarrow$ 运行。暴露这个接口，是因为当前任务的状态转换（例如运行 $\rightarrow$ 阻塞 blocked_resched、运行 $\rightarrow$ 退出 exit）需要将任务队列保存在进程各自的私有的阻塞队列中，需要外部库操作了私有队列之后，再调用这个接口进行调度。
5. `pub extern "C" fn resched_f(cpu_id: usize) -> bool;`：同 `resched`，但其用于协程调度。
6. `pub extern "C" fn switch_to(cpu_id: usize, prev_task: &BaseTaskRef, next_task: BaseTaskRef);`：进行任务迁移时，需要直接使用这个接口，而不是使用 `resched` 接口。



#### 与任务相关的接口

1. `pub extern "C" fn prev_task(cpu_id: usize) -> BaseTaskRef;`：获取 CPU 上运行的前一个任务，通过 `prev_task.on_cpu()` 来保证调度时，不会出现同一个任务在多个 CPU 上运行；

2. `pub extern "C" fn current(cpu_id: usize) -> BaseTaskRef;`：获取 CPU 上正在运行的任务；

3. `pub extern "C" fn set_priority(prio: isize, cpu_id: usize) -> bool;`：设置 CPU 上正在运行的任务的优先级；



### 位置无关的无锁任务队列

在描述无锁任务队列之前，先说明任务控制块。任务控制块除了 `TaskInner` 中的字段外，其余的扩展字段按照 Arceos 中的方式，都通过 task_ext 字段来记录其指针。因此不同的地址空间、特权级下的任务都具有相同的 TaskInner 接口，具体的不同则是在 task_ext 字段记录的指针所指向的不同的 `TaskExt` 结构。

无锁任务队列中则保存 TaskInner 的指针，通过这种方式实现了不同地址空间和特权级的任务都可以保存在同一个任务队列，通过同一个调度器进行调度和任务切换。

使用的无锁数据结构见[此处](https://github.com/AsyncModules/vsched/tree/main/utils)，调度器的实现见[此处](https://github.com/AsyncModules/vsched/tree/main/scheduler)，实现了 `fifo`、`round-robin`、`cfs` 调度器。



### 任务上下文

任务上下文 `TaskContext` 定义在[此处](https://github.com/AsyncModules/vsched/tree/main/hal)，但是目前还不完善，因为没有增加地址空间和特权级等字段，只包含函数调用所规定的通用寄存器的结构，后续需要继续完善。

上下文切换也只切换通用寄存器，不会对 tls、页表等进行操作。

后续需要根据地址空间和特权级进行不同的切换（以下的描述不完整，请忽略）：

1. **同地址空间、不切换特权级**：**两个任务都属于同一个进程、且都处于用户态**，切换时，只禁用抢占，不对中断进行操作，允许发生中断；**两个任务都属于同一个进程、且都处于内核态**，切换时禁用中断和抢占；
2. **同地址空间、切换特权级**：**两个任务属于同一个进程、但属于不同的地址空间**，使用当前 Arceos 中的陷入和返回用户态的方式即可；
3. **不同地址空间、不切换特权级**：**两个任务属于不同的地址空间，但都处于内核态**，切换时关闭中断且禁用抢占；
4. **不同地址空间、切换特权级**：两个任务属于不同地址空间，但都处于用户态，这种切换可节省在内核中的路径，进行快速切换。这里看不到下一个任务的标识，只能通过看到操作系统标识和进程标识，进程标识，看不到任务标识，需要陷入到 S 态，才可以看到下一个任务的标识。



## 接口层 vsched_apis

这是 vsched 提供给外部库使用的接口层，实现见[此处](https://github.com/AsyncModules/vsched/tree/main/vsched_apis)，通过 build.rs 自动化构建，不需要手动进行修改。



## 用户态测试 user_test

测试在标准库环境下进行，实现了一个基于 vsched 的用户态的任务运行时，见[此处](https://github.com/AsyncModules/vsched/tree/main/user_test)。

测试只能在 linux 环境下运行，因为需要通过 qemu-$(ARCH) 用户态模拟环境来，mac 上缺少支持。编译测试时静态链接了标准库环境。



### 用户态的任务控制块

任务控制块的定义见[此处](https://github.com/AsyncModules/vsched/blob/main/user_test/src/task.rs)，一些方法（例如 join）需要通过 `TaskExt` 和 `TaskInner` 相互配合。



### 用户态运行时

见[此处](https://github.com/AsyncModules/vsched/blob/main/user_test/src/vsched.rs)，这里提供了 `exit`、`blocked_resched` 接口，并配合 `vsched_apis` 暴露的 `spawn`、`yield`、`unblock_task` 实现了完整的任务调度运行时。

[`wait_queue`](https://github.com/AsyncModules/vsched/blob/main/user_test/src/wait_queue.rs) 利用上一段描述的 `blocked_resched` 接口实现了阻塞队列，提供了 `wait`、`wait_until` 等方法，但实现不完善。

此外这里提供了初始化运行时的方法。



### 测试用例

测试用例见[此处](https://github.com/AsyncModules/vsched/tree/main/user_test/src/bin)，测试了 `init`、`spawn`、`yield`、`wait`、`multi_thread`。其中 `multi_thread` 通过创建两个线程，每个线程使用一个 vsched 的调度器，进行并行测试，目前测试逻辑还比较简单。



[^1]: vsched：vDSO based scheduling（基于 vDSO 的调度）。
[^2]: 暴露的接口需要使用 `#[unsafe(no_mangle)]` 属性标记，否则不会生成对应的动态符号表项。
[^3]: `TaskInner` 只包括 id、状态、上下文、扩展字段等基础的信息，除扩展字段外，其余的字段用于模块内部的调度和切换。









