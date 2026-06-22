//! The main pipeline: Planner → Critic → Router → Workers → Reviewer.
//!
//! Tasks form a DAG; we run ready tasks in parallel (subject to per-adapter
//! concurrency caps) and only proceed when dependencies are satisfied.

use crate::agent_events;
use crate::core::session_store::SessionStore;
use crate::core::types::{Event, Role, Session, Task, TaskStatus};
use crate::core::worker::WorkerAdapter;
use crate::roles::critic::Critic;
use crate::roles::planner::Planner;
use crate::roles::reviewer::Reviewer;
use crate::roles::router::CapabilityRouter;
use crate::task_policy::{apply_conversation_policy, should_skip_review_for_task};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;
use uuid::Uuid;

/// Progress events emitted by the orchestrator for live terminal chat display.
/// (Borrowed from Claude Code's terminal pattern: background task + mpsc channel.)
#[derive(Clone, Debug)]
pub enum RunProgress {
    Planning,
    Planned {
        sub_task_count: usize,
    },
    Critiquing {
        round: u32,
    },
    CritiqueResult {
        approved: bool,
        issues: usize,
    },
    Replanning {
        attempt: u32,
    },
    Executing {
        sub_task_count: usize,
    },
    WorkerStarted {
        task_id: Uuid,
        agent: String,
        role: String,
        runtime: String,
        cc_agent: Option<String>,
        model: String,
        provider: Option<String>,
        prompt: String,
    },
    WorkerOutput {
        task_id: Uuid,
        agent: String,
        role: String,
        content: String,
    },
    RoleOutput {
        role: String,
        content: String,
    },
    WorkerDone {
        task_id: Uuid,
        agent: String,
        role: String,
        duration_ms: u64,
        ok: bool,
    },
    Reviewing {
        task_id: Uuid,
    },
    ReviewSkipped {
        task_id: Uuid,
        reason: String,
    },
    ReviewResult {
        task_id: Uuid,
        approved: bool,
        issues: usize,
    },
    ReviewUnavailable {
        task_id: Uuid,
        message: String,
    },
    Done {
        task_count: usize,
    },
    Failed(String),
}

pub struct Orchestrator {
    pub planner: Arc<dyn Planner>,
    pub critic: Arc<dyn Critic>,
    pub reviewer: Arc<dyn Reviewer>,
    pub router: Arc<CapabilityRouter>,
    pub adapters: Arc<HashMap<String, Arc<dyn WorkerAdapter>>>,
    pub session_store: Arc<SessionStore>,
    pub max_replan: u32,
    pub enable_critic: bool,
    pub enable_reviewer: bool,
}

impl Orchestrator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        planner: Arc<dyn Planner>,
        critic: Arc<dyn Critic>,
        reviewer: Arc<dyn Reviewer>,
        router: Arc<CapabilityRouter>,
        adapters: HashMap<String, Arc<dyn WorkerAdapter>>,
        session_store: Arc<SessionStore>,
        max_replan: u32,
        enable_critic: bool,
        enable_reviewer: bool,
    ) -> Self {
        Self {
            planner,
            critic,
            reviewer,
            router,
            adapters: Arc::new(adapters),
            session_store,
            max_replan,
            enable_critic,
            enable_reviewer,
        }
    }

    pub async fn run(&self, top_task: Task) -> Result<Vec<Task>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.run_with_progress(top_task, tx).await
    }

    /// Like `run`, but pushes progress events to `tx` for live terminal chat display.
    /// The terminal UI should drain `tx` on every render to keep input responsive.
    pub async fn run_with_progress(
        &self,
        top_task: Task,
        tx: UnboundedSender<RunProgress>,
    ) -> Result<Vec<Task>> {
        let mut orchestration_session =
            Session::new(top_task.id, "orchestrator", Role::Orchestrator);
        orchestration_session.model = "pipeline".to_string();
        orchestration_session.parent_session_ids = top_task.parent_session_ids.clone();
        self.session_store
            .finalize(&orchestration_session)
            .context("starting orchestration session")?;
        self.session_store
            .append(&Event {
                session_id: orchestration_session.id,
                task_id: top_task.id,
                ts: chrono::Utc::now(),
                kind: "user".into(),
                payload: serde_json::json!({
                    "message": top_task.prompt.clone(),
                    "tags": top_task.tags.clone(),
                    "agent_hint": top_task.agent_hint.clone(),
                    "cc_agent_hint": top_task.cc_agent_hint.clone(),
                    "model_hint": top_task.model_hint.clone(),
                    "model_provider_hint": top_task.model_provider_hint.clone(),
                }),
            })
            .context("recording orchestration request")?;
        let (record_tx, record_rx) = tokio::sync::mpsc::unbounded_channel();
        let recorder = spawn_progress_recorder(
            self.session_store.clone(),
            orchestration_session.id,
            top_task.id,
            record_rx,
            tx,
        );

        // Wrap body in async block so we can log any error before returning.
        // This is the safety net — without it, an `Err` from critique / replan
        // / reviewer silently exits and terminal chat is left spinning forever.
        let result: Result<Vec<Task>> = async {
            self.run_inner(&top_task, record_tx.clone(), Some(orchestration_session.id))
                .await
        }
        .await;
        if let Err(ref e) = result {
            tracing::error!(
                "Pipeline exiting with error (was at some stage, terminal chat may be stuck): {:#}",
                e
            );
            let _ = record_tx.send(RunProgress::Failed(format!("{:#}", e)));
        }
        drop(record_tx);
        if let Err(err) = recorder.await {
            tracing::warn!("orchestration progress recorder task failed: {err}");
        }
        orchestration_session.ended_at = Some(chrono::Utc::now());
        self.session_store
            .finalize(&orchestration_session)
            .with_context(|| {
                format!(
                    "finalizing orchestration session {}",
                    orchestration_session.id
                )
            })?;
        result
    }

    /// Internal pipeline body. Returns Err on any LLM/network/parse failure.
    /// Errors propagate out of `run_with_progress` which logs them.
    async fn run_inner(
        &self,
        top_task: &Task,
        tx: UnboundedSender<RunProgress>,
        orchestration_session_id: Option<Uuid>,
    ) -> Result<Vec<Task>> {
        // 1. Plan
        let _ = tx.send(RunProgress::Planning);
        tracing::info!("planning task: {}", top_task.prompt);
        let plan_start = std::time::Instant::now();
        let mut plan = self
            .planner
            .plan_with_progress(top_task, Some(tx.clone()))
            .await
            .context("planning task")?;
        apply_conversation_policy(top_task, &mut plan);
        apply_top_task_agent_hint(top_task, &mut plan.sub_tasks);
        attach_parent_session(orchestration_session_id, &mut plan.sub_tasks);
        tracing::info!(
            "→ Planning done: {} sub-tasks in {:?}",
            plan.sub_tasks.len(),
            plan_start.elapsed()
        );
        let _ = tx.send(RunProgress::Planned {
            sub_task_count: plan.sub_tasks.len(),
        });

        // 2. Critique loop (before consuming plan.sub_tasks)
        if self.enable_critic {
            for i in 0..self.max_replan {
                let _ = tx.send(RunProgress::Critiquing { round: i + 1 });
                tracing::info!("→ Critiquing started (round {})", i + 1);
                let crit_start = std::time::Instant::now();
                let crit = self
                    .critic
                    .critique_with_progress(top_task, &plan, Some(tx.clone()))
                    .await
                    .with_context(|| format!("critiquing plan round {}", i + 1))?;
                tracing::info!(
                    "→ Critiquing done (round {}): approved={}, {} issues in {:?}",
                    i + 1,
                    crit.approved,
                    crit.issues.len(),
                    crit_start.elapsed()
                );
                let _ = tx.send(RunProgress::CritiqueResult {
                    approved: crit.approved,
                    issues: crit.issues.len(),
                });
                if crit.approved {
                    break;
                }
                tracing::info!(
                    "critic rejected (round {}): {} issues",
                    i + 1,
                    crit.issues.len()
                );
                let _ = tx.send(RunProgress::Replanning { attempt: i + 1 });
                tracing::info!("→ Replanning started (attempt {})", i + 1);
                let replan_start = std::time::Instant::now();
                let replanned = self
                    .planner
                    .replan_with_progress(top_task, &crit, Some(tx.clone()))
                    .await;
                plan = match replanned {
                    Ok(new_plan) => new_plan,
                    Err(err) => {
                        tracing::warn!(
                            "replanning after critique round {} failed; continuing with previous plan: {:#}",
                            i + 1,
                            err
                        );
                        let _ = tx.send(RunProgress::RoleOutput {
                            role: "planner".to_string(),
                            content: format!(
                                "replan failed; continuing with previous plan: {}",
                                format!("{:#}", err)
                                    .lines()
                                    .next()
                                    .unwrap_or("unknown replan error")
                            ),
                        });
                        break;
                    }
                };
                apply_conversation_policy(top_task, &mut plan);
                apply_top_task_agent_hint(top_task, &mut plan.sub_tasks);
                attach_parent_session(orchestration_session_id, &mut plan.sub_tasks);
                tracing::info!(
                    "→ Replanning done (attempt {}): {} sub-tasks in {:?}",
                    i + 1,
                    plan.sub_tasks.len(),
                    replan_start.elapsed()
                );
            }
        }

        // 3. Execute DAG
        // Do NOT pre-mark tasks as Running here — execute_dag's filter
        // looks for Pending tasks and sets Running per spawn. Pre-marking
        // would cause the filter to skip every task (DAG exits instantly
        // with 0 executed).
        let tasks = plan.sub_tasks;
        let _ = tx.send(RunProgress::Executing {
            sub_task_count: tasks.len(),
        });
        tracing::info!("→ Executing DAG: {} sub-tasks", tasks.len());
        let exec_start = std::time::Instant::now();
        let completed = self
            .execute_dag(tasks, tx.clone())
            .await
            .context("executing task DAG")?;
        tracing::info!("→ DAG execution done in {:?}", exec_start.elapsed());

        // 4. Review
        if self.enable_reviewer {
            for t in &completed {
                if t.status != TaskStatus::Completed {
                    continue;
                }
                if should_skip_review_for_task(top_task, t) {
                    let _ = tx.send(RunProgress::ReviewSkipped {
                        task_id: t.id,
                        reason: "conversational answer".to_string(),
                    });
                    tracing::info!(
                        "→ Review skipped for task {}: conversational answer",
                        &t.id.to_string()[..8]
                    );
                    continue;
                }
                let _ = tx.send(RunProgress::Reviewing { task_id: t.id });
                tracing::info!("→ Reviewing task {}", &t.id.to_string()[..8]);
                // Look up the session for this task to get log + worktree paths
                let session = self
                    .session_store
                    .list(1000)
                    .ok()
                    .and_then(|sessions| sessions.into_iter().find(|s| s.task_id == t.id));
                let ctx = session
                    .map(|s| {
                        let worktree = t.worktree.clone().unwrap_or_else(|| {
                            self.session_store
                                .log_dir()
                                .parent()
                                .unwrap_or_else(|| std::path::Path::new("."))
                                .join("worktrees")
                                .join(t.id.to_string())
                        });
                        crate::core::types::ReviewContext {
                            session_log_path: self.session_store.log_path(s.id),
                            worktree_path: worktree,
                        }
                    })
                    .unwrap_or_else(|| crate::core::types::ReviewContext {
                        session_log_path: self.session_store.log_path(uuid::Uuid::new_v4()),
                        worktree_path: t
                            .worktree
                            .clone()
                            .unwrap_or_else(|| std::path::PathBuf::from(".")),
                    });
                let rev_start = std::time::Instant::now();
                let rev = match self
                    .reviewer
                    .review_with_progress(t, &ctx, Some(tx.clone()))
                    .await
                    .with_context(|| format!("reviewing task {}", t.id))
                {
                    Ok(rev) => rev,
                    Err(err) => {
                        let message = format!("{:#}", err);
                        tracing::warn!(
                            "reviewer failed for task {}; continuing with completed worker output: {}",
                            t.id,
                            message
                        );
                        let _ = tx.send(RunProgress::ReviewUnavailable {
                            task_id: t.id,
                            message: first_error_line(&message),
                        });
                        continue;
                    }
                };
                tracing::info!(
                    "→ Reviewing done task {}: approved={}, {} issues in {:?}",
                    &t.id.to_string()[..8],
                    rev.approved,
                    rev.issues.len(),
                    rev_start.elapsed()
                );
                let _ = tx.send(RunProgress::ReviewResult {
                    task_id: t.id,
                    approved: rev.approved,
                    issues: rev.issues.len(),
                });
                if !rev.approved {
                    tracing::warn!("reviewer rejected task {}: {:?}", t.id, rev.issues);
                } else {
                    tracing::info!("reviewer approved task {}", t.id);
                }
            }
        }

        let _ = tx.send(RunProgress::Done {
            task_count: completed.len(),
        });
        tracing::info!("→ Pipeline done: {} tasks", completed.len());
        Ok(completed)
    }

    async fn execute_dag(
        &self,
        mut tasks: Vec<Task>,
        tx: UnboundedSender<RunProgress>,
    ) -> Result<Vec<Task>> {
        let total = tasks.len();
        let by_id: HashMap<Uuid, Task> = tasks.iter().map(|t| (t.id, t.clone())).collect();
        let mut completed_ids: HashSet<Uuid> = HashSet::new();
        let mut results: Vec<Task> = Vec::new();
        let mut joinset: JoinSet<(Uuid, String, String, u64, Result<Session>)> = JoinSet::new();

        loop {
            // Find ready tasks: status==Pending (only — NOT Running) and all
            // deps done. Including Running caused re-spawn: a task already in
            // flight (e.g. worktree created, worker running) would be
            // spawned AGAIN on the next loop iteration, and the second
            // `git worktree add` would fail with "already exists".
            let ready: Vec<Task> = tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Pending)
                .filter(|t| t.deps.iter().all(|d| completed_ids.contains(d)))
                .cloned()
                .collect();

            for mut t in ready {
                t.status = TaskStatus::Running;
                // Update the in-list task to Running
                if let Some(slot) = tasks.iter_mut().find(|x| x.id == t.id) {
                    slot.status = TaskStatus::Running;
                }
                let assignment = self.router.resolve(&t)?;
                if t.model_hint.is_none() {
                    t.model_hint = Some(assignment.model.clone());
                }
                if t.model_provider_hint.is_none() {
                    t.model_provider_hint = assignment.provider.clone();
                }
                let adapter = self
                    .adapters
                    .get(&assignment.runtime)
                    .ok_or_else(|| {
                        anyhow::anyhow!("no adapter for runtime '{}'", assignment.runtime)
                    })?
                    .clone();
                let task_id = t.id;
                let agent = adapter.name().to_string();
                let worker_role = assignment.role.clone();
                let worker_start = std::time::Instant::now();
                let _ = tx.send(RunProgress::WorkerStarted {
                    task_id,
                    agent: agent.clone(),
                    role: worker_role.clone(),
                    runtime: assignment.runtime.clone(),
                    cc_agent: t.cc_agent_hint.clone(),
                    model: assignment.model.clone(),
                    provider: assignment.provider.clone(),
                    prompt: t.prompt.clone(),
                });
                let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
                let progress_tx = tx.clone();
                let output_agent = agent.clone();
                let output_role = worker_role.clone();
                let forwarder = tokio::spawn(async move {
                    let mut last_output_key: Option<String> = None;
                    while let Some(event) = event_rx.recv().await {
                        if event.kind == "heartbeat" {
                            continue;
                        }
                        let content = format_worker_event(&output_agent, &event);
                        if agent_events::is_low_value_output(&content) {
                            continue;
                        }
                        if let Some(key) = agent_events::normalized_output_key(&content, 240_000) {
                            if last_output_key.as_deref() == Some(key.as_str()) {
                                continue;
                            }
                            last_output_key = Some(key);
                        }
                        let _ = progress_tx.send(RunProgress::WorkerOutput {
                            task_id,
                            agent: output_agent.clone(),
                            role: output_role.clone(),
                            content,
                        });
                    }
                });
                joinset.spawn(async move {
                    let res = adapter.start(&t, Some(event_tx)).await;
                    let _ = forwarder.await;
                    (
                        task_id,
                        agent,
                        worker_role,
                        duration_ms(worker_start.elapsed()),
                        res.map(|h| h.session),
                    )
                });
            }

            if joinset.is_empty() {
                if completed_ids.len() < total {
                    let remaining = tasks
                        .iter()
                        .filter(|t| !completed_ids.contains(&t.id))
                        .count();
                    anyhow::bail!(
                        "task DAG stalled: {} task(s) remain with unsatisfied dependencies",
                        remaining
                    );
                }
                break;
            }

            if let Some(joined) = joinset.join_next().await {
                let (task_id, agent, role, duration_ms, res) = joined?;
                match res {
                    Ok(mut session) => {
                        let done_agent = if session.agent.trim().is_empty() {
                            agent
                        } else {
                            session.agent.clone()
                        };
                        if let Some(task) = by_id.get(&task_id) {
                            for parent_id in &task.parent_session_ids {
                                if !session.parent_session_ids.contains(parent_id) {
                                    session.parent_session_ids.push(*parent_id);
                                }
                            }
                        }
                        session.ended_at = Some(chrono::Utc::now());
                        self.session_store
                            .finalize(&session)
                            .with_context(|| format!("finalizing worker session {}", session.id))?;

                        // Mark the task complete.
                        if let Some(slot) = tasks.iter_mut().find(|t| t.id == task_id) {
                            slot.status = TaskStatus::Completed;
                        }
                        completed_ids.insert(task_id);
                        if let Some(t) = by_id.get(&task_id).cloned() {
                            let mut t = t;
                            t.status = TaskStatus::Completed;
                            results.push(t);
                        }
                        let _ = tx.send(RunProgress::WorkerDone {
                            task_id,
                            agent: done_agent,
                            role,
                            duration_ms,
                            ok: true,
                        });
                    }
                    Err(e) => {
                        let error_message = format!("{:#}", e);
                        tracing::error!("task {} failed: {}", task_id, error_message);
                        if let Some(slot) = tasks.iter_mut().find(|t| t.id == task_id) {
                            slot.status = TaskStatus::Failed;
                            slot.error = Some(error_message.clone());
                        }
                        completed_ids.insert(task_id);
                        let _ = tx.send(RunProgress::WorkerOutput {
                            task_id,
                            agent: agent.clone(),
                            role: role.clone(),
                            content: format!("{} error: {}", agent, error_message),
                        });
                        let _ = tx.send(RunProgress::WorkerDone {
                            task_id,
                            agent,
                            role,
                            duration_ms,
                            ok: false,
                        });
                    }
                }
            }
        }

        tracing::info!("executed {} / {} tasks", results.len(), total);
        Ok(results)
    }
}

fn duration_ms(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn first_error_line(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("reviewer unavailable")
        .to_string()
}

fn spawn_progress_recorder(
    store: Arc<SessionStore>,
    session_id: Uuid,
    top_task_id: Uuid,
    mut rx: UnboundedReceiver<RunProgress>,
    tx: UnboundedSender<RunProgress>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let stored_event = run_progress_to_event(session_id, top_task_id, &event);
            if let Err(err) = store.append(&stored_event) {
                tracing::warn!(
                    "failed to record orchestration event {} for session {}: {err:#}",
                    stored_event.kind,
                    session_id
                );
            }
            let _ = tx.send(event);
        }
    })
}

fn run_progress_to_event(session_id: Uuid, top_task_id: Uuid, event: &RunProgress) -> Event {
    let (kind, task_id, payload) = match event {
        RunProgress::Planning => (
            "planner",
            top_task_id,
            serde_json::json!({
                "status": "running",
                "message": "planning",
            }),
        ),
        RunProgress::Planned { sub_task_count } => (
            "planner",
            top_task_id,
            serde_json::json!({
                "status": "done",
                "message": format!("plan ready - {sub_task_count} sub-task(s)"),
                "count": sub_task_count,
            }),
        ),
        RunProgress::Critiquing { round } => (
            "critic",
            top_task_id,
            serde_json::json!({
                "status": "running",
                "message": format!("checking plan - round {round}"),
                "round": round,
            }),
        ),
        RunProgress::CritiqueResult { approved, issues } => (
            "critic",
            top_task_id,
            serde_json::json!({
                "status": if *approved { "done" } else { "warning" },
                "message": if *approved {
                    "plan approved".to_string()
                } else {
                    format!("plan needs fixes - {issues} issue(s)")
                },
                "approved": approved,
                "issues": issues,
            }),
        ),
        RunProgress::Replanning { attempt } => (
            "planner",
            top_task_id,
            serde_json::json!({
                "status": "running",
                "message": format!("replanning - attempt {attempt}"),
                "attempt": attempt,
            }),
        ),
        RunProgress::Executing { sub_task_count } => (
            "worker",
            top_task_id,
            serde_json::json!({
                "status": "running",
                "message": format!("running {sub_task_count} sub-task(s)"),
                "count": sub_task_count,
            }),
        ),
        RunProgress::WorkerStarted {
            task_id,
            agent,
            role,
            runtime,
            cc_agent,
            model,
            provider,
            prompt,
        } => (
            "worker",
            *task_id,
            serde_json::json!({
                "status": "running",
                "message": format!("{role} started"),
                "task_id": task_id,
                "agent": agent,
                "worker_role": role,
                "runtime": runtime,
                "cc_agent": cc_agent,
                "model": model,
                "provider": provider,
                "task_prompt": prompt,
            }),
        ),
        RunProgress::WorkerOutput {
            task_id,
            agent,
            role,
            content,
        } => (
            "worker",
            *task_id,
            serde_json::json!({
                "status": "output",
                "message": format!("{role} output"),
                "task_id": task_id,
                "agent": agent,
                "worker_role": role,
                "content": content,
            }),
        ),
        RunProgress::RoleOutput { role, content } => (
            role.as_str(),
            top_task_id,
            serde_json::json!({
                "status": "output",
                "message": format!("{role} output"),
                "content": content,
            }),
        ),
        RunProgress::WorkerDone {
            task_id,
            agent,
            role,
            duration_ms,
            ok,
        } => (
            "worker",
            *task_id,
            serde_json::json!({
                "status": if *ok { "done" } else { "failed" },
                "message": if *ok {
                    format!("{role} done")
                } else {
                    format!("{role} failed")
                },
                "task_id": task_id,
                "agent": agent,
                "worker_role": role,
                "duration_ms": duration_ms,
                "ok": ok,
            }),
        ),
        RunProgress::Reviewing { task_id } => (
            "reviewer",
            *task_id,
            serde_json::json!({
                "status": "running",
                "message": "reviewing",
                "task_id": task_id,
            }),
        ),
        RunProgress::ReviewSkipped { task_id, reason } => (
            "reviewer",
            *task_id,
            serde_json::json!({
                "status": "skipped",
                "message": format!("review skipped - {reason}"),
                "task_id": task_id,
                "reason": reason,
            }),
        ),
        RunProgress::ReviewResult {
            task_id,
            approved,
            issues,
        } => (
            "reviewer",
            *task_id,
            serde_json::json!({
                "status": if *approved { "done" } else { "warning" },
                "message": if *approved {
                    "review approved".to_string()
                } else {
                    format!("review needs fixes - {issues} issue(s)")
                },
                "task_id": task_id,
                "approved": approved,
                "issues": issues,
            }),
        ),
        RunProgress::ReviewUnavailable { task_id, message } => (
            "reviewer",
            *task_id,
            serde_json::json!({
                "status": "warning",
                "message": format!("review unavailable - {message}"),
                "task_id": task_id,
                "reason": message,
            }),
        ),
        RunProgress::Done { task_count } => (
            "orchestrator",
            top_task_id,
            serde_json::json!({
                "status": "done",
                "message": format!("done - {task_count} sub-task(s)"),
                "count": task_count,
            }),
        ),
        RunProgress::Failed(message) => (
            "orchestrator",
            top_task_id,
            serde_json::json!({
                "status": "failed",
                "message": message,
            }),
        ),
    };

    Event {
        session_id,
        task_id,
        ts: chrono::Utc::now(),
        kind: kind.to_string(),
        payload,
    }
}

fn apply_top_task_agent_hint(top_task: &Task, sub_tasks: &mut [Task]) {
    for task in sub_tasks {
        if task.agent_hint.is_none() {
            if let Some(agent_hint) = &top_task.agent_hint {
                task.agent_hint = Some(agent_hint.clone());
            }
        }
        if task.cc_agent_hint.is_none() {
            if let Some(cc_agent_hint) = &top_task.cc_agent_hint {
                task.cc_agent_hint = Some(cc_agent_hint.clone());
            }
        }
        if task.worktree.is_none() {
            if let Some(worktree) = &top_task.worktree {
                task.worktree = Some(worktree.clone());
            }
        }
    }
}

fn attach_parent_session(parent_session_id: Option<Uuid>, sub_tasks: &mut [Task]) {
    let Some(parent_session_id) = parent_session_id else {
        return;
    };
    for task in sub_tasks {
        if !task.parent_session_ids.contains(&parent_session_id) {
            task.parent_session_ids.push(parent_session_id);
        }
    }
}

fn format_worker_event(agent: &str, event: &Event) -> String {
    agent_events::format_runtime_output(agent, &event.kind, &event.payload, 240_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoleConfig;
    use crate::core::types::{
        CritiqueOutput, Event, PlanOutput, ReviewContext, ReviewOutput, Role, Session,
    };
    use crate::core::worker::WorkerHandle;
    use futures::stream::{self, BoxStream};

    #[test]
    fn top_task_agent_hint_is_applied_to_unhinted_subtasks() {
        let mut top = Task::new("top");
        top.agent_hint = Some("worker-cc".into());
        top.cc_agent_hint = Some("reviewer".into());
        let mut sub_tasks = vec![Task::new("a"), Task::new("b")];
        sub_tasks[1].agent_hint = Some("custom".into());
        sub_tasks[1].cc_agent_hint = Some("executor".into());

        apply_top_task_agent_hint(&top, &mut sub_tasks);

        assert_eq!(sub_tasks[0].agent_hint.as_deref(), Some("worker-cc"));
        assert_eq!(sub_tasks[1].agent_hint.as_deref(), Some("custom"));
        assert_eq!(sub_tasks[0].cc_agent_hint.as_deref(), Some("reviewer"));
        assert_eq!(sub_tasks[1].cc_agent_hint.as_deref(), Some("executor"));
    }

    #[test]
    fn top_task_worktree_is_applied_to_unhinted_subtasks() {
        let mut top = Task::new("top");
        top.worktree = Some(std::path::PathBuf::from("/tmp/project"));
        let mut sub_tasks = vec![Task::new("a"), Task::new("b")];
        sub_tasks[1].worktree = Some(std::path::PathBuf::from("/tmp/other"));

        apply_top_task_agent_hint(&top, &mut sub_tasks);

        assert_eq!(
            sub_tasks[0].worktree.as_deref(),
            Some(std::path::Path::new("/tmp/project"))
        );
        assert_eq!(
            sub_tasks[1].worktree.as_deref(),
            Some(std::path::Path::new("/tmp/other"))
        );
    }

    #[test]
    fn orchestration_parent_is_attached_to_subtasks_once() {
        let parent_id = Uuid::new_v4();
        let mut task = Task::new("sub");
        task.parent_session_ids.push(parent_id);
        let mut sub_tasks = vec![task, Task::new("other")];

        attach_parent_session(Some(parent_id), &mut sub_tasks);

        assert_eq!(
            sub_tasks[0]
                .parent_session_ids
                .iter()
                .filter(|id| **id == parent_id)
                .count(),
            1
        );
        assert_eq!(sub_tasks[1].parent_session_ids, vec![parent_id]);
    }

    struct StaticPlanner;

    #[async_trait::async_trait]
    impl Planner for StaticPlanner {
        async fn plan(&self, _top_task: &Task) -> Result<PlanOutput> {
            Ok(PlanOutput {
                sub_tasks: vec![Task::new("sub-task")],
                rationale: "test plan".to_string(),
                estimated_cost_usd: 0.0,
            })
        }

        async fn replan(&self, top_task: &Task, _critique: &CritiqueOutput) -> Result<PlanOutput> {
            self.plan(top_task).await
        }
    }

    struct FailingReplanPlanner;

    #[async_trait::async_trait]
    impl Planner for FailingReplanPlanner {
        async fn plan(&self, _top_task: &Task) -> Result<PlanOutput> {
            Ok(PlanOutput {
                sub_tasks: vec![Task::new("original sub-task")],
                rationale: "original plan".to_string(),
                estimated_cost_usd: 0.0,
            })
        }

        async fn replan(&self, _top_task: &Task, _critique: &CritiqueOutput) -> Result<PlanOutput> {
            anyhow::bail!("planner returned no sub_tasks (parse failed)")
        }
    }

    struct ApprovingCritic;

    #[async_trait::async_trait]
    impl Critic for ApprovingCritic {
        async fn critique(&self, _top_task: &Task, _plan: &PlanOutput) -> Result<CritiqueOutput> {
            Ok(CritiqueOutput {
                approved: true,
                issues: vec![],
                suggestions: vec![],
            })
        }
    }

    struct RejectingCritic;

    #[async_trait::async_trait]
    impl Critic for RejectingCritic {
        async fn critique(&self, _top_task: &Task, _plan: &PlanOutput) -> Result<CritiqueOutput> {
            Ok(CritiqueOutput {
                approved: false,
                issues: vec!["needs a clearer worker prompt".to_string()],
                suggestions: vec!["make the prompt direct".to_string()],
            })
        }
    }

    struct FailingCritic;

    #[async_trait::async_trait]
    impl Critic for FailingCritic {
        async fn critique(&self, _top_task: &Task, _plan: &PlanOutput) -> Result<CritiqueOutput> {
            anyhow::bail!("critic unavailable")
        }
    }

    struct ApprovingReviewer;

    #[async_trait::async_trait]
    impl Reviewer for ApprovingReviewer {
        async fn review(&self, _task: &Task, _ctx: &ReviewContext) -> Result<ReviewOutput> {
            Ok(ReviewOutput {
                approved: true,
                issues: vec![],
            })
        }
    }

    struct FailingReviewer;

    #[async_trait::async_trait]
    impl Reviewer for FailingReviewer {
        async fn review(&self, _task: &Task, _ctx: &ReviewContext) -> Result<ReviewOutput> {
            anyhow::bail!("reviewer unavailable")
        }
    }

    struct CompletingAdapter;

    #[async_trait::async_trait]
    impl WorkerAdapter for CompletingAdapter {
        fn name(&self) -> &str {
            "test-worker"
        }

        async fn start(
            &self,
            task: &Task,
            event_tx: Option<tokio::sync::mpsc::UnboundedSender<Event>>,
        ) -> Result<WorkerHandle> {
            if let Some(tx) = event_tx {
                let _ = tx.send(Event {
                    session_id: Uuid::new_v4(),
                    task_id: task.id,
                    ts: chrono::Utc::now(),
                    kind: "assistant".into(),
                    payload: serde_json::json!({
                        "message": {
                            "content": [
                                { "type": "text", "text": "worker is making progress" }
                            ]
                        }
                    }),
                });
            }
            let mut session = Session::new(task.id, self.name(), Role::Worker);
            session.model = task.model_hint.clone().unwrap_or_default();
            Ok(WorkerHandle {
                session,
                kill: Arc::new(|| {}),
            })
        }

        fn stream_events(&self, _session: &Session) -> BoxStream<'static, Result<Event>> {
            Box::pin(stream::empty())
        }

        async fn cancel(&self, _session: &Session) -> Result<()> {
            Ok(())
        }

        async fn get_diff(&self, _session: &Session) -> Result<String> {
            Ok(String::new())
        }

        async fn build_context(
            &self,
            _reader: &dyn crate::core::session_store::SessionReader,
            _parent_ids: &[Uuid],
        ) -> Result<String> {
            Ok(String::new())
        }
    }

    fn test_orchestrator(critic: Arc<dyn Critic>) -> (tempfile::TempDir, Orchestrator) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("sessions"), &tmp.path().join("state.db"))
                .expect("session store"),
        );

        let mut roles = std::collections::HashMap::new();
        roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "test-model".to_string(),
                runtime: "test-runtime".to_string(),
                agent_teams: false,
            },
        );

        let orch = Orchestrator::new(
            Arc::new(StaticPlanner),
            critic,
            Arc::new(ApprovingReviewer),
            Arc::new(CapabilityRouter::new(&roles, &[])),
            std::collections::HashMap::new(),
            store,
            1,
            true,
            false,
        );

        (tmp, orch)
    }

    #[tokio::test]
    async fn run_with_progress_reports_stage_errors() {
        let (_tmp, orch) = test_orchestrator(Arc::new(FailingCritic));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let err = orch
            .run_with_progress(Task::new("top task"), tx)
            .await
            .expect_err("critic failure");

        let err_msg = format!("{:#}", err);
        assert!(err_msg.contains("critiquing plan round 1"));
        assert!(err_msg.contains("critic unavailable"));

        let mut saw_failed = false;
        while let Ok(event) = rx.try_recv() {
            if let RunProgress::Failed(msg) = event {
                saw_failed =
                    msg.contains("critiquing plan round 1") && msg.contains("critic unavailable");
            }
        }
        assert!(saw_failed, "expected Failed progress event");
    }

    #[tokio::test]
    async fn run_with_progress_continues_when_replan_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("sessions"), &tmp.path().join("state.db"))
                .expect("session store"),
        );
        let mut roles = std::collections::HashMap::new();
        roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "test-model".to_string(),
                runtime: "test-runtime".to_string(),
                agent_teams: false,
            },
        );
        let mut adapters: std::collections::HashMap<String, Arc<dyn WorkerAdapter>> =
            std::collections::HashMap::new();
        adapters.insert("test-runtime".to_string(), Arc::new(CompletingAdapter));
        let orch = Orchestrator::new(
            Arc::new(FailingReplanPlanner),
            Arc::new(RejectingCritic),
            Arc::new(ApprovingReviewer),
            Arc::new(CapabilityRouter::new(&roles, &[])),
            adapters,
            store,
            1,
            true,
            false,
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let completed = orch
            .run_with_progress(Task::new("top task"), tx)
            .await
            .expect("failed replan should not abort execution");

        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].prompt, "original sub-task");
        let mut saw_replan_warning = false;
        while let Ok(event) = rx.try_recv() {
            if let RunProgress::RoleOutput { role, content } = event {
                saw_replan_warning = role == "planner"
                    && content.contains("replan failed; continuing with previous plan");
            }
        }
        assert!(saw_replan_warning, "expected visible replan fallback event");
    }

    #[tokio::test]
    async fn run_with_progress_persists_orchestration_session_and_links_worker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("sessions"), &tmp.path().join("state.db"))
                .expect("session store"),
        );
        let mut roles = std::collections::HashMap::new();
        roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "test-model".to_string(),
                runtime: "test-runtime".to_string(),
                agent_teams: false,
            },
        );
        let mut adapters: std::collections::HashMap<String, Arc<dyn WorkerAdapter>> =
            std::collections::HashMap::new();
        adapters.insert("test-runtime".to_string(), Arc::new(CompletingAdapter));
        let orch = Orchestrator::new(
            Arc::new(StaticPlanner),
            Arc::new(ApprovingCritic),
            Arc::new(ApprovingReviewer),
            Arc::new(CapabilityRouter::new(&roles, &[])),
            adapters,
            store.clone(),
            1,
            true,
            false,
        );
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let completed = orch
            .run_with_progress(Task::new("persist the full run"), tx)
            .await
            .expect("run should complete");

        assert_eq!(completed.len(), 1);
        let sessions = store.list(10).unwrap();
        let orchestration = sessions
            .iter()
            .find(|session| session.agent == "orchestrator")
            .expect("orchestration session should be finalized");
        assert_eq!(orchestration.role, Role::Orchestrator);
        assert!(orchestration.ended_at.is_some());
        let worker = sessions
            .iter()
            .find(|session| session.agent == "test-worker")
            .expect("worker session should be finalized");
        assert!(
            worker.parent_session_ids.contains(&orchestration.id),
            "worker session should link back to orchestration session"
        );

        let log = std::fs::read_to_string(store.log_path(orchestration.id)).unwrap();
        assert!(log.contains("persist the full run"));
        assert!(log.contains("\"type\":\"planner\""));
        assert!(log.contains("\"type\":\"critic\""));
        assert!(log.contains("\"type\":\"worker\""));
        assert!(log.contains("\"type\":\"orchestrator\""));
    }

    #[tokio::test]
    async fn run_with_progress_skips_reviewer_for_conversation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("sessions"), &tmp.path().join("state.db"))
                .expect("session store"),
        );
        let mut roles = std::collections::HashMap::new();
        roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "test-model".to_string(),
                runtime: "test-runtime".to_string(),
                agent_teams: false,
            },
        );
        let mut adapters: std::collections::HashMap<String, Arc<dyn WorkerAdapter>> =
            std::collections::HashMap::new();
        adapters.insert("test-runtime".to_string(), Arc::new(CompletingAdapter));
        let orch = Orchestrator::new(
            Arc::new(StaticPlanner),
            Arc::new(ApprovingCritic),
            Arc::new(FailingReviewer),
            Arc::new(CapabilityRouter::new(&roles, &[])),
            adapters,
            store,
            1,
            true,
            true,
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let completed = orch
            .run_with_progress(Task::new("你好"), tx)
            .await
            .expect("conversation should complete without reviewer");

        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].tags, vec!["chat".to_string()]);
        assert!(completed[0].prompt.contains("User message:\n你好"));

        let mut saw_review_skipped = false;
        let mut saw_review_result = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                RunProgress::ReviewSkipped { reason, .. } => {
                    saw_review_skipped = reason == "conversational answer";
                }
                RunProgress::ReviewResult { .. } => saw_review_result = true,
                _ => {}
            }
        }
        assert!(saw_review_skipped, "expected visible review skipped event");
        assert!(!saw_review_result, "reviewer should not produce a result");
    }

    #[tokio::test]
    async fn run_with_progress_continues_when_reviewer_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("sessions"), &tmp.path().join("state.db"))
                .expect("session store"),
        );
        let mut roles = std::collections::HashMap::new();
        roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "test-model".to_string(),
                runtime: "test-runtime".to_string(),
                agent_teams: false,
            },
        );
        let mut adapters: std::collections::HashMap<String, Arc<dyn WorkerAdapter>> =
            std::collections::HashMap::new();
        adapters.insert("test-runtime".to_string(), Arc::new(CompletingAdapter));
        let orch = Orchestrator::new(
            Arc::new(StaticPlanner),
            Arc::new(ApprovingCritic),
            Arc::new(FailingReviewer),
            Arc::new(CapabilityRouter::new(&roles, &[])),
            adapters,
            store,
            1,
            true,
            true,
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let completed = orch
            .run_with_progress(Task::new("write an implementation plan"), tx)
            .await
            .expect("reviewer failure should not abort completed worker output");

        assert_eq!(completed.len(), 1);
        let mut saw_review_unavailable = false;
        let mut saw_done = false;
        let mut saw_failed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                RunProgress::ReviewUnavailable { message, .. } => {
                    saw_review_unavailable = message.contains("reviewer unavailable");
                }
                RunProgress::Done { task_count } => saw_done = task_count == 1,
                RunProgress::Failed(_) => saw_failed = true,
                _ => {}
            }
        }
        assert!(
            saw_review_unavailable,
            "expected visible review unavailable event"
        );
        assert!(saw_done, "pipeline should still finish");
        assert!(!saw_failed, "reviewer failure should not be terminal");
    }

    #[tokio::test]
    async fn execute_dag_errors_when_dependencies_never_become_ready() {
        let (_tmp, orch) = test_orchestrator(Arc::new(ApprovingCritic));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut first = Task::new("first");
        let mut second = Task::new("second");
        first.deps.push(second.id);
        second.deps.push(first.id);

        let err = orch
            .execute_dag(vec![first, second], tx)
            .await
            .expect_err("cyclic DAG should not complete");

        assert!(
            format!("{:#}", err).contains("task DAG stalled"),
            "unexpected error: {:#}",
            err
        );
    }

    #[tokio::test]
    async fn execute_dag_finalizes_completed_worker_sessions() {
        let (_tmp, mut orch) = test_orchestrator(Arc::new(ApprovingCritic));
        Arc::get_mut(&mut orch.adapters)
            .expect("orchestrator should be uniquely owned")
            .insert("test-runtime".to_string(), Arc::new(CompletingAdapter));
        let task = Task::new("complete me");
        let task_id = task.id;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let completed = orch.execute_dag(vec![task], tx).await.unwrap();
        assert_eq!(completed.len(), 1);

        let sessions = orch.session_store.list(10).unwrap();
        let session = sessions
            .iter()
            .find(|s| s.task_id == task_id)
            .expect("worker session should be finalized");
        assert_eq!(session.agent, "test-worker");
        assert!(session.ended_at.is_some());
    }

    #[tokio::test]
    async fn execute_dag_passes_resolved_worker_model_to_adapter() {
        let (_tmp, mut orch) = test_orchestrator(Arc::new(ApprovingCritic));
        Arc::get_mut(&mut orch.adapters)
            .expect("orchestrator should be uniquely owned")
            .insert("test-runtime".to_string(), Arc::new(CompletingAdapter));
        let task = Task::new("use routed model");
        let task_id = task.id;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let _ = orch.execute_dag(vec![task], tx).await.unwrap();

        let sessions = orch.session_store.list(10).unwrap();
        let session = sessions
            .iter()
            .find(|s| s.task_id == task_id)
            .expect("worker session should be finalized");
        assert_eq!(session.model, "test-model");
    }

    #[tokio::test]
    async fn execute_dag_forwards_worker_output_events() {
        let (_tmp, mut orch) = test_orchestrator(Arc::new(ApprovingCritic));
        Arc::get_mut(&mut orch.adapters)
            .expect("orchestrator should be uniquely owned")
            .insert("test-runtime".to_string(), Arc::new(CompletingAdapter));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let _ = orch
            .execute_dag(vec![Task::new("stream me")], tx)
            .await
            .unwrap();

        let mut saw_worker_started = false;
        let mut saw_worker_output = false;
        let mut saw_worker_done_duration = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                RunProgress::WorkerStarted {
                    role,
                    runtime,
                    model,
                    prompt,
                    cc_agent,
                    ..
                } => {
                    saw_worker_started = role == "worker-cc"
                        && runtime == "test-runtime"
                        && model == "test-model"
                        && prompt == "stream me"
                        && cc_agent.is_none();
                }
                RunProgress::WorkerOutput { role, content, .. } => {
                    saw_worker_output = role == "worker-cc"
                        && content.contains("test-worker assistant")
                        && content.contains("worker is making progress");
                }
                RunProgress::WorkerDone {
                    role,
                    duration_ms,
                    ok,
                    ..
                } => {
                    saw_worker_done_duration = role == "worker-cc" && ok && duration_ms < 60_000;
                }
                _ => {}
            }
        }
        assert!(saw_worker_started, "expected WorkerStarted metadata event");
        assert!(saw_worker_output, "expected forwarded WorkerOutput event");
        assert!(
            saw_worker_done_duration,
            "expected WorkerDone duration metadata"
        );
    }
}
