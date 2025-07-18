# mini_executor

The smallest, simplest Rust task executor built on Tokio runtime.

## Features

- Minimal trait-based design (`Task` and `BatchTask`)
- Simple `TaskExecutor` that runs tasks on a Tokio runtime
- Individual task execution: `execute_waiting()` and `execute_detached()`
- Batch processing: `execute_batch_waiting()` and `execute_batch_detached()`

## Examples

### Individual Task Execution

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
```

### Batch Processing Example

```rust
use tokio::runtime::Runtime;
use mini_executor::{TaskExecutor, BatchTask};
use std::sync::{Arc, Mutex};

// Define a batch task for logging
#[derive(Clone)]
struct LogTask {
    message: String,
    storage: Arc<Mutex<Vec<String>>>,
}

impl BatchTask for LogTask {
    async fn batch_run(list: Vec<Self>) {
        // Process all tasks in the batch together
        let storage = list[0].storage.clone();
        let mut logs = storage.lock().unwrap();
        
        println!("Processing batch of {} log messages", list.len());
        for task in list {
            logs.push(format!("[BATCH] {}", task.message));
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = Box::leak(Box::new(Runtime::new().unwrap()));
    let executor = TaskExecutor::new(rt);
    let storage = Arc::new(Mutex::new(Vec::new()));

    rt.block_on(async {
        // Fire-and-forget batch tasks
        executor.execute_batch_detached(LogTask {
            message: "User logged in".to_string(),
            storage: storage.clone(),
        });
        
        executor.execute_batch_detached(LogTask {
            message: "Data processed".to_string(),
            storage: storage.clone(),
        });
        
        executor.execute_batch_detached(LogTask {
            message: "Cache updated".to_string(),
            storage: storage.clone(),
        });

        // Wait for the batch to complete
        executor.execute_batch_waiting(LogTask {
            message: "Operation finished".to_string(),
            storage: storage.clone(),
        }).await?;

        // Check results
        let logs = storage.lock().unwrap();
        println!("Total logs processed: {}", logs.len());
        for log in logs.iter() {
            println!("{}", log);
        }

        Ok(())
    })
}
```

### Database Batch Insert Example

```rust
use tokio::runtime::Runtime;
use mini_executor::{TaskExecutor, BatchTask};

#[derive(Clone)]
struct DatabaseInsert {
    id: u32,
    name: String,
}

impl BatchTask for DatabaseInsert {
    async fn batch_run(list: Vec<Self>) {
        // Simulate batched database insertion
        let ids: Vec<u32> = list.iter().map(|item| item.id).collect();
        let names: Vec<String> = list.iter().map(|item| item.name.clone()).collect();
        
        println!("BATCH INSERT: Inserting {} records", list.len());
        println!("IDs: {:?}", ids);
        println!("Names: {:?}", names);
        
        // In a real application, you would execute:
        // INSERT INTO users (id, name) VALUES (?, ?), (?, ?), ...
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        println!("Batch insert completed successfully");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = Box::leak(Box::new(Runtime::new().unwrap()));
    let executor = TaskExecutor::new(rt);

    rt.block_on(async {
        // Submit multiple database inserts
        executor.execute_batch_detached(DatabaseInsert {
            id: 1,
            name: "Alice".to_string(),
        });
        
        executor.execute_batch_detached(DatabaseInsert {
            id: 2,
            name: "Bob".to_string(),
        });
        
        executor.execute_batch_detached(DatabaseInsert {
            id: 3,
            name: "Charlie".to_string(),
        });

        // Wait for all inserts to complete
        executor.execute_batch_waiting(DatabaseInsert {
            id: 4,
            name: "David".to_string(),
        }).await?;

        println!("All database operations completed!");
        Ok(())
    })
}
```

## Key Benefits of Batch Processing

1. **Efficiency**: Reduce overhead by processing multiple similar tasks together
2. **Database Optimization**: Batch multiple database operations into single queries
3. **Rate Limiting**: Control resource usage by processing tasks in groups
4. **Atomic Operations**: Ensure related tasks are processed together

## API Overview

- `execute_waiting(task)` - Execute individual task and wait for result
- `execute_detached(task)` - Execute individual task without waiting
- `execute_batch_waiting(task)` - Add task to batch and wait for batch completion
- `execute_batch_detached(task)` - Add task to batch without waiting

## Running Examples

All examples in this README are fully tested and runnable. You can try them out:

```bash
# Run the individual task example
cargo run --example individual_task

# Run the batch logging example
cargo run --example batch_logging

# Run the database batch example
cargo run --example database_batch
```
