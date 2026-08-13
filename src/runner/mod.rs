//! Long-running process groups (Job Object / process group).

pub mod ansi;
pub mod process;

pub use process::{ProcessRegistry, ProcessStatus, ProcessView};
