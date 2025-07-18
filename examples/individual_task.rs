use tokio::runtime::Runtime;
use mini_executor::{TaskExecutor, Task};

struct MyTask;
impl Task for MyTask {
    type Output = String;
    fn run(self) -> impl std::future::Future<Output = Self::Output> + Send {
        async move { "hello".to_string() }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = Box::leak(Box::new(Runtime::new().unwrap()));
    let executor = TaskExecutor::new(rt);
    
    let result = rt.block_on(async {
        executor.execute_waiting(MyTask).await
    })?;
    
    println!("Result: {}", result);
    Ok(())
}
