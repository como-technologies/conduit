//! conduit — forge-neutral agentic development harness (Adopt-stage engine).
//!
//! Library crate: all logic lives here; `main.rs` is clap marshalling +
//! human rendering only. Spec: docs/src/dev/spike-design.md.

pub mod cli;
pub mod config;
pub mod contract;
pub mod machine;
pub mod store;
pub mod task;
