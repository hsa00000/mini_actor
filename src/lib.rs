#![allow(dead_code)]

//! # mini_executor
//!
//! A minimalist, lightweight, and intuitive task execution library for Rust, designed to work seamlessly with the Tokio runtime.
//!
//! This crate offers a simple and ergonomic API to spawn asynchronous tasks, manage their lifecycle, and retrieve their results.
//! It supports both individual task execution and efficient batch processing. Whether you need to wait for a task to complete
//! for synchronous-style control flow, execute it in a "fire-and-forget" manner, or group multiple operations into a single
//! batch, `mini_executor` provides a straightforward solution.
//!
//! ## Key Features
//!
//! - **Simple API**: A minimal surface area makes the library easy to learn and use.
//! - **Flexible Execution**: Choose between awaiting a task's result or running it detached.
//! - **Efficient Batching**: Group multiple tasks into a single execution batch to reduce overhead (e.g., for database inserts or logging).
//! - **Built on Tokio**: Leverages the power and efficiency of the `tokio` ecosystem.
//! - **Robust Error Handling**: Individual task panics are captured and returned as errors, not crashed.
//!
//! ## Getting Started
//!
//! To begin, add `mini_executor`, `tokio`, and `dashmap` to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! tokio = { version = "1", features = ["full"] }
//! mini_executor = "2.0" // Replace with the desired version
//! dashmap = "5.5"
//! ```
//!
//! ## Quick Start
//!
//! Here's a quick example demonstrating both individual and batch task execution.
//!
//! ```rust
//! use mini_executor::{TaskExecutor, Task, BatchTask};
//! use tokio::runtime::Runtime;
//! use std::sync::OnceLock;
//! use tokio::time::{sleep, Duration};
//!
//! // -- Define an individual task --
//! struct GreetTask(String);
//!
//! impl Task for GreetTask {
//!     type Output = String;
//!     async fn run(self) -> Self::Output {
//!         format!("Hello, {}!", self.0)
//!     }
//! }
//!
//! // -- Define a batchable task --
//! #[derive(Clone)]
//! struct LogTask(String);
//!
//! impl BatchTask for LogTask {
//!     async fn batch_run(list: Vec<Self>) {
//!         println!("--- Batch Log ({} tasks) ---", list.len());
//!         for task in list {
//!             println!("Logged: {}", task.0);
//!         }
//!     }
//! }
//!
//! // -- Setup and Execution --
//! static RT: OnceLock<Runtime> = OnceLock::new();
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let rt = RT.get_or_init(|| Runtime::new().unwrap());
//!     let executor = TaskExecutor::new(rt);
//!
//!     rt.block_on(async {
//!         // 1. Execute an individual task and wait for its result.
//!         let greeting = executor.execute_waiting(GreetTask("World".into())).await?;
//!         println!("{}", greeting); // Prints: "Hello, World!"
//!
//!         // 2. Execute several batch tasks without waiting.
//!         executor.execute_batch_detached(LogTask("User session started".into()));
//!         executor.execute_batch_detached(LogTask("Data loaded".into()));
//!         
//!         // Allow a moment for the detached batch to be processed.
//!         sleep(Duration::from_millis(10)).await;
//!
//!         // The async block must return a Result because `?` was used.
//!         Ok::<(), Box<dyn std::error::Error>>(())
//!     })?;
//!
//!     Ok(())
//! }
//! ```

use std::any::{Any, TypeId};
use std::future::Future;

use dashmap::{DashMap, mapref::entry::Entry};
use tokio::{
    runtime::Runtime,
    sync::{
        mpsc::{UnboundedSender, UnboundedReceiver, unbounded_channel},
        oneshot,
    },
    task::{JoinError, JoinHandle},
};

/// A trait defining an asynchronously executable unit of work.
///
/// Implement this trait for any struct that represents a task for the `TaskExecutor` to run.
/// The `run` method contains the core logic of the task and is executed asynchronously.
///
/// # Associated Types
///
/// *   `Output`: The type of the value that the task will produce upon successful completion.
///
/// # Type Constraints
///
/// *   `Sized + Send + 'static`: Ensures the task can be owned, moved between threads,
///     and has a lifetime that spans the entire program duration.
/// *   `Output: Send + 'static`: Ensures the task's result can also be safely sent
///     across threads.
///
/// # Example
///
/// ```rust
/// use mini_executor::{TaskExecutor, Task};
/// use tokio::runtime::{Runtime, Builder};
/// use tokio::time::{sleep, Duration};
/// use std::sync::OnceLock;
///
/// // 1. Define the task struct.
/// struct MySimpleTask {
///     id: u32,
/// }
///
/// // 2. Implement the `Task` trait for it.
/// impl Task for MySimpleTask {
///     type Output = String;
///
///     async fn run(self) -> Self::Output {
///         sleep(Duration::from_millis(10)).await;
///         format!("Task {} finished", self.id)
///     }
/// }
///
/// // 3. Execute it using a TaskExecutor.
/// static RT: OnceLock<Runtime> = OnceLock::new();
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let rt = RT.get_or_init(|| {
///         Builder::new_current_thread()
///             .enable_time() // Enable timers for `sleep`
///             .build()
///             .unwrap()
///     });
///     let executor = TaskExecutor::new(rt);
///
///     let result = rt.block_on(async {
///         executor.execute_waiting(MySimpleTask { id: 1 }).await
///     })?;
///
///     assert_eq!(result, "Task 1 finished");
///     println!("{}", result);
///     Ok(())
/// }
/// ```
pub trait Task: Sized + Send + 'static {
    type Output: Send + 'static;

    /// The core logic of the task.
    ///
    /// This method is an `async` function that returns a `Future`, which the `TaskExecutor`
    /// will poll to completion.
    fn run(self) -> impl Future<Output = Self::Output> + Send;
}

/// Represents a task that can be processed in a batch.
///
/// This trait is designed for operations that can be optimized by grouping them together,
/// such as database inserts, logging, or sending notifications. When multiple tasks of the
/// same type are submitted to the `TaskExecutor` in quick succession, they are collected and
/// executed in a single call to `batch_run`. Note that the `batch_run` function does not
/// return a value and its signature must resolve to `()`.
///
/// # Type Constraints
///
/// *   `Sized + Send + 'static`: Ensures the task can be owned and moved between threads.
///
/// # Example
///
/// ```rust
/// use mini_executor::{TaskExecutor, BatchTask};
/// use tokio::runtime::{Runtime, Builder};
/// use std::sync::atomic::{AtomicUsize, Ordering};
/// use std::sync::{Arc, OnceLock};
///
/// // 1. Define a task that holds a reference to a shared counter.
/// #[derive(Clone)]
/// struct IncrementTask(Arc<AtomicUsize>);
///
/// // 2. Implement BatchTask to process multiple increments at once.
/// impl BatchTask for IncrementTask {
///     async fn batch_run(list: Vec<Self>) {
///         if list.is_empty() { return; }
///         // Access the Arc from the *first task* in the vector.
///         let counter = list[0].0.clone();
///         let total_increments = list.len();
///         println!("Batch processing {} increments.", total_increments);
///         counter.fetch_add(total_increments, Ordering::SeqCst);
///     }
/// }
///
/// // 3. Execute the batch task using a TaskExecutor.
/// static RT: OnceLock<Runtime> = OnceLock::new();
///
/// fn main() {
///     let rt = RT.get_or_init(|| Builder::new_current_thread().build().unwrap());
///     let executor = TaskExecutor::new(rt);
///     let counter = Arc::new(AtomicUsize::new(0));
///
///     rt.block_on(async {
///         // We can execute and wait for a batch.
///         executor.execute_batch_waiting(IncrementTask(counter.clone())).await.unwrap();
///         assert_eq!(counter.load(Ordering::SeqCst), 1);
///
///         // Or execute several in a fire-and-forget manner.
///         executor.execute_batch_detached(IncrementTask(counter.clone()));
///         executor.execute_batch_detached(IncrementTask(counter.clone()));
///
///         // Await the final task to ensure the previous detached ones are also processed.
///         executor.execute_batch_waiting(IncrementTask(counter.clone())).await.unwrap();
///     });
///     
///     assert_eq!(counter.load(Ordering::SeqCst), 4);
///     println!("Final counter value: {}", counter.load(Ordering::SeqCst));
/// }
/// ```
pub trait BatchTask: Sized + Send + 'static {
    /// The core logic for processing a batch of tasks.
    ///
    /// This method receives a `Vec<Self>` containing all tasks collected for the batch.
    /// It should perform the work and return a `Future` that resolves when the batch
    /// is complete.
    fn batch_run(list: Vec<Self>) -> impl Future<Output = ()> + Send;
}

// Internal type for passing batch items through the task executor's channels.
// Contains the task and an optional oneshot sender for completion notification.
type BatchItem<BT> = (BT, Option<oneshot::Sender<()>>);

/// A `TaskExecutor` provides a simple interface for spawning tasks onto a Tokio `Runtime`.
///
/// It holds a static reference to a `Runtime`, which acts as the task executor.
/// It also manages background workers for different types of batchable tasks, creating
/// them on demand. This design encourages treating the runtime as a shared, long-lived resource.
///
/// For creation, see [`TaskExecutor::new`].
pub struct TaskExecutor {
    rt: &'static Runtime,
    batch_senders: DashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl TaskExecutor {
    /// Creates a new `TaskExecutor` instance bound to a statically-lived Tokio runtime.
    ///
    /// An application will typically create a single `Runtime` instance that lives for the
    /// duration of the program. To pass a reference of this runtime to the `TaskExecutor`,
    /// it must have a `'static` lifetime. This ensures the `TaskExecutor` can never outlive
    /// the runtime it depends on.
    ///
    /// # Arguments
    ///
    /// * `rt`: A static reference to a `tokio::runtime::Runtime`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use mini_executor::TaskExecutor;
    /// use tokio::runtime::Runtime;
    /// use std::sync::OnceLock;
    ///
    /// // Use OnceLock to create a static runtime reference.
    /// static RT: OnceLock<Runtime> = OnceLock::new();
    ///
    /// fn main() {
    ///     let rt = RT.get_or_init(|| Runtime::new().unwrap());
    ///     let executor = TaskExecutor::new(rt);
    ///     // The task executor is now ready to execute tasks.
    /// }
    /// ```
    pub fn new(rt: &'static Runtime) -> Self {
        TaskExecutor {
            rt,
            batch_senders: DashMap::new(),
        }
    }

    /// Spawns an individual task and asynchronously waits for its result.
    ///
    /// This method submits the task to the executor's runtime and suspends the current
    /// async context until the task has finished.
    ///
    /// # Arguments
    ///
    /// * `task`: An instance of a type that implements the `Task` trait.
    ///
    /// # Returns
    ///
    /// A `Result` containing either:
    /// - `Ok(T::Output)`: The successful output of the task.
    /// - `Err(JoinError)`: An error indicating that the task panicked during execution.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use mini_executor::{TaskExecutor, Task};
    /// # use tokio::runtime::Runtime;
    /// # use std::sync::OnceLock;
    /// #
    /// # static RT: OnceLock<Runtime> = OnceLock::new();
    /// #
    /// struct MyTask;
    /// impl Task for MyTask {
    ///     type Output = u32;
    ///     async fn run(self) -> Self::Output { 42 }
    /// }
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let rt = RT.get_or_init(|| Runtime::new().unwrap());
    /// let executor = TaskExecutor::new(rt);
    ///
    /// rt.block_on(async {
    ///     let result = executor.execute_waiting(MyTask).await?;
    ///     assert_eq!(result, 42);
    ///     Ok::<(), Box<dyn std::error::Error>>(())
    /// })?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_waiting<T: Task>(&self, task: T) -> Result<T::Output, JoinError> {
        let handle = self.rt.spawn(task.run());
        handle.await
    }

    /// Spawns an individual task and immediately returns a `JoinHandle` without awaiting it.
    ///
    /// This is useful for "fire-and-forget" style execution, where the task runs
    /// in the background. The returned `JoinHandle` can still be used to await the task's
    /// completion at a later point if its result is needed.
    ///
    /// # Arguments
    ///
    /// * `task`: An instance of a type that implements the `Task` trait.
    ///
    /// # Returns
    ///
    /// A `JoinHandle` representing the spawned task. Awaiting the handle will
    /// yield a `Result<T::Output, JoinError>`.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use mini_executor::{TaskExecutor, Task};
    /// # use tokio::runtime::Runtime;
    /// # use std::sync::OnceLock;
    /// #
    /// # static RT: OnceLock<Runtime> = OnceLock::new();
    /// #
    /// struct BackgroundTask;
    /// impl Task for BackgroundTask {
    ///     type Output = String;
    ///     async fn run(self) -> Self::Output { "done".to_string() }
    /// }
    /// #
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let rt = RT.get_or_init(|| Runtime::new().unwrap());
    /// let executor = TaskExecutor::new(rt);
    /// #
    /// rt.block_on(async {
    ///     // Spawn the task but don't wait for it yet.
    ///     let handle = executor.execute_detached(BackgroundTask);
    ///
    ///     // We can do other work here...
    ///     println!("Task is running in the background.");
    ///
    ///     // Later, await the handle to get the result.
    ///     let result = handle.await?;
    ///     assert_eq!(result, "done");
    ///     Ok::<(), Box<dyn std::error::Error>>(())
    /// })?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn execute_detached<T: Task>(&self, task: T) -> JoinHandle<T::Output> {
        self.rt.spawn(task.run())
    }

    fn ensure_sender<BT: BatchTask>(&self) -> UnboundedSender<BatchItem<BT>> {
        let key = TypeId::of::<BT>();
        match self.batch_senders.entry(key) {
            Entry::Occupied(o) => o.get()
                .downcast_ref::<UnboundedSender<BatchItem<BT>>>()
                .expect("Type mismatch in batch_senders")
                .clone(),
            Entry::Vacant(v) => {
                let (tx, rx) = unbounded_channel::<BatchItem<BT>>();
                Self::spawn_worker::<BT>(self.rt, rx);
                v.insert(Box::new(tx.clone()));
                tx
            }
        }
    }

    /// Spawns a worker for processing batch tasks of type `BT`.
    ///
    /// The worker runs `batch_run` in a sub-task to isolate panics, preventing
    /// the worker from being killed by panics in user code.
    fn spawn_worker<BT: BatchTask>(rt: &'static Runtime, mut rx: UnboundedReceiver<BatchItem<BT>>) {
        rt.spawn(async move {
            loop {
                // Receive the first task, or break if the channel is closed
                let Some((first_task, first_sig)) = rx.recv().await else { break; };

                // Collect batch tasks
                let mut tasks: Vec<BT> = vec![first_task];
                let mut sigs: Vec<Option<oneshot::Sender<()>>> = vec![first_sig];

                while let Ok((t, s)) = rx.try_recv() {
                    tasks.push(t);
                    sigs.push(s);
                }

                // Run batch_run in a sub-task to isolate panics
                let join = tokio::spawn(async move {
                    BT::batch_run(tasks).await;
                });

                match join.await {
                    Ok(()) => {
                        // Notify all waiting clients on success
                        for s in sigs.into_iter().flatten() {
                            let _ = s.send(());
                        }
                    }
                    Err(_panic) => {
                        // On panic, drop signals without notifying; worker continues
                    }
                }
            }
        });
    }

    /// Executes a batch task asynchronously without waiting for its completion.
    ///
    /// This method submits a task to its corresponding batch processor. If a processor for this
    /// task type does not exist, one is spawned. The task will be collected with other
    /// pending tasks of the same type and executed in a single batch.
    ///
    /// This is a "fire-and-forget" operation.
    ///
    /// # Arguments
    ///
    /// * `batch_task`: An instance of a type that implements `BatchTask`.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use mini_executor::{TaskExecutor, BatchTask};
    /// # use tokio::runtime::Runtime;
    /// # use std::sync::OnceLock;
    /// # use tokio::time::{sleep, Duration};
    /// #
    /// # static RT: OnceLock<Runtime> = OnceLock::new();
    /// #
    /// #[derive(Clone)]
    /// struct LogMessage(String);
    /// impl BatchTask for LogMessage {
    ///     async fn batch_run(list: Vec<Self>) {
    ///         // In a real app, this might write to a file or database.
    ///         println!("-- Logging batch of {} messages --", list.len());
    ///         for msg in list {
    ///             println!("{}", msg.0);
    ///         }
    ///     }
    /// }
    /// #
    /// # fn main() {
    /// let rt = RT.get_or_init(|| Runtime::new().unwrap());
    /// let executor = TaskExecutor::new(rt);
    ///
    /// rt.block_on(async {
    ///     // Fire-and-forget these logging tasks.
    ///     executor.execute_batch_detached(LogMessage("User logged in".to_string()));
    ///     executor.execute_batch_detached(LogMessage("Data processed".to_string()));
    ///
    ///     // Give the batch processor a moment to run.
    ///     sleep(Duration::from_millis(50)).await;
    /// });
    /// # }
    /// ```
    pub fn execute_batch_detached<BT: BatchTask>(&self, batch_task: BT) {
        let key = TypeId::of::<BT>();
        let mut tx = self.ensure_sender::<BT>();

        match tx.send((batch_task, None)) {
            Ok(()) => {}
            Err(e) => {
                // Receiver closed; remove old sender, recreate worker, and resend
                self.batch_senders.remove(&key);
                tx = self.ensure_sender::<BT>();
                let _ = tx.send(e.0);
            }
        }
    }

    /// Executes a batch task and waits for its containing batch to complete.
    ///
    /// This method submits a task to its batch processor and waits for a completion signal.
    /// The signal is sent after the entire batch (which includes this task) has finished processing.
    ///
    /// # Arguments
    ///
    /// * `batch_task`: An instance of a type that implements `BatchTask`.
    ///
    /// # Returns
    ///
    /// A `Result` which is `Ok(())` on successful completion of the batch. It returns
    /// `Err(oneshot::error::RecvError)` if the batch processor panics or is terminated
    /// before sending a completion signal.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use mini_executor::{TaskExecutor, BatchTask};
    /// # use tokio::runtime::Runtime;
    /// # use std::sync::OnceLock;
    /// #
    /// # static RT: OnceLock<Runtime> = OnceLock::new();
    /// #
    /// #[derive(Clone)]
    /// struct DatabaseInsert(u32);
    /// impl BatchTask for DatabaseInsert {
    ///     async fn batch_run(list: Vec<Self>) {
    ///         let ids: Vec<u32> = list.into_iter().map(|item| item.0).collect();
    ///         println!("BATCH INSERT: Simulating inserting IDs: {:?}", ids);
    ///         // In a real app, you'd perform the batched database query here.
    ///     }
    /// }
    /// #
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let rt = RT.get_or_init(|| Runtime::new().unwrap());
    /// let executor = TaskExecutor::new(rt);
    ///
    /// rt.block_on(async {
    ///     // Detach a few tasks first.
    ///     executor.execute_batch_detached(DatabaseInsert(1));
    ///     executor.execute_batch_detached(DatabaseInsert(2));
    ///     
    ///     // Now, execute a task and wait for its batch to complete.
    ///     // This task will be batched with the two above.
    ///     let result = executor.execute_batch_waiting(DatabaseInsert(3)).await;
    ///     
    ///     assert!(result.is_ok());
    ///     println!("Batch confirmed as complete.");
    /// });
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_batch_waiting<BT: BatchTask>(
        &self,
        batch_task: BT,
    ) -> Result<(), oneshot::error::RecvError> {
        let key = TypeId::of::<BT>();
        let (tx_oneshot, rx_oneshot) = oneshot::channel::<()>();

        let mut tx = self.ensure_sender::<BT>();
        match tx.send((batch_task, Some(tx_oneshot))) {
            Ok(()) => {}
            Err(e) => {
                // Receiver closed; remove old sender, recreate worker, and resend
                self.batch_senders.remove(&key);
                tx = self.ensure_sender::<BT>();
                let _ = tx.send(e.0);
            }
        }
        rx_oneshot.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;
    use tokio::runtime::Builder;
    use tokio::time::{sleep, timeout};

    static TEST_RT: OnceLock<Runtime> = OnceLock::new();

    fn get_test_runtime() -> &'static Runtime {
        TEST_RT.get_or_init(|| {
            Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
        })
    }

    // Test task implementations
    #[derive(Debug, Clone)]
    struct SimpleTask {
        id: u32,
        value: String,
    }

    impl Task for SimpleTask {
        type Output = String;

        async fn run(self) -> Self::Output {
            format!("Task {} completed with value: {}", self.id, self.value)
        }
    }

    #[derive(Debug, Clone)]
    struct DelayedTask {
        id: u32,
        delay_ms: u64,
    }

    impl Task for DelayedTask {
        type Output = u32;

        async fn run(self) -> Self::Output {
            sleep(Duration::from_millis(self.delay_ms)).await;
            self.id
        }
    }

    #[derive(Debug, Clone)]
    struct PanicTask;

    impl Task for PanicTask {
        type Output = String;

        async fn run(self) -> Self::Output {
            panic!("This task panics intentionally");
        }
    }

    // Batch task implementations
    #[derive(Debug, Clone)]
    struct CounterTask {
        counter: Arc<AtomicUsize>,
        increment: usize,
    }

    impl BatchTask for CounterTask {
        async fn batch_run(list: Vec<Self>) {
            if list.is_empty() {
                return;
            }

            let counter = &list[0].counter;
            let total_increment: usize = list.iter().map(|task| task.increment).sum();

            counter.fetch_add(total_increment, Ordering::SeqCst);
        }
    }

    #[derive(Debug, Clone)]
    struct LogTask {
        message: String,
        log_storage: Arc<Mutex<Vec<String>>>,
    }

    impl BatchTask for LogTask {
        async fn batch_run(list: Vec<Self>) {
            if list.is_empty() {
                return;
            }

            let log_storage = list[0].log_storage.clone();
            let mut storage = log_storage.lock().unwrap();

            for task in list {
                storage.push(format!("BATCH: {}", task.message));
            }
        }
    }

    #[derive(Debug, Clone)]
    struct DelayedBatchTask {
        id: u32,
        delay_ms: u64,
        results: Arc<Mutex<Vec<u32>>>,
    }

    impl BatchTask for DelayedBatchTask {
        async fn batch_run(list: Vec<Self>) {
            if list.is_empty() {
                return;
            }

            let results = list[0].results.clone();
            let delay_ms = list[0].delay_ms;

            sleep(Duration::from_millis(delay_ms)).await;

            let mut results_guard = results.lock().unwrap();
            for task in list {
                results_guard.push(task.id);
            }
        }
    }

    #[derive(Debug, Clone)]
    struct PanicBatchTask;

    impl BatchTask for PanicBatchTask {
        async fn batch_run(_list: Vec<Self>) {
            panic!("This batch task panics intentionally");
        }
    }

    // Test TaskExecutor::new
    #[test]
    fn test_task_executor_new() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        // Verify task executor is created successfully
        assert_eq!(executor.batch_senders.len(), 0);
    }

    // Test execute_waiting with simple task
    #[test]
    fn test_execute_waiting_simple() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        let result = rt.block_on(async {
            let task = SimpleTask {
                id: 1,
                value: "test".to_string(),
            };
            executor.execute_waiting(task).await
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Task 1 completed with value: test");
    }

    // Test execute_waiting with delayed task
    #[test]
    fn test_execute_waiting_delayed() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        let result = rt.block_on(async {
            let task = DelayedTask {
                id: 42,
                delay_ms: 50,
            };
            executor.execute_waiting(task).await
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    // Test execute_waiting with panic task
    #[test]
    fn test_execute_waiting_panic() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        let result = rt.block_on(async {
            let task = PanicTask;
            executor.execute_waiting(task).await
        });

        assert!(result.is_err());
        assert!(result.unwrap_err().is_panic());
    }

    // Test execute_detached
    #[test]
    fn test_execute_detached() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        let result = rt.block_on(async {
            let task = SimpleTask {
                id: 2,
                value: "detached".to_string(),
            };
            let handle = executor.execute_detached(task);
            handle.await
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Task 2 completed with value: detached");
    }

    // Test execute_detached with panic
    #[test]
    fn test_execute_detached_panic() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        let result = rt.block_on(async {
            let task = PanicTask;
            let handle = executor.execute_detached(task);
            handle.await
        });

        assert!(result.is_err());
        assert!(result.unwrap_err().is_panic());
    }

    // Test execute_batch_detached
    #[test]
    fn test_execute_batch_detached() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);
        let counter = Arc::new(AtomicUsize::new(0));

        rt.block_on(async {
            // Send multiple batch tasks
            executor.execute_batch_detached(CounterTask {
                counter: counter.clone(),
                increment: 1,
            });
            executor.execute_batch_detached(CounterTask {
                counter: counter.clone(),
                increment: 2,
            });
            executor.execute_batch_detached(CounterTask {
                counter: counter.clone(),
                increment: 3,
            });

            // Wait a bit for processing
            sleep(Duration::from_millis(100)).await;
        });

        assert_eq!(counter.load(Ordering::SeqCst), 6);
    }

    // Test execute_batch_waiting
    #[test]
    fn test_execute_batch_waiting() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);
        let counter = Arc::new(AtomicUsize::new(0));

        let result = rt.block_on(async {
            // Send some detached tasks first
            executor.execute_batch_detached(CounterTask {
                counter: counter.clone(),
                increment: 5,
            });
            executor.execute_batch_detached(CounterTask {
                counter: counter.clone(),
                increment: 10,
            });

            // Send a waiting task - this should batch with the above
            executor
                .execute_batch_waiting(CounterTask {
                    counter: counter.clone(),
                    increment: 15,
                })
                .await
        });

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 30);
    }

    // Test batch processing with log storage
    #[test]
    fn test_batch_processing_with_storage() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);
        let log_storage = Arc::new(Mutex::new(Vec::new()));

        rt.block_on(async {
            // Send multiple log tasks
            executor.execute_batch_detached(LogTask {
                message: "First log".to_string(),
                log_storage: log_storage.clone(),
            });
            executor.execute_batch_detached(LogTask {
                message: "Second log".to_string(),
                log_storage: log_storage.clone(),
            });

            // Wait for a batch to complete
            executor
                .execute_batch_waiting(LogTask {
                    message: "Third log".to_string(),
                    log_storage: log_storage.clone(),
                })
                .await
                .unwrap();
        });

        let logs = log_storage.lock().unwrap();
        assert_eq!(logs.len(), 3);
        assert!(logs.contains(&"BATCH: First log".to_string()));
        assert!(logs.contains(&"BATCH: Second log".to_string()));
        assert!(logs.contains(&"BATCH: Third log".to_string()));
    }

    // Test delayed batch processing
    #[test]
    fn test_delayed_batch_processing() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);
        let results = Arc::new(Mutex::new(Vec::new()));

        let result = rt.block_on(async {
            timeout(
                Duration::from_millis(500),
                executor.execute_batch_waiting(DelayedBatchTask {
                    id: 1,
                    delay_ms: 100,
                    results: results.clone(),
                }),
            )
            .await
        });

        assert!(result.is_ok());
        assert!(result.unwrap().is_ok());

        let results_guard = results.lock().unwrap();
        assert_eq!(results_guard.len(), 1);
        assert_eq!(results_guard[0], 1);
    }

    // Test multiple different batch task types
    #[test]
    fn test_multiple_batch_types() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);
        let counter = Arc::new(AtomicUsize::new(0));
        let log_storage = Arc::new(Mutex::new(Vec::new()));

        rt.block_on(async {
            // Submit different types of batch tasks
            executor.execute_batch_detached(CounterTask {
                counter: counter.clone(),
                increment: 100,
            });
            executor.execute_batch_detached(LogTask {
                message: "Mixed batch test".to_string(),
                log_storage: log_storage.clone(),
            });

            // Wait for both to complete
            executor
                .execute_batch_waiting(CounterTask {
                    counter: counter.clone(),
                    increment: 200,
                })
                .await
                .unwrap();
            executor
                .execute_batch_waiting(LogTask {
                    message: "Mixed batch test 2".to_string(),
                    log_storage: log_storage.clone(),
                })
                .await
                .unwrap();
        });

        assert_eq!(counter.load(Ordering::SeqCst), 300);
        let logs = log_storage.lock().unwrap();
        assert_eq!(logs.len(), 2);
    }

    // Test concurrent task execution
    #[test]
    fn test_concurrent_execution() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        let result = rt.block_on(async {
            let mut handles = Vec::new();

            // Start multiple tasks concurrently
            for i in 0..10 {
                let handle = executor.execute_detached(DelayedTask {
                    id: i,
                    delay_ms: 20,
                });
                handles.push(handle);
            }

            // Wait for all tasks to complete
            let mut results = Vec::new();
            for handle in handles {
                results.push(handle.await.unwrap());
            }

            results.sort();
            results
        });

        assert_eq!(result.len(), 10);
        assert_eq!(result, (0..10).collect::<Vec<_>>());
    }

    // Test error handling in batch processing
    #[test]
    fn test_batch_error_handling() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        let result = rt.block_on(async {
            timeout(
                Duration::from_millis(500),
                executor.execute_batch_waiting(PanicBatchTask),
            )
            .await
        });

        // The batch processor should panic, causing the oneshot receiver to be dropped
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }

    // Test that worker survives after batch panics and can process normal batches
    #[test]
    fn batch_panics_but_worker_survives() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        // Trigger a batch that will panic
        let r = rt.block_on(async {
            executor.execute_batch_waiting(PanicBatchTask).await
        });
        assert!(r.is_err());

        // Send a normal batch, should succeed
        #[derive(Clone)]
        struct OkTask(Arc<AtomicUsize>);
        impl BatchTask for OkTask {
            async fn batch_run(list: Vec<Self>) {
                let c = &list[0].0;
                c.fetch_add(list.len(), Ordering::SeqCst);
            }
        }
        let c = Arc::new(AtomicUsize::new(0));
        rt.block_on(async {
            executor.execute_batch_waiting(OkTask(c.clone())).await.unwrap();
        });
        assert_eq!(c.load(Ordering::SeqCst), 1);
    }

    // Test that batch processors are reused
    #[test]
    fn test_batch_processor_reuse() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);
        let counter = Arc::new(AtomicUsize::new(0));

        rt.block_on(async {
            // First batch
            executor
                .execute_batch_waiting(CounterTask {
                    counter: counter.clone(),
                    increment: 1,
                })
                .await
                .unwrap();

            // Second batch - should reuse the same processor
            executor
                .execute_batch_waiting(CounterTask {
                    counter: counter.clone(),
                    increment: 2,
                })
                .await
                .unwrap();
        });

        assert_eq!(counter.load(Ordering::SeqCst), 3);
        // Verify that only one batch processor was created
        assert_eq!(executor.batch_senders.len(), 1);
    }

    // Test empty batch handling
    #[test]
    fn test_empty_batch_handling() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);
        let counter = Arc::new(AtomicUsize::new(0));

        // Create a batch task that handles empty lists
        #[derive(Debug, Clone)]
        struct EmptyBatchTask {
            counter: Arc<AtomicUsize>,
        }

        impl BatchTask for EmptyBatchTask {
            async fn batch_run(list: Vec<Self>) {
                if list.is_empty() {
                    return;
                }
                list[0].counter.fetch_add(1, Ordering::SeqCst);
            }
        }

        let result = rt.block_on(async {
            executor
                .execute_batch_waiting(EmptyBatchTask {
                    counter: counter.clone(),
                })
                .await
        });

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    // Test high volume batch processing
    #[test]
    fn test_high_volume_batch_processing() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);
        let counter = Arc::new(AtomicUsize::new(0));

        rt.block_on(async {
            // Submit many batch tasks quickly
            for i in 0..100 {
                executor.execute_batch_detached(CounterTask {
                    counter: counter.clone(),
                    increment: i,
                });
            }

            // Wait for processing to complete
            executor
                .execute_batch_waiting(CounterTask {
                    counter: counter.clone(),
                    increment: 0,
                })
                .await
                .unwrap();
        });

        // Sum of 0 to 99 is 4950
        assert_eq!(counter.load(Ordering::SeqCst), 4950);
    }

    // Test mixed execution patterns
    #[test]
    fn test_mixed_execution_patterns() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);
        let counter = Arc::new(AtomicUsize::new(0));

        rt.block_on(async {
            // Mix of individual and batch tasks
            let individual_handle = executor.execute_detached(DelayedTask {
                id: 999,
                delay_ms: 50,
            });

            executor.execute_batch_detached(CounterTask {
                counter: counter.clone(),
                increment: 10,
            });

            let individual_result = individual_handle.await.unwrap();
            assert_eq!(individual_result, 999);

            executor
                .execute_batch_waiting(CounterTask {
                    counter: counter.clone(),
                    increment: 20,
                })
                .await
                .unwrap();

            let batch_result = executor
                .execute_waiting(SimpleTask {
                    id: 777,
                    value: "mixed".to_string(),
                })
                .await
                .unwrap();

            assert_eq!(batch_result, "Task 777 completed with value: mixed");
        });

        assert_eq!(counter.load(Ordering::SeqCst), 30);
    }

    // Test same type panics then succeeds (ensure same worker not killed)
    #[test]
    fn same_type_panics_then_succeeds() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        #[derive(Clone)]
        struct SometimesPanicTask {
            counter: Arc<AtomicUsize>,
            should_panic: bool,
        }
        impl BatchTask for SometimesPanicTask {
            async fn batch_run(list: Vec<Self>) {
                if list.iter().any(|t| t.should_panic) {
                    panic!("boom");
                }
                let c = &list[0].counter;
                c.fetch_add(list.len(), Ordering::SeqCst);
            }
        }

        let c = Arc::new(AtomicUsize::new(0));

        // First: panic, waiting end should receive Err
        let r = rt.block_on(async {
            tokio::time::timeout(
                Duration::from_millis(500),
                executor.execute_batch_waiting(SometimesPanicTask { counter: c.clone(), should_panic: true }),
            ).await
        }).expect("timeout waiting for panic batch");
        assert!(r.is_err());

        // Second: same type, normal, should succeed and count +1
        rt.block_on(async {
            executor.execute_batch_waiting(SometimesPanicTask { counter: c.clone(), should_panic: false })
                .await
                .unwrap();
        });
        assert_eq!(c.load(Ordering::SeqCst), 1);
    }

    // Test multiple waiters all get RecvError on panic
    #[test]
    fn multiple_waiters_all_get_recverror_on_panic() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        #[derive(Clone)]
        struct AllPanic;
        impl BatchTask for AllPanic {
            async fn batch_run(_list: Vec<Self>) {
                panic!("batch exploded");
            }
        }

        let (r1, r2, r3) = rt.block_on(async {
            tokio::join!(
                executor.execute_batch_waiting(AllPanic),
                executor.execute_batch_waiting(AllPanic),
                executor.execute_batch_waiting(AllPanic),
            )
        });

        assert!(r1.is_err());
        assert!(r2.is_err());
        assert!(r3.is_err());
    }

    // Test panic storm then recover
    #[test]
    fn panic_storm_then_recover() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        #[derive(Clone)]
        struct SometimesPanic {
            ok_counter: Arc<AtomicUsize>,
            panic: bool,
        }
        impl BatchTask for SometimesPanic {
            async fn batch_run(list: Vec<Self>) {
                if list.iter().any(|t| t.panic) { panic!("storm"); }
                let c = &list[0].ok_counter;
                c.fetch_add(list.len(), Ordering::SeqCst);
            }
        }

        // Continuous 10 panics
        for _ in 0..10 {
            let r = rt.block_on(async { executor.execute_batch_waiting(SometimesPanic { ok_counter: Arc::new(AtomicUsize::new(0)), panic: true }).await });
            assert!(r.is_err());
        }

        // Afterwards should succeed
        let ok = Arc::new(AtomicUsize::new(0));
        rt.block_on(async {
            executor.execute_batch_waiting(SometimesPanic { ok_counter: ok.clone(), panic: false }).await.unwrap();
        });
        assert_eq!(ok.load(Ordering::SeqCst), 1);
    }

    // Test interleaved OK / PANIC / OK (same type, state switching doesn't break)
    #[test]
    fn interleaved_ok_panic_ok_same_type() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        #[derive(Clone)]
        struct FlipTask {
            sum: Arc<AtomicUsize>,
            panic: bool,
        }
        impl BatchTask for FlipTask {
            async fn batch_run(list: Vec<Self>) {
                if list.iter().any(|t| t.panic) { panic!("flip"); }
                list[0].sum.fetch_add(list.len(), Ordering::SeqCst);
            }
        }

        let sum = Arc::new(AtomicUsize::new(0));

        // OK
        rt.block_on(async {
            executor.execute_batch_waiting(FlipTask { sum: sum.clone(), panic: false }).await.unwrap();
        });
        assert_eq!(sum.load(Ordering::SeqCst), 1);

        // PANIC
        rt.block_on(async {
            tokio::time::timeout(
                Duration::from_millis(500),
                executor.execute_batch_waiting(FlipTask { sum: sum.clone(), panic: true }),
            ).await
        }).expect("timeout").unwrap_err();
        // Error is expected

        // OK again
        rt.block_on(async {
            executor.execute_batch_waiting(FlipTask { sum: sum.clone(), panic: false }).await.unwrap();
        });
        assert_eq!(sum.load(Ordering::SeqCst), 2);
    }

    // Test mixed waiters and fire-and-forget on panic, no deadlock (waiters all Err)
    #[test]
    fn detached_and_waiters_mixed_on_panic() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        #[derive(Clone)]
        struct MixPanic;
        impl BatchTask for MixPanic {
            async fn batch_run(_list: Vec<Self>) {
                panic!("oops");
            }
        }

        rt.block_on(async {
            // First send several detached
            for _ in 0..5 {
                executor.execute_batch_detached(MixPanic);
            }

            // Then two waiters; regardless of whether same batch, both should Err
            let (a, b) = tokio::join!(
                executor.execute_batch_waiting(MixPanic),
                executor.execute_batch_waiting(MixPanic),
            );
            assert!(a.is_err());
            assert!(b.is_err());
        });
    }

    // Test large batch coalesces many tasks (lower bound check)
    #[test]
    fn batch_coalesces_many_tasks_lower_bound() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        #[derive(Clone)]
        struct RecordLen {
            last_len: Arc<AtomicUsize>,
        }
        impl BatchTask for RecordLen {
            async fn batch_run(list: Vec<Self>) {
                list[0].last_len.store(list.len(), Ordering::SeqCst);
            }
        }

        let last = Arc::new(AtomicUsize::new(0));

        rt.block_on(async {
            // First send 200 detached
            for _ in 0..200 {
                executor.execute_batch_detached(RecordLen { last_len: last.clone() });
            }
            // Then send one waiter, usually will be taken by same batch as above 200
            executor.execute_batch_waiting(RecordLen { last_len: last.clone() })
                .await
                .unwrap();
        });

        // At least >= 1; in general case will be 201 (all in same batch drained)
        assert!(last.load(Ordering::SeqCst) >= 1);
    }

    // Test many waiters all panic (stress and timeliness): no hang
    #[test]
    fn many_waiters_all_panic_no_hang() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        #[derive(Clone)]
        struct Boom;
        impl BatchTask for Boom {
            async fn batch_run(_list: Vec<Self>) { panic!("boom"); }
        }

        rt.block_on(async {
            let mut results = Vec::new();

            // Create multiple waiters that will all panic
            for _ in 0..50 {
                let result = tokio::time::timeout(
                    Duration::from_secs(1),
                    executor.execute_batch_waiting(Boom)
                ).await;
                results.push(result);
            }

            // All should finish within 1 second (Ok(Err(..)))
            assert!(results.iter().all(|r| r.is_ok() && r.as_ref().unwrap().is_err()));
        });
    }

    // Test waiter dropped midway (drop future), worker send oneshot doesn't break
    #[test]
    fn dropped_waiter_does_not_break_worker() {
        let rt = get_test_runtime();

        #[derive(Clone)]
        struct SlowOk(Arc<AtomicUsize>);
        impl BatchTask for SlowOk {
            async fn batch_run(list: Vec<Self>) {
                tokio::time::sleep(Duration::from_millis(50)).await;
                list[0].0.fetch_add(list.len(), Ordering::SeqCst);
            }
        }

        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        rt.block_on(async {
            // Start one waiter, but quickly cancel it (drop future)
            let rt_inner = get_test_runtime();
            let executor1 = TaskExecutor::new(rt_inner);
            let handle = tokio::spawn(async move {
                executor1.execute_batch_waiting(SlowOk(c1)).await
            });
            tokio::time::sleep(Duration::from_millis(5)).await;
            handle.abort(); // Drop waiter

            // Afterwards send normal waiter, should succeed
            let rt_inner2 = get_test_runtime();
            let executor2 = TaskExecutor::new(rt_inner2);
            executor2.execute_batch_waiting(SlowOk(c2.clone())).await.unwrap();
        });

        // If worker normal, should at least +1 (second waiter)
        assert!(c2.load(Ordering::SeqCst) >= 1);
    }

    // Test multi-type isolation: one type panic doesn't affect another
    #[test]
    fn multi_type_isolation_panic_does_not_affect_other_type() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        #[derive(Clone)]
        struct WillPanic;
        impl BatchTask for WillPanic {
            async fn batch_run(_list: Vec<Self>) { panic!("type A broke"); }
        }

        #[derive(Clone)]
        struct TypeB(Arc<AtomicUsize>);
        impl BatchTask for TypeB {
            async fn batch_run(list: Vec<Self>) {
                list[0].0.fetch_add(list.len(), Ordering::SeqCst);
            }
        }

        // A type: panic
        let ra = rt.block_on(async { executor.execute_batch_waiting(WillPanic).await });
        assert!(ra.is_err());

        // B type: should normal
        let bsum = Arc::new(AtomicUsize::new(0));
        rt.block_on(async {
            executor.execute_batch_waiting(TypeB(bsum.clone())).await.unwrap();
        });
        assert_eq!(bsum.load(Ordering::SeqCst), 1);
    }

    // Test large scale same type mixed (mostly OK, few PANIC), ensure recovery
    #[test]
    fn large_mixed_after_panics_recovers() {
        let rt = get_test_runtime();
        let executor = TaskExecutor::new(rt);

        #[derive(Clone)]
        struct MaybePanic {
            ok: Arc<AtomicUsize>,
            panic: bool,
        }
        impl BatchTask for MaybePanic {
            async fn batch_run(list: Vec<Self>) {
                if list.iter().any(|t| t.panic) { panic!("mixed"); }
                list[0].ok.fetch_add(list.len(), Ordering::SeqCst);
            }
        }

        // First send a wave of panics
        for _ in 0..5 {
            let r = rt.block_on(async { executor.execute_batch_waiting(MaybePanic { ok: Arc::new(AtomicUsize::new(0)), panic: true }).await });
            assert!(r.is_err());
        }

        // Then large amount of OK
        let ok = Arc::new(AtomicUsize::new(0));
        rt.block_on(async {
            for _ in 0..100 {
                executor.execute_batch_detached(MaybePanic { ok: ok.clone(), panic: false });
            }
            executor.execute_batch_waiting(MaybePanic { ok: ok.clone(), panic: false }).await.unwrap();
        });

        assert!(ok.load(Ordering::SeqCst) >= 1);
    }
}
