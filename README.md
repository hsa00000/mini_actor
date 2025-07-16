# mini_actor

The smallest, simplest Rust actor model built on Tokio runtime.

## Features

- Minimal trait-based design (`Task`)
- Simple `Actor` that runs tasks on a Tokio runtime
- Two modes: `execute_waiting()` and `execute_detached()`

## Example

```rust
use tokio::runtime::Runtime;
use mini_actor::{Actor, Task};

struct MyTask;
impl Task for MyTask {
    type Output = String;
    fn run(self) -> impl std::future::Future<Output = Self::Output> + Send {
        async move { "hello".to_string() }
    }
}

let rt = Box::leak(Box::new(Runtime::new().unwrap()));
let actor = Actor::new(rt);
let result = actor.execute_waiting(MyTask).await;
