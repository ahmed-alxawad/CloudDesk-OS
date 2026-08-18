//! Shared optional-runtime orchestrator (Phase 6): one typed lifecycle,
//! isolation, resource-policy, and observability framework that future
//! Code/Office/Browser runtimes consume, instead of each inventing their
//! own process supervision, storage layout, port allocation, health
//! monitoring, idle shutdown, resource limits, logging, and crash
//! handling.
//!
//! Deliberately does not touch `clouddesk-media`'s existing, working
//! Phase 3 process lifecycle -- see the module docs on why, and what
//! resource-policy primitives here `clouddesk-media` could adopt later
//! without a rewrite.

pub mod adapter;
pub mod cgroup;
pub mod host_process;
pub mod manager;
pub mod model;
pub mod oci;
pub mod port;
pub mod proxy;
pub mod storage;
pub mod store;

pub use adapter::{Availability, HealthStatus, InstanceContext, RunningHandle, RuntimeAdapter};
pub use manager::{RuntimeManager, StartError, StopError};
pub use model::{Generation, InstanceId, InstanceState, Persistence, ResourcePolicy, RuntimeKind};
