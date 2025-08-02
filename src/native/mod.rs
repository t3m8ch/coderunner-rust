/// Native module contains implementations of core traits
/// without using any external dependencies like Docker or Runc,
/// using syscalls directly instead.
pub mod executor;
