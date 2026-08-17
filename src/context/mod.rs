//! Shared Project Context stored in `.orbit/`.

pub mod digest;
pub mod skills;
pub mod store;

pub use digest::{HandoffSummary, build_digest, build_handoff};
pub use skills::Skill;
pub use store::{OrbitStore, SessionRecord, TaskStatus};
