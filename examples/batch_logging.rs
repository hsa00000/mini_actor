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
