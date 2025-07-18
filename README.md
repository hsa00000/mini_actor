# mini_executor

The smallest, simplest Rust task executor built on Tokio runtime.

## Features

- Minimal trait-based design (`Task` and `BatchTask`)
- Simple `TaskExecutor` that runs tasks on a Tokio runtime
- Individual task execution: `execute_waiting()` and `execute_detached()`
- Batch processing: `execute_batch_waiting()` and `execute_batch_detached()`

## Example

```rust
use tokio::runtime::Runtime;
use mini_executor::{TaskExecutor, Task};

struct MyTask;
impl Task for MyTask {
    type Output = String;
    fn run(self) -> impl std::future::Future<Output = Self::Output> + Send {
        async move { "hello".to_string() }
    }
}

let rt = Box::leak(Box::new(Runtime::new().unwrap()));
let executor = TaskExecutor::new(rt);
let result = executor.execute_waiting(MyTask).await;
