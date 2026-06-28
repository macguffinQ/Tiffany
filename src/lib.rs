//! orchestrator: multi-agent orchestration platform.
//!
//! Public re-exports for embedding orchestrator as a library.

extern crate self as orchestrator;

pub mod acp;
pub mod adapters;
pub mod agent_events;
pub mod agent_md;
pub mod cc_config;
pub mod cc_session_import;
pub(crate) mod cli;
pub mod cli_entry;
pub mod config;
pub mod core;
pub mod doctor;
pub mod mux;
pub mod pipeline;
pub mod providers;
pub mod retry;
pub mod roles;
pub mod runtime;
pub mod session_display;
pub mod session_export;
pub mod storage;
pub mod task_policy;
pub mod tiffany_events;
pub mod tiffany_install;
pub mod tui;
pub mod tui_jobs;
pub mod usage;
pub mod webhook;

pub(crate) use cli_entry::{
    Cmd, ConfigCmd, DoctorFormat, EventsFormat, JobsCmd, NativeHistoryFormat, ProviderConfigCmd,
    RolesCmd, SessionExportFormatArg, SessionsCmd, ThreadCmd,
};
pub use config::Config;
pub use core::types::{
    CritiqueOutput, Event, PlanOutput, ReviewOutput, Role, Session, Task, TaskStatus,
};
pub use core::worker::WorkerAdapter;
pub use pipeline::orchestrator::Orchestrator;
