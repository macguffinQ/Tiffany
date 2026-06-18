#[cfg(feature = "runtime")]
mod runtime;
#[cfg(feature = "runtime")]
mod service;
#[cfg(not(feature = "runtime"))]
mod service_stub;

pub use codex_code_mode_protocol::*;
#[cfg(feature = "runtime")]
pub use service::CodeModeService;
#[cfg(feature = "runtime")]
pub use service::InProcessCodeModeSessionProvider;
#[cfg(feature = "runtime")]
pub use service::NoopCodeModeSessionDelegate;
#[cfg(not(feature = "runtime"))]
pub use service_stub::CodeModeService;
#[cfg(not(feature = "runtime"))]
pub use service_stub::InProcessCodeModeSessionProvider;
#[cfg(not(feature = "runtime"))]
pub use service_stub::NoopCodeModeSessionDelegate;
