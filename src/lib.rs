//! # mini_actor
//!
//! A minimalist, lightweight, and intuitive actor-like library for Rust, designed to work seamlessly with the Tokio runtime.
//!
//! This crate offers a simple and ergonomic API to spawn asynchronous tasks, manage their lifecycle, and retrieve their results.
//! Whether you need to wait for a task to complete for synchronous-style control flow or execute it in a "fire-and-forget"
//! manner, `mini_actor` provides a straightforward solution.
//!
//! ## Key Features
//!
//! - **Simple API**: A minimal surface area makes the library easy to learn and use.
//! - **Flexible Execution**: Choose between awaiting a task's result or running it detached.
//! - **Built on Tokio**: Leverages the power and efficiency of the `tokio` ecosystem.
//! - **Robust Error Handling**: Task panics are captured and returned as errors, not crashed.
//!
//! ## Getting Started
//!
//! To begin, add `mini_actor` and `tokio` to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! tokio = { version = "1", features = ["full"] }
//! mini_actor = "0.1" # Replace with the desired version
//! rayon = "1.10"
//! tokio-rayon = "2.2"
//! ```
//!
//! You'll need a `tokio::runtime::Runtime` to power the `Actor`. You can then define your tasks and use the actor to execute them.
//!
//! ## Example Usage
//!
//! ```rust
//! use std::time::Duration;
//! use tokio::runtime::Runtime;
//! use tokio::time::sleep;
//! use mini_actor::{Actor, Task};
//! use std::sync::OnceLock;
//!
//! // 1. It's common practice to initialize the runtime as a static singleton.
//! //    `std::sync::OnceLock` is a great way to achieve this.
//! static RT: OnceLock<Runtime> = OnceLock::new();
//!
//! // 2. Define a task by implementing the `Task` trait.
//! //    This task will take a string, sleep for a second, and return its length.
//! struct MyTask {
//!     input: String,
//! }
//!
//! impl Task for MyTask {
//!     type Output = usize;
//!
//!     async fn run(self) -> Self::Output {
//!         println!("Task received input: '{}'", self.input);
//!         sleep(Duration::from_secs(1)).await;
//!         println!("Task finished processing.");
//!         self.input.len()
//!     }
//! }
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 3. Initialize and start the runtime.
//!     let rt = RT.get_or_init(|| Runtime::new().unwrap());
//!     let actor = Actor::new(rt);
//!
//!     // 4. Execute the task and wait for the result.
//!     //    The `?` operator propagates a potential task panic.
//!     let task = MyTask { input: "hello".to_string() };
//!     let result = rt.block_on(async {
//!         actor.execute_waiting(task).await
//!     })?;
//!     println!("Result from awaited task: {}", result);
//!
//!     // Or, execute a task and detach it ("fire-and-forget").
//!     let detached_task = MyTask { input: "world".to_string() };
//!     let handle = actor.execute_detached(detached_task);
//!     println!("Detached task is running in the background.");
//!
//!     // We can optionally wait for the detached handle later.
//!     let result_from_handle = rt.block_on(async { handle.await })?;
//!     println!("Result from detached task: {}", result_from_handle);
//!
//!     Ok(())
//! }
//! ```
//!
use tokio::{runtime::Runtime, task::{JoinError, JoinHandle}};

/// A trait defining an asynchronously executable unit of work.
///
/// Implement this trait for any struct that represents a task for the `Actor` to run.
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
pub trait Task: Sized + Send + 'static {
    type Output: Send + 'static;

    /// The core logic of the task.
    ///
    /// This method is an `async` function that returns a `Future`, which the `Actor`
    /// will poll to completion.
    ///
    /// ### Handling Blocking Operations
    ///
    /// It is crucial to **never** perform blocking operations directly within an async `run` method,
    /// as this will stall the Tokio runtime thread, preventing other tasks from making progress.
    /// Instead, offload blocking work to a thread pool appropriate for the task.
    ///
    /// #### Example: I/O-Bound Blocking Task
    ///
    /// For tasks that perform blocking I/O (e.g., interacting with a standard file or a blocking database driver),
    /// use `tokio::task::spawn_blocking`. This moves the blocking operation to a dedicated Tokio thread pool,
    /// allowing the async runtime to remain responsive.
    ///
    /// ```rust
    /// # use mini_actor::Task;
    /// use std::fs;
    /// use std::io::Read;
    ///
    /// struct ReadFileTask {
    ///     path: String,
    /// }
    ///
    /// impl Task for ReadFileTask {
    ///     type Output = Result<String, std::io::Error>;
    ///
    ///     async fn run(self) -> Self::Output {
    ///         tokio::task::spawn_blocking(move || {
    ///             let mut file = fs::File::open(self.path)?;
    ///             let mut contents = String::new();
    ///             file.read_to_string(&mut contents)?;
    ///             Ok(contents)
    ///         }).await.unwrap() // .await on the JoinHandle, leaving the inner Result
    ///     }
    /// }
    /// ```
    ///
    /// #### Example: CPU-Bound Parallel Task with `tokio-rayon`
    ///
    /// For CPU-intensive computations that can be parallelized (e.g., data processing, image resizing),
    /// the `tokio-rayon` crate provides a bridge to run Rayon-powered tasks without blocking the Tokio runtime.
    ///
    /// First, add the dependencies to `Cargo.toml`:
    /// ```toml
    /// [dependencies]
    /// rayon = "1.10"
    /// tokio-rayon = "2.2"
    /// ```
    ///
    /// Then, use `tokio_rayon::spawn` to run the parallel computation.
    /// ```rust
    /// # use mini_actor::Task;
    /// // Make sure to add `rayon` and `tokio-rayon` to your Cargo.toml
    /// use rayon::prelude::*;
    ///
    /// struct CpuIntensiveTask {
    ///     data: Vec<u64>,
    /// }
    ///
    /// impl Task for CpuIntensiveTask {
    ///     type Output = u64;
    ///
    ///     async fn run(self) -> Self::Output {
    ///         // `tokio_rayon::spawn` moves the computation to a Rayon thread pool
    ///         // and returns a handle that we can .await.
    ///         tokio_rayon::spawn(move || {
    ///             self.data.into_par_iter()
    ///                 .map(|n| n * n) // some parallel computation
    ///                 .sum()
    ///         }).await
    ///     }
    /// }
    /// ```
    fn run(self) -> impl std::future::Future<Output = Self::Output> + Send;
}

/// An `Actor` provides a simple interface for spawning tasks onto a Tokio `Runtime`.
///
/// It holds a static reference to a `Runtime`, which acts as the task executor.
/// This design encourages treating the runtime as a shared, long-lived resource.
pub struct Actor {
    rt: &'static Runtime,
}

impl Actor {
    /// Creates a new `Actor` instance bound to a statically-lived Tokio runtime.
    ///
    /// An application will typically create a single `Runtime` instance that lives for the
    /// duration of the program. To pass a reference of this runtime to the `Actor`,
    /// it must have a `'static` lifetime. This ensures the `Actor` can never outlive
    /// the runtime it depends on.
    ///
    /// The two most common patterns to achieve this are shown below.
    ///
    /// # Arguments
    ///
    /// * `rt`: A static reference to a `tokio::runtime::Runtime`.
    ///
    /// # Example using `std::sync::OnceLock`
    /// This is a very common and safe pattern for creating a global, static reference
    /// to a resource that is initialized at runtime.
    ///
    /// ```rust
    /// # use mini_actor::Actor;
    /// use tokio::runtime::Runtime;
    /// use std::sync::OnceLock;
    ///
    /// static RT: OnceLock<Runtime> = OnceLock::new();
    ///
    /// fn main() {
    ///     let rt = RT.get_or_init(|| Runtime::new().unwrap());
    ///     let actor = Actor::new(rt);
    ///     // ... actor can now be used
    /// }
    /// ```
    ///
    /// # Example using `Box::leak`
    /// This pattern creates a `'static` reference by intentionally leaking memory, which is
    /// an acceptable trade-off for singletons that must exist for the program's entire lifetime.
    ///
    /// ```rust
    /// # use mini_actor::Actor;
    /// use tokio::runtime::Runtime;
    ///
    /// let rt = Box::leak(Box::new(Runtime::new().unwrap()));
    /// let actor = Actor::new(rt);
    /// // ... actor can now be used
    /// ```
    pub fn new(rt: &'static Runtime) -> Self {
        Actor { rt }
    }

    /// Spawns a task and asynchronously waits for its result.
    ///
    /// This method submits the task to the actor's runtime and suspends the current
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
    /// # use mini_actor::{Actor, Task};
    /// # use tokio::runtime::Runtime;
    /// # use std::sync::OnceLock;
    /// # static RT: OnceLock<Runtime> = OnceLock::new();
    /// # struct MyTask;
    /// # impl Task for MyTask { type Output = u32; async fn run(self) -> Self::Output { 42 } }
    /// # fn main() {
    /// #   let rt = RT.get_or_init(|| Runtime::new().unwrap());
    /// #   let actor = Actor::new(rt);
    /// #   rt.block_on(async {
    /// // .await returns a Result, which can be handled or propagated with `?`
    /// let result = actor.execute_waiting(MyTask).await;
    /// assert!(result.is_ok());
    /// assert_eq!(result.unwrap(), 42);
    /// #   });
    /// # }
    /// ```
    pub async fn execute_waiting<T: Task>(&self, task: T) -> Result<T::Output, JoinError> {
        let handle = self.rt.spawn(task.run());
        handle.await
    }

    /// Spawns a task and immediately returns a `JoinHandle` without awaiting it.
    ///
    /// This is useful for "fire-and-forget" style execution, where the task runs
    /// in the background. The returned `JoinHandle` can still be used to await the task's
    /// completion at a later point if its result is needed.
    ///
    /// # Arguments
    ///
    /// * `task`: An instance of a type that implements the `Task` trait.
    ///
    /// `Ok(T::Output)`: The successful output of the task.
    /// `Err(JoinError)`: An error indicating that the task panicked during execution.
    ///
    /// # Returns
    ///
    /// A `JoinHandle` representing the spawned task. Awaiting the handle will
    /// yield a `Result<T::Output, JoinError>`.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use mini_actor::{Actor, Task};
    /// # use tokio::runtime::Runtime;
    /// # use std::sync::OnceLock;
    /// # static RT: OnceLock<Runtime> = OnceLock::new();
    /// # struct MyTask;
    /// # impl Task for MyTask { type Output = u32; async fn run(self) -> Self::Output { 42 } }
    /// # fn main() {
    /// #   let rt = RT.get_or_init(|| Runtime::new().unwrap());
    /// #   let actor = Actor::new(rt);
    /// #   rt.block_on(async {
    /// let handle = actor.execute_detached(MyTask);
    /// // You can perform other work here while the task runs in the background.
    ///
    /// // Later, you can await the handle to get the result.
    /// let result = handle.await.unwrap();
    /// assert_eq!(result, 42);
    /// #   });
    /// # }
    /// ```
    pub fn execute_detached<T: Task>(&self, task: T) -> JoinHandle<T::Output> {
        self.rt.spawn(task.run())
    }
}