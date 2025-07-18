# Migration Notice: mini_actor → mini_executor

## 🚨 Important Notice

The `mini_actor` crate has been **renamed** to `mini_executor` starting from version 2.0.0.

### Why the change?

Based on community feedback, the name "mini_actor" was misleading as this library doesn't implement the traditional actor model pattern. Instead, it's a task execution library with batch processing capabilities. The new name `mini_executor` better reflects its actual purpose.

### Migration Guide

#### For existing users of `mini_actor` v1.0.0:

1. **Update your Cargo.toml:**
   ```toml
   # OLD
   [dependencies]
   mini_actor = "1.0"
   
   # NEW
   [dependencies]
   mini_executor = "2.0"
   ```

2. **Update your imports:**
   ```rust
   // OLD
   use mini_actor::{Actor, Task, BatchTask};
   
   // NEW
   use mini_executor::{TaskExecutor, Task, BatchTask};
   ```

3. **Update your code:**
   ```rust
   // OLD
   let actor = Actor::new(rt);
   let result = actor.execute_waiting(task).await;
   
   // NEW
   let executor = TaskExecutor::new(rt);
   let result = executor.execute_waiting(task).await;
   ```

### What's New in v2.0.0?

- **Better naming**: `Actor` → `TaskExecutor`
- **Same functionality**: All the features you love remain unchanged
- **Improved documentation**: Clearer examples and better API documentation
- **Better semantic versioning**: The major version bump reflects the breaking API changes

### Timeline

- **mini_actor v1.0.0**: Still available but deprecated
- **mini_executor v2.0.0**: The new, actively maintained version
- **mini_actor v1.x**: Will receive critical bug fixes only

### Questions?

If you have any questions about the migration, please:
1. Check the documentation at [docs.rs/mini_executor](https://docs.rs/mini_executor)
2. Open an issue in the [GitHub repository](https://github.com/hsa00000/mini_executor)
