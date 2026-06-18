use std::sync::Arc;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeNestedToolCall;
use codex_code_mode_protocol::CodeModeSession;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionProvider;
use codex_code_mode_protocol::CodeModeSessionProviderFuture;
use codex_code_mode_protocol::CodeModeSessionResultFuture;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::ExecuteToPendingOutcome;
use codex_code_mode_protocol::NotificationFuture;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::StartedCell;
use codex_code_mode_protocol::ToolInvocationFuture;
use codex_code_mode_protocol::WaitOutcome;
use codex_code_mode_protocol::WaitRequest;
use codex_code_mode_protocol::WaitToPendingOutcome;
use codex_code_mode_protocol::WaitToPendingRequest;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const UNAVAILABLE: &str = "code mode runtime is not available in this build";

pub struct NoopCodeModeSessionDelegate;

impl CodeModeSessionDelegate for NoopCodeModeSessionDelegate {
    fn invoke_tool<'a>(
        &'a self,
        _invocation: CodeModeNestedToolCall,
        cancellation_token: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            cancellation_token.cancelled().await;
            Err("code mode nested tools are unavailable".to_string())
        })
    }

    fn notify<'a>(
        &'a self,
        _call_id: String,
        _cell_id: CellId,
        _text: String,
        _cancellation_token: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn cell_closed(&self, _cell_id: &CellId) {}
}

#[derive(Default)]
pub struct InProcessCodeModeSessionProvider;

impl CodeModeSessionProvider for InProcessCodeModeSessionProvider {
    fn create_session<'a>(
        &'a self,
        _delegate: Arc<dyn CodeModeSessionDelegate>,
    ) -> CodeModeSessionProviderFuture<'a> {
        Box::pin(async { Ok(Arc::new(CodeModeService) as Arc<dyn CodeModeSession>) })
    }
}

pub struct CodeModeService;

impl CodeModeService {
    pub fn new() -> Self {
        Self
    }

    pub fn with_delegate(_delegate: Arc<dyn CodeModeSessionDelegate>) -> Self {
        Self
    }

    pub async fn execute(&self, request: ExecuteRequest) -> Result<StartedCell, String> {
        let cell_id = CellId::new(request.tool_call_id);
        let (response_tx, response_rx) = oneshot::channel();
        let _ = response_tx.send(RuntimeResponse::Result {
            cell_id: cell_id.clone(),
            content_items: Vec::new(),
            error_text: Some(UNAVAILABLE.to_string()),
        });
        Ok(StartedCell::new(cell_id, response_rx))
    }

    pub async fn execute_to_pending(
        &self,
        request: ExecuteRequest,
    ) -> Result<ExecuteToPendingOutcome, String> {
        Ok(ExecuteToPendingOutcome::Completed(unavailable_response(
            CellId::new(request.tool_call_id),
        )))
    }

    pub async fn wait(&self, request: WaitRequest) -> Result<WaitOutcome, String> {
        Ok(WaitOutcome::MissingCell(unavailable_response(
            request.cell_id,
        )))
    }

    pub async fn terminate(&self, cell_id: CellId) -> Result<WaitOutcome, String> {
        Ok(WaitOutcome::MissingCell(unavailable_response(cell_id)))
    }

    pub async fn wait_to_pending(
        &self,
        request: WaitToPendingRequest,
    ) -> Result<WaitToPendingOutcome, String> {
        Ok(WaitToPendingOutcome::MissingCell(unavailable_response(
            request.cell_id,
        )))
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Default for CodeModeService {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeModeSession for CodeModeService {
    fn is_alive(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        request: ExecuteRequest,
    ) -> CodeModeSessionResultFuture<'a, StartedCell> {
        Box::pin(CodeModeService::execute(self, request))
    }

    fn wait<'a>(&'a self, request: WaitRequest) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(CodeModeService::wait(self, request))
    }

    fn terminate<'a>(&'a self, cell_id: CellId) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(CodeModeService::terminate(self, cell_id))
    }

    fn shutdown<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
        Box::pin(CodeModeService::shutdown(self))
    }
}

fn unavailable_response(cell_id: CellId) -> RuntimeResponse {
    RuntimeResponse::Result {
        cell_id,
        content_items: Vec::new(),
        error_text: Some(UNAVAILABLE.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use codex_code_mode_protocol::ExecuteRequest;

    use super::CodeModeService;
    use super::UNAVAILABLE;

    #[tokio::test]
    async fn no_runtime_execute_returns_explicit_unavailable_result() {
        let service = CodeModeService::new();
        let started = service
            .execute(ExecuteRequest {
                tool_call_id: "call-1".to_string(),
                enabled_tools: Vec::new(),
                source: "1 + 1".to_string(),
                yield_time_ms: None,
                max_output_tokens: None,
            })
            .await
            .expect("stub starts a synthetic cell");
        let response = started
            .initial_response()
            .await
            .expect("stub response should be delivered");
        let text = serde_json::to_string(&response).expect("serialize response");
        assert!(text.contains(UNAVAILABLE));
    }
}
