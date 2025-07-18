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
