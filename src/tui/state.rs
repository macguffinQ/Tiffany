use crate::pipeline::orchestrator::RunProgress;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ContextMode {
    Off,
    #[default]
    Compact,
    Full,
}

impl ContextMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Compact => "compact",
            Self::Full => "full",
        }
    }
}

#[derive(Default)]
pub(super) struct InputState {
    /// Current text in the input box.
    pub(super) buffer: String,
    /// Cursor byte offset inside buffer.
    pub(super) cursor: usize,
    /// Chat transcript, oldest first.
    pub(super) transcript: Vec<ChatMsg>,
    /// Receiver for live progress events from the running task.
    pub(super) run_rx: Option<UnboundedReceiver<RunProgress>>,
    pub(super) current_stage: String,
    pub(super) current_stage_detail: String,
    pub(super) last_event_at: Option<Instant>,
    /// Cancel flag set by /cancel before aborting the running task.
    pub(super) cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Background task handle for the active run, used by /cancel.
    pub(super) run_handle: Option<JoinHandle<()>>,
    /// Last non-command prompt, used by /retry.
    pub(super) last_prompt: Option<String>,
    /// Captured progress timeline for the current or most recent run.
    pub(super) run_events: Vec<String>,
    /// File where the current or most recent run process was captured.
    pub(super) process_capture_path: Option<PathBuf>,
    /// Optional substring filter for captured process events.
    pub(super) process_filter: Option<String>,
    /// Best final text captured from worker output for the active run.
    pub(super) run_final_output: Option<String>,
    /// Last readable worker output, used as a fallback final result.
    pub(super) run_last_worker_output: Option<String>,
    /// Last completed run result, used by /result and /copy result.
    pub(super) last_result_output: Option<String>,
    /// Last generated handoff package, used by /handoff status.
    pub(super) last_handoff_path: Option<PathBuf>,
    /// Last saved git patch checkpoint, used by /rollback last.
    pub(super) last_checkpoint_path: Option<PathBuf>,
    /// Background test process started by /tests run.
    pub(super) test_child: Option<Child>,
    /// Log file receiving output from the active or most recent test run.
    pub(super) test_log_path: Option<PathBuf>,
    /// Last known test run status.
    pub(super) last_test_status: Option<String>,
    /// Number of reviewer issue reports seen in the active run.
    pub(super) run_review_issue_count: usize,
    /// Number of worker failures seen in the active run.
    pub(super) run_worker_failure_count: usize,
    /// Output summaries already shown in the terminal for this run.
    pub(super) run_visible_output_keys: Vec<String>,
    /// Progress status lines already shown in terminal scrollback for this run.
    pub(super) run_visible_status_keys: Vec<String>,
    /// Output summaries already recorded in the process log for this run.
    pub(super) run_recorded_output_keys: Vec<String>,
    /// Captured tool/process outputs already inserted into chat history.
    pub(super) run_chat_output_keys: Vec<String>,
    /// Follow-up prompts submitted while another run is active.
    pub(super) queued_prompts: Vec<String>,
    /// Whether queued prompts should wait after the current run finishes.
    pub(super) queue_paused: bool,
    /// Whether transcript messages are rendered as folded one-line summaries.
    pub(super) history_folded: bool,
    /// Whether the chat keeps a live trace block for run progress.
    pub(super) trace_live_enabled: bool,
    /// Whether the live trace block shows a longer event window.
    pub(super) trace_expanded: bool,
    /// Transcript index of the active live trace block.
    pub(super) trace_message_index: Option<usize>,
    /// Optional role hint used to route future worker tasks.
    pub(super) agent_hint: Option<String>,
    /// Optional Claude Code subagent passed to Claude worker tasks.
    pub(super) cc_agent_hint: Option<String>,
    /// How much transcript context is injected into future task prompts.
    pub(super) context_mode: ContextMode,
    /// Transcript index before which messages are ignored for context memory.
    pub(super) context_cutoff: usize,
    /// Rolling deterministic summary of older remembered messages.
    pub(super) context_summary_text: String,
    /// Transcript index up to which the rolling summary has absorbed messages.
    pub(super) context_summary_upto: usize,
    /// Number of remembered messages injected into the most recent task prompt.
    pub(super) last_context_messages: usize,
    /// Character count injected into the most recent task prompt.
    pub(super) last_context_chars: usize,
    /// Prompt history for interactive reuse.
    pub(super) input_history: Vec<String>,
    /// Cursor into prompt history when browsing with Up/Down.
    pub(super) history_index: Option<usize>,
    /// Draft text preserved while browsing prompt history.
    pub(super) history_draft: Option<String>,
    /// Highlighted item in the transient slash-command completion menu.
    pub(super) slash_completion_index: usize,
    /// Whether the slash completion menu was dismissed for the current buffer.
    pub(super) slash_completion_dismissed: bool,
}

#[derive(Clone)]
pub(super) struct ChatMsg {
    pub(super) role: String,
    pub(super) content: String,
    pub(super) ts: std::time::SystemTime,
    pub(super) status: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct TuiRuntimeConfig {
    pub(super) config_path: Option<PathBuf>,
}
