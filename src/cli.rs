//! CLI dispatch.

use anyhow::{Context, Result};
use orchestrator::config::Config;
use orchestrator::core::session_store::{
    NativeConversation, NativeEvent, NativeImportReport, NativeTurn, SessionStore, TuiJob,
    WorkerThread,
};
use orchestrator::core::types::{Role, Session, Task, TaskStatus};
use orchestrator::pipeline::orchestrator::Orchestrator;
use orchestrator::roles::ab_judge::AbJudge;
use orchestrator::session_export::SessionExportFormat;
use orchestrator::tiffany_events::{TiffanyProgressEvent, TiffanyTextProgressFormatter};
use orchestrator::tiffany_install;
use orchestrator::{adapters, cc_config, mux, roles, runtime, storage};
use std::cmp::Reverse;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

const TIFFANY_NATIVE_SESSIONS_FILE: &str = "tiffany-orchestrator/native-sessions.json";
const TIFFANY_JOB_RESULT_MAX_CHARS: usize = 12_000;

pub async fn run(cmd: crate::Cmd, config_path: &Path) -> Result<()> {
    match cmd {
        crate::Cmd::Init => {
            let target = Config::init_default()?;
            println!("Wrote default config to {}", target.display());
            println!(
                "Next: run `orchestrator setup` or open `tiffany-loop` and use /provider + /role."
            );
            Ok(())
        }

        crate::Cmd::Setup => run_wizard(config_path),

        crate::Cmd::Run {
            prompt,
            tag,
            planner,
            critic,
            worker,
            agent,
            reviewer,
            ab,
            no_critic,
            no_reviewer,
            detach,
        } => {
            if detach {
                return detach_run(DetachRunRequest {
                    config_path,
                    prompt: &prompt,
                    tags: &tag,
                    planner: planner.as_deref(),
                    critic: critic.as_deref(),
                    worker: worker.as_deref(),
                    cc_agent: agent.as_deref(),
                    reviewer: reviewer.as_deref(),
                    no_critic,
                    no_reviewer,
                });
            }
            let cfg = Config::load(config_path)?;
            let orch = build_orchestrator(
                &cfg,
                no_critic,
                no_reviewer,
                planner.as_deref(),
                critic.as_deref(),
                reviewer.as_deref(),
            )
            .await?;
            if ab {
                run_ab_mode(
                    &cfg,
                    &orch,
                    prompt,
                    tag,
                    worker.as_deref(),
                    agent.as_deref(),
                )
                .await
            } else {
                run_single_mode(
                    &cfg,
                    &orch,
                    prompt,
                    tag,
                    worker.as_deref(),
                    agent.as_deref(),
                )
                .await
            }
        }

        crate::Cmd::Attach { id, tail, status } => attach_run(id.as_deref(), tail, status),

        crate::Cmd::Events {
            prompt,
            tag,
            planner,
            critic,
            worker,
            agent,
            reviewer,
            no_critic,
            no_reviewer,
            format,
        } => {
            let cfg = Config::load(config_path)?;
            let store = SessionStore::open(&cfg.behavior.session_log_dir, &cfg.behavior.db_path)?;
            let job = store
                .create_tui_job(&prompt, "running", None, worker.as_deref())
                .ok();
            let orch = build_orchestrator(
                &cfg,
                no_critic,
                no_reviewer,
                planner.as_deref(),
                critic.as_deref(),
                reviewer.as_deref(),
            )
            .await?;
            let mut task = Task::new(prompt);
            task.tags = tag;
            if let Some(w) = worker {
                task.agent_hint = runtime::normalize_agent_hint_with_roles(&w, &cfg.roles);
            }
            task.cc_agent_hint = sanitize_cc_agent_hint(agent.as_deref());

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let run = tokio::spawn(async move { orch.run_with_progress(task, tx).await });
            let mut stdout = tokio::io::BufWriter::new(tokio::io::stdout());
            let mut text_formatter = TiffanyTextProgressFormatter::new();
            let mut job_result = TuiJobResultCapture::default();

            while let Some(event) = rx.recv().await {
                if let Some(job) = job.as_ref() {
                    attach_tui_job_progress(&store, job.id, &event);
                }
                job_result.observe(&event);
                match format {
                    crate::EventsFormat::Json => {
                        let line = serde_json::to_string(&TiffanyProgressEvent::from(event))?;
                        stdout.write_all(line.as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                        stdout.flush().await?;
                    }
                    crate::EventsFormat::Text => {
                        if let Some(line) = text_formatter.format(&event) {
                            stdout.write_all(line.as_bytes()).await?;
                            stdout.write_all(b"\n").await?;
                            stdout.flush().await?;
                        }
                    }
                }
            }

            let run_result = run.await?;
            match run_result {
                Ok(_) => {
                    if let Some(job) = job {
                        let result = job_result.result();
                        let _ = store.set_tui_job_status(job.id, "done", result.as_deref(), None);
                    }
                    Ok(())
                }
                Err(err) => {
                    if let Some(job) = job {
                        let _ = store.set_tui_job_status(
                            job.id,
                            "failed",
                            None,
                            Some(&format!("{err:#}")),
                        );
                    }
                    Err(err)
                }
            }
        }

        crate::Cmd::Jobs { action, limit } => {
            let cfg = Config::load(config_path)?;
            let store = SessionStore::open(&cfg.behavior.session_log_dir, &cfg.behavior.db_path)?;
            println!("{}", run_jobs_command(&store, action, limit)?);
            Ok(())
        }

        crate::Cmd::Tui {
            detach,
            new_tab,
            ratatui,
        } => {
            if !detach && !new_tab {
                if let Some(status) = run_tiffany_tui(config_path)? {
                    if status.success() {
                        return Ok(());
                    }
                    anyhow::bail!("tiffany-loop UI exited with status {}", status);
                }
                println!(
                    "tiffany-loop UI binary not found; using legacy terminal chat. Run `./scripts/tiffany-dev` from source or install the `tiffany-loop` binary for the primary UI."
                );
            }

            // If user explicitly asked for a new zellij tab, do it
            if mux::zellij::in_zellij() && new_tab {
                println!("→ opening new zellij tab for terminal chat");
                return mux::zellij::open_tui_in_new_tab();
            }
            if detach && !mux::zellij::in_zellij() {
                return mux::fallback::detach_tui();
            }
            let cfg = Config::load(config_path)?;
            let store = Arc::new(orchestrator::core::session_store::SessionStore::open(
                &cfg.behavior.session_log_dir,
                &cfg.behavior.db_path,
            )?);
            let orch = build_orchestrator(&cfg, false, false, None, None, None).await?;
            if ratatui {
                orchestrator::tui::terminal::run_with_mode_notice(
                    store,
                    Arc::new(orch),
                    Arc::new(cfg),
                    config_path,
                    "--ratatui now uses the tiffany-loop-style normal scrollback renderer for stable resize and native selection.",
                )
                .await
            } else {
                orchestrator::tui::terminal::run(store, Arc::new(orch), Arc::new(cfg), config_path)
                    .await
            }
        }

        crate::Cmd::Acp {
            agent,
            no_critic,
            no_reviewer,
        } => {
            let cfg = Config::load(config_path)?;
            let orch = build_orchestrator(&cfg, no_critic, no_reviewer, None, None, None).await?;
            orchestrator::acp::serve_stdio(Arc::new(orch), agent, cfg.roles.clone()).await
        }

        crate::Cmd::Sessions { action } => {
            let cfg = Config::load(config_path)?;
            let store = orchestrator::core::session_store::SessionStore::open(
                &cfg.behavior.session_log_dir,
                &cfg.behavior.db_path,
            )?;
            match action {
                crate::SessionsCmd::List { limit } => {
                    let sessions = store.list(10_000)?;
                    let shown = sessions
                        .iter()
                        .take(limit as usize)
                        .cloned()
                        .collect::<Vec<_>>();
                    println!(
                        "{}",
                        orchestrator::session_display::format_session_list_with_options(
                            &shown,
                            &sessions,
                            orchestrator::session_display::SessionListRenderOptions {
                                action_style:
                                    orchestrator::session_display::SessionListActionStyle::Cli,
                            },
                        )
                    );
                }
                crate::SessionsCmd::Show {
                    id,
                    raw,
                    tail,
                    tree,
                    flow,
                } => {
                    let s = store.resolve_selector(id.as_deref().unwrap_or("last"))?;
                    let path = store.log_path(s.id);
                    if flow {
                        let all_sessions = store.list(10_000)?;
                        println!(
                            "{}",
                            orchestrator::session_display::format_session_flow(
                                &s,
                                &all_sessions,
                                store.log_dir(),
                                orchestrator::session_display::SessionFlowRenderOptions {
                                    tail_per_session: tail,
                                },
                            )
                        );
                    } else if tree {
                        let all_sessions = store.list(10_000)?;
                        println!(
                            "{}",
                            orchestrator::session_display::format_session_tree(
                                &s,
                                &all_sessions,
                                store.log_dir(),
                            )
                        );
                    } else if path.exists() {
                        println!(
                            "{}",
                            orchestrator::session_display::format_session_log(
                                &s,
                                &path,
                                orchestrator::session_display::SessionLogRenderOptions {
                                    raw,
                                    tail
                                },
                            )?
                        );
                    } else {
                        println!(
                            "{}\n\nEvents:\n  log file not found: {}",
                            orchestrator::session_display::format_session_header(&s, &path),
                            path.display()
                        );
                    }
                }
                crate::SessionsCmd::Grep { pattern, limit } => {
                    let hits = store.grep(&pattern)?;
                    println!(
                        "{}",
                        orchestrator::session_display::format_session_grep(
                            &pattern,
                            hits,
                            orchestrator::session_display::SessionGrepRenderOptions {
                                limit,
                                action_style:
                                    orchestrator::session_display::SessionListActionStyle::Cli,
                            },
                        )
                    );
                }
                crate::SessionsCmd::Export {
                    id,
                    format,
                    out,
                    clipboard,
                } => {
                    let session = store.resolve_selector(id.as_deref().unwrap_or("last"))?;
                    let format = match format {
                        crate::SessionExportFormatArg::Markdown => SessionExportFormat::Markdown,
                        crate::SessionExportFormatArg::Html => SessionExportFormat::Html,
                    };
                    if clipboard {
                        let body = orchestrator::session_export::render_session_markdown(
                            &store, &session,
                        )?;
                        copy_to_clipboard_cli(&body)?;
                        println!(
                            "Copied session {} Markdown to clipboard ({} bytes).",
                            orchestrator::session_export::short_session_id(&session),
                            body.len()
                        );
                    } else if let Some(path) = out {
                        let body = match format {
                            SessionExportFormat::Markdown => {
                                orchestrator::session_export::render_session_markdown(
                                    &store, &session,
                                )?
                            }
                            SessionExportFormat::Html => {
                                orchestrator::session_export::render_session_html(&store, &session)?
                            }
                        };
                        write_session_export(&path, &body)?;
                        println!(
                            "Exported session {} to {}",
                            orchestrator::session_export::short_session_id(&session),
                            path.display()
                        );
                    } else {
                        let export = orchestrator::session_export::export_session_to_file(
                            &store, &session, format,
                        )?;
                        println!(
                            "Exported session {} to {}",
                            orchestrator::session_export::short_session_id(&export.session),
                            export.path.display()
                        );
                    }
                }
                crate::SessionsCmd::ImportCc { project } => {
                    use orchestrator::cc_session_import;
                    let cwd = match project {
                        Some(p) => std::path::PathBuf::from(p),
                        None => std::env::current_dir()?,
                    };
                    let report = cc_session_import::import_cc_sessions(&store, &cwd)?;
                    println!(
                        "discovered {} CC session(s) from {}; imported {}, backfilled {}, skipped {}",
                        report.discovered,
                        report.project.display(),
                        report.imported,
                        report.backfilled,
                        report.skipped
                    );
                    for id in report.session_ids.iter().take(20) {
                        println!("  {}", id);
                    }
                    if report.session_ids.len() > 20 {
                        println!("  ... {} more", report.session_ids.len() - 20);
                    }
                }
                crate::SessionsCmd::ImportNative { path } => {
                    let path = match path {
                        Some(path) => path,
                        None => default_native_sessions_path()
                            .context("could not find tiffany-loop native history; pass --path")?,
                    };
                    let report = import_native_sessions_file(&store, &path)?;
                    println!(
                        "imported Tiffany native history from {}\n  conversations: {}\n  turns: {}\n  events: {}",
                        path.display(),
                        report.conversations,
                        report.turns,
                        report.events
                    );
                }
                crate::SessionsCmd::NativeHistory {
                    cwd,
                    format,
                    out,
                    role,
                    thread,
                    native,
                    kind,
                } => {
                    let cwd = match cwd {
                        Some(cwd) => cwd,
                        None => {
                            let cwd = std::env::current_dir()?;
                            cwd.canonicalize()
                                .unwrap_or(cwd)
                                .to_string_lossy()
                                .to_string()
                        }
                    };
                    let conversation = store.native_conversation_by_cwd(&cwd)?;
                    let filter = NativeHistoryCliFilter::new(role, thread, native, kind);
                    let conversation = conversation.map(|conversation| {
                        filter_native_conversation_for_cli(conversation, &filter)
                    });
                    let body = match format {
                        crate::NativeHistoryFormat::Json => {
                            serde_json::to_string_pretty(&conversation)?
                        }
                        crate::NativeHistoryFormat::Text => {
                            format_native_history_cli(conversation.as_ref(), &cwd, &filter)
                        }
                    };
                    if let Some(out) = out {
                        if let Some(parent) = out.parent() {
                            std::fs::create_dir_all(parent)
                                .with_context(|| format!("creating {}", parent.display()))?;
                        }
                        std::fs::write(&out, body.as_bytes())
                            .with_context(|| format!("writing {}", out.display()))?;
                        println!("Exported native history to {}", out.display());
                    } else {
                        println!("{body}");
                    }
                }
            }
            Ok(())
        }

        crate::Cmd::Roles { action } => handle_roles(config_path, action),

        crate::Cmd::Thread { action } => handle_thread(config_path, action),

        crate::Cmd::Status => {
            print_status(config_path)?;
            Ok(())
        }

        crate::Cmd::Doctor { format } => {
            let report = orchestrator::doctor::run(config_path);
            match format {
                crate::DoctorFormat::Text => println!("{}", report.render_text()),
                crate::DoctorFormat::Json => println!("{}", report.render_json_pretty()?),
            }
            Ok(())
        }

        crate::Cmd::Usage { window } => {
            let cfg = Config::load(config_path)?;
            let store = orchestrator::core::session_store::SessionStore::open(
                &cfg.behavior.session_log_dir,
                &cfg.behavior.db_path,
            )?;
            let win = match window.as_str() {
                "today" | "day" => orchestrator::usage::UsageWindow::Today,
                "month" => orchestrator::usage::UsageWindow::ThisMonth,
                "week" => orchestrator::usage::UsageWindow::LastDays(7),
                "all" => orchestrator::usage::UsageWindow::All,
                _ => orchestrator::usage::UsageWindow::Today,
            };
            let u = orchestrator::usage::compute_for_window(&store, win)?;
            println!("=== Token usage ({}) ===\n", window);
            println!(
                "Total: {} tokens in · {} tokens out · ${:.4}",
                u.total_tokens_in, u.total_tokens_out, u.total_cost_usd
            );
            println!("\nBy provider:");
            let mut provs: Vec<_> = u.by_provider.iter().collect();
            provs.sort_by_key(|provider| Reverse(provider.1.tokens_in));
            for (name, p) in provs {
                println!(
                    "  {:<20} {} in / {} out / ${:.4} ({} sessions)",
                    name, p.tokens_in, p.tokens_out, p.cost_usd, p.session_count
                );
            }
            if !u.by_day.is_empty() {
                println!("\nBy day:");
                for d in &u.by_day {
                    println!(
                        "  {}  {} in / {} out / ${:.4}",
                        d.date, d.tokens_in, d.tokens_out, d.cost_usd
                    );
                }
            }
            if let Some(status) =
                orchestrator::usage::compute_budget_status(&store, &cfg.behavior.token_plan)?
            {
                println!("\n{}", orchestrator::usage::format_budget_status(&status));
            }
            Ok(())
        }

        crate::Cmd::Config { action } => match action {
            crate::ConfigCmd::Show => show_config(config_path),
            crate::ConfigCmd::Set { role, model } => set_role_model(config_path, &role, &model),
            crate::ConfigCmd::Get { role } => get_role_model(config_path, &role),
            crate::ConfigCmd::UseClaude {
                planner,
                critic,
                worker_cc,
                worker_codex,
                reviewer,
            } => apply_claude_preset(
                config_path,
                planner.as_deref().unwrap_or("sonnet"),
                critic.as_deref().unwrap_or("opus"),
                worker_cc.as_deref().unwrap_or("sonnet"),
                worker_codex.as_deref(),
                reviewer.as_deref().unwrap_or("haiku"),
            ),
            crate::ConfigCmd::UseOpenai {
                planner,
                critic,
                worker_cc,
                worker_codex,
                reviewer,
            } => apply_role_models(
                config_path,
                &[
                    ("planner", &planner),
                    ("critic", &critic),
                    ("worker-cc", &worker_cc),
                    ("worker-codex", &worker_codex),
                    ("reviewer", &reviewer),
                ],
            ),
            crate::ConfigCmd::Wizard => run_wizard(config_path),
            crate::ConfigCmd::SetKey {
                provider,
                kind,
                key,
            } => set_provider_key(config_path, &provider, kind.as_deref(), key.as_deref()),
            crate::ConfigCmd::SetEndpoint {
                provider,
                url,
                kind,
            } => set_provider_endpoint(config_path, &provider, kind.as_deref(), &url),
            crate::ConfigCmd::Provider { action } => match action {
                None => run_provider_setup_ui(config_path, false, false),
                Some(crate::ProviderConfigCmd::Ui { dry_run, check_env }) => {
                    run_provider_setup_ui(config_path, dry_run, check_env)
                }
                Some(crate::ProviderConfigCmd::List) => list_providers(config_path),
                Some(crate::ProviderConfigCmd::Show { provider }) => {
                    show_provider(config_path, &provider)
                }
                Some(crate::ProviderConfigCmd::Presets) => list_provider_presets(),
                Some(crate::ProviderConfigCmd::Delete { provider, dry_run }) => {
                    delete_provider(config_path, &provider, dry_run)
                }
                Some(crate::ProviderConfigCmd::Setup {
                    provider,
                    kind,
                    key,
                    env,
                    endpoint,
                    dry_run,
                    check_env,
                }) => setup_provider(
                    config_path,
                    &provider,
                    kind.as_deref(),
                    key.as_deref(),
                    env.as_deref(),
                    endpoint.as_deref(),
                    dry_run,
                    check_env,
                ),
            },
            crate::ConfigCmd::UseExpensiveOrchestrators {
                orchestrator_model,
                reviewer_model,
                worker_model,
            } => apply_expensive_orchestrators(
                config_path,
                &orchestrator_model,
                &reviewer_model,
                worker_model.as_deref(),
            ),
            crate::ConfigCmd::UseRoleset { name } => apply_roleset(config_path, &name),
        },
    }
}

async fn run_single_mode(
    cfg: &Config,
    orch: &Orchestrator,
    prompt: String,
    tags: Vec<String>,
    worker: Option<&str>,
    cc_agent: Option<&str>,
) -> Result<()> {
    let mut task = Task::new(prompt);
    task.tags = tags;
    if let Some(worker) = worker {
        task.agent_hint = runtime::normalize_agent_hint_with_roles(worker, &cfg.roles);
    }
    task.cc_agent_hint = sanitize_cc_agent_hint(cc_agent);
    let results = orch.run(task).await?;
    print_completed_tasks(&results);
    Ok(())
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct DetachedRunRecord {
    id: String,
    pid: u32,
    prompt: String,
    created_at: String,
    log_path: PathBuf,
    exit_path: PathBuf,
}

struct DetachRunRequest<'a> {
    config_path: &'a Path,
    prompt: &'a str,
    tags: &'a [String],
    planner: Option<&'a str>,
    critic: Option<&'a str>,
    worker: Option<&'a str>,
    cc_agent: Option<&'a str>,
    reviewer: Option<&'a str>,
    no_critic: bool,
    no_reviewer: bool,
}

fn detach_run(request: DetachRunRequest<'_>) -> Result<()> {
    let run_dir = detached_runs_dir()?;
    std::fs::create_dir_all(&run_dir)?;
    let id = detached_run_id();
    let log_path = run_dir.join(format!("{id}.log"));
    let exit_path = run_dir.join(format!("{id}.exit"));
    let status_path = run_dir.join(format!("{id}.json"));
    let latest_path = run_dir.join("last.json");

    let exe = std::env::current_exe().context("could not resolve current executable")?;
    let log_file = std::fs::File::create(&log_path)
        .with_context(|| format!("creating {}", log_path.display()))?;
    let mut event_args = vec![
        "--config".to_string(),
        request.config_path.display().to_string(),
        "events".to_string(),
        request.prompt.to_string(),
        "--format".to_string(),
        "text".to_string(),
    ];
    for tag in request.tags {
        event_args.push("--tag".to_string());
        event_args.push(tag.clone());
    }
    if let Some(planner) = request.planner {
        event_args.push("--planner".to_string());
        event_args.push(planner.to_string());
    }
    if let Some(critic) = request.critic {
        event_args.push("--critic".to_string());
        event_args.push(critic.to_string());
    }
    if let Some(worker) = request.worker {
        event_args.push("--worker".to_string());
        event_args.push(worker.to_string());
    }
    if let Some(cc_agent) = sanitize_cc_agent_hint(request.cc_agent) {
        event_args.push("--agent".to_string());
        event_args.push(cc_agent);
    }
    if let Some(reviewer) = request.reviewer {
        event_args.push("--reviewer".to_string());
        event_args.push(reviewer.to_string());
    }
    if request.no_critic {
        event_args.push("--no-critic".to_string());
    }
    if request.no_reviewer {
        event_args.push("--no-reviewer".to_string());
    }

    let shell_script = detached_run_shell_script(&exe, &event_args, &exit_path);
    let mut command = Command::new(default_shell_program());
    command
        .arg(default_shell_flag())
        .arg(shell_script)
        .stdin(Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file);

    let child = prepare_detached_process(&mut command)
        .spawn()
        .context("starting detached orchestrator run")?;
    let record = DetachedRunRecord {
        id: id.clone(),
        pid: child.id(),
        prompt: request.prompt.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        log_path,
        exit_path,
    };
    write_detached_run_record(&status_path, &record)?;
    write_detached_run_record(&latest_path, &record)?;

    println!("detached run started: {}", record.id);
    println!("pid: {}", record.pid);
    println!("log: {}", record.log_path.display());
    println!("attach: orchestrator attach {}", record.id);
    Ok(())
}

fn attach_run(selector: Option<&str>, tail: usize, show_status: bool) -> Result<()> {
    let record = resolve_detached_run(selector)?;
    let state = detached_run_state(&record);
    println!("Detached run {}", record.id);
    println!("  status: {}", state);
    println!("  pid: {}", record.pid);
    println!("  started: {}", record.created_at);
    println!("  log: {}", record.log_path.display());
    println!("  prompt: {}", truncate_for_cli(&record.prompt, 180));
    if show_status && state == "running" {
        println!("  follow: tail -f {}", record.log_path.display());
        println!("  stop: kill {}", record.pid);
    }
    println!();
    print_file_tail(&record.log_path, tail)?;
    Ok(())
}

fn detached_run_state(record: &DetachedRunRecord) -> String {
    if let Ok(code) = std::fs::read_to_string(&record.exit_path) {
        let code = code.trim();
        if code == "0" {
            return "completed".to_string();
        }
        if !code.is_empty() {
            return format!("exited {code}");
        }
    }
    if process_is_running(record.pid) {
        "running".to_string()
    } else {
        "unknown".to_string()
    }
}

fn detached_runs_dir() -> Result<PathBuf> {
    let home = home::home_dir().context("could not determine home directory")?;
    Ok(home.join(".orchestrator").join("runs"))
}

fn detached_run_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("run-{millis}-{}", std::process::id())
}

fn write_detached_run_record(path: &Path, record: &DetachedRunRecord) -> Result<()> {
    let body = serde_json::to_string_pretty(record)?;
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

fn detached_run_shell_script(exe: &Path, args: &[String], exit_path: &Path) -> String {
    let mut command = shell_quote(&exe.display().to_string());
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    format!(
        "{}; status=$?; printf '%s\\n' \"$status\" > {}; exit \"$status\"",
        command,
        shell_quote(&exit_path.display().to_string())
    )
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn default_shell_program() -> &'static str {
    if cfg!(windows) {
        "cmd"
    } else {
        "sh"
    }
}

fn default_shell_flag() -> &'static str {
    if cfg!(windows) {
        "/C"
    } else {
        "-c"
    }
}

fn resolve_detached_run(selector: Option<&str>) -> Result<DetachedRunRecord> {
    let run_dir = detached_runs_dir()?;
    let path = match selector.filter(|value| !value.trim().is_empty()) {
        None | Some("last") | Some(".") => run_dir.join("last.json"),
        Some(selector) => find_detached_run_record(&run_dir, selector)?,
    };
    let body = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "reading detached run record {}; start one with `orchestrator run --detach ...`",
            path.display()
        )
    })?;
    serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))
}

fn find_detached_run_record(run_dir: &Path, selector: &str) -> Result<PathBuf> {
    let exact = run_dir.join(format!("{selector}.json"));
    if exact.exists() {
        return Ok(exact);
    }
    let mut matches = vec![];
    if run_dir.exists() {
        for entry in std::fs::read_dir(run_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.file_name().and_then(|name| name.to_str()) == Some("last.json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if path.extension().and_then(|ext| ext.to_str()) == Some("json")
                && stem.starts_with(selector)
            {
                matches.push(path);
            }
        }
    }
    match matches.len() {
        0 => anyhow::bail!("detached run not found: {selector}"),
        1 => Ok(matches.remove(0)),
        count => anyhow::bail!("ambiguous detached run prefix: {selector} ({count} matches)"),
    }
}

fn print_file_tail(path: &Path, lines: usize) -> Result<()> {
    let body =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw_lines = body.lines().collect::<Vec<_>>();
    let start = raw_lines.len().saturating_sub(lines);
    if raw_lines.is_empty() {
        println!("(log is empty)");
        return Ok(());
    }
    for line in &raw_lines[start..] {
        println!("{line}");
    }
    Ok(())
}

fn truncate_for_cli(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push('…');
    }
    out
}

#[derive(Default)]
struct TuiJobResultCapture {
    final_output: Option<String>,
    last_worker_output: Option<String>,
}

impl TuiJobResultCapture {
    fn observe(&mut self, event: &orchestrator::pipeline::orchestrator::RunProgress) {
        let orchestrator::pipeline::orchestrator::RunProgress::WorkerOutput {
            event_kind,
            content,
            ..
        } = event
        else {
            return;
        };
        if is_worker_error_event_kind(event_kind) {
            return;
        }
        if !is_worker_process_event_kind(event_kind) {
            if let Some(final_output) = orchestrator::agent_events::final_output_candidate(
                content,
                TIFFANY_JOB_RESULT_MAX_CHARS,
            ) {
                remember_better_job_text(&mut self.final_output, final_output);
            }
        }
        let display =
            orchestrator::agent_events::humanize_jsonish(content, TIFFANY_JOB_RESULT_MAX_CHARS);
        if display.trim().is_empty() || orchestrator::agent_events::is_low_value_output(&display) {
            return;
        }
        if is_worker_process_event_kind(event_kind) {
            return;
        }
        let normalized = normalize_job_worker_output(&display);
        if normalized.trim().is_empty()
            || orchestrator::agent_events::is_low_value_output(&normalized)
        {
            return;
        }
        remember_better_job_text(&mut self.last_worker_output, normalized);
    }

    fn result(&self) -> Option<String> {
        if let Some(final_output) = self.final_output.as_deref() {
            let display = orchestrator::agent_events::humanize_jsonish(
                final_output,
                TIFFANY_JOB_RESULT_MAX_CHARS,
            );
            if !display.trim().is_empty() {
                return Some(display);
            }
        }
        self.last_worker_output
            .as_deref()
            .map(|text| {
                orchestrator::agent_events::humanize_jsonish(text, TIFFANY_JOB_RESULT_MAX_CHARS)
            })
            .filter(|text| !text.trim().is_empty())
    }
}

fn is_worker_error_event_kind(event_kind: &str) -> bool {
    matches!(
        orchestrator::agent_events::visible_agent_output_kind_for_event_kind(event_kind),
        Some(orchestrator::agent_events::VisibleAgentOutputKind::Stderr)
            | Some(orchestrator::agent_events::VisibleAgentOutputKind::Actionable)
    ) || matches!(event_kind, "error" | "stderr" | "process_exit")
}

fn is_worker_process_event_kind(event_kind: &str) -> bool {
    orchestrator::agent_events::visible_agent_output_kind_for_event_kind(event_kind)
        .is_some_and(|kind| kind.is_process_event())
}

fn normalize_job_worker_output(display: &str) -> String {
    let normalized = orchestrator::agent_events::normalize_output_summary(display);
    let trimmed = normalized.trim();
    let Some((prefix, body)) = trimmed.split_once(": ") else {
        return trimmed.to_string();
    };
    let kind = prefix.split_whitespace().last().unwrap_or_default();
    if matches!(kind, "tool" | "tool_use" | "tool_result" | "exec") {
        body.trim_start().to_string()
    } else {
        trimmed.to_string()
    }
}

fn remember_better_job_text(slot: &mut Option<String>, candidate: String) {
    let candidate = candidate.trim().to_string();
    if candidate.is_empty() {
        return;
    }
    let should_replace = slot
        .as_ref()
        .map(|current| candidate.chars().count() > current.chars().count())
        .unwrap_or(true);
    if should_replace {
        *slot = Some(candidate);
    }
}

fn format_tui_jobs_cli(store: &SessionStore, limit: usize) -> Result<String> {
    orchestrator::tui_jobs::format_tui_jobs(
        store,
        limit,
        orchestrator::tui_jobs::TuiJobsSurface::Cli,
    )
}

fn run_jobs_command(
    store: &SessionStore,
    action: Option<crate::JobsCmd>,
    parent_limit: usize,
) -> Result<String> {
    match action {
        None => format_tui_jobs_cli(store, parent_limit),
        Some(crate::JobsCmd::List { limit }) => format_tui_jobs_cli(store, limit),
        Some(crate::JobsCmd::Show { id }) => {
            let job = resolve_tui_job(store, &id)?;
            Ok(format_tui_job_detail_cli(store, &job))
        }
        Some(crate::JobsCmd::Cancel { id }) => cancel_tui_job(store, &id),
        Some(crate::JobsCmd::Recover { stale_minutes }) => {
            recover_stale_tui_jobs(store, stale_minutes, parent_limit)
        }
        Some(crate::JobsCmd::Retry {
            id,
            emit_retry_prompt,
            tui_handoff,
        }) => retry_tui_job(store, &id, parent_limit, emit_retry_prompt, tui_handoff),
    }
}

fn recover_stale_tui_jobs(
    store: &SessionStore,
    stale_minutes: u64,
    parent_limit: usize,
) -> Result<String> {
    if stale_minutes == 0 {
        anyhow::bail!("jobs recover --stale-minutes must be greater than zero");
    }
    let now = chrono::Utc::now();
    let cutoff = now - chrono::Duration::minutes(stale_minutes.min(i64::MAX as u64) as i64);
    let mut recovered = Vec::new();
    for job in store.list_active_tui_jobs()? {
        if job.status != "running" {
            continue;
        }
        let last_seen = job.started_at.unwrap_or(job.updated_at);
        if last_seen > cutoff {
            continue;
        }
        let error = format!(
            "recovered stale running job after {stale_minutes} minute(s); previous process is no longer tracked"
        );
        store.set_tui_job_status(job.id, "failed", None, Some(&error))?;
        recovered.push(short_uuid_for_cli(job.id));
    }

    let mut out = if recovered.is_empty() {
        format!("Recovered stale jobs\n  none older than {stale_minutes} minute(s)")
    } else {
        format!(
            "Recovered stale jobs\n  failed: {}  ids: {}",
            recovered.len(),
            recovered.join(", ")
        )
    };
    out.push_str("\n\n");
    out.push_str(&format_tui_jobs_cli(store, parent_limit)?);
    Ok(out)
}

fn retry_tui_job(
    store: &SessionStore,
    selector: &str,
    parent_limit: usize,
    emit_retry_prompt: bool,
    tui_handoff: bool,
) -> Result<String> {
    let job = resolve_tui_job(store, selector)?;
    if matches!(job.status.as_str(), "queued" | "running") {
        anyhow::bail!(
            "job {} is {}; use `orchestrator jobs show {}` or cancel/recover it first",
            short_uuid_for_cli(job.id),
            job.status,
            short_uuid_for_cli(job.id)
        );
    }

    if tui_handoff {
        let mut out = format!(
            "Job {} prepared for TUI retry\n  status: queued in current TUI input queue\n  prompt: {}\n  retry prompt: {}\n\nNext:\n  /queue run\n  /jobs",
            short_uuid_for_cli(job.id),
            truncate_for_cli(&job.prompt.replace('\n', " "), 160),
            escape_retry_prompt_for_tui(&job.prompt)
        );
        out.push_str("\n\n");
        out.push_str(&format_tui_jobs_cli(store, parent_limit)?);
        return Ok(out);
    }

    let retry = store.create_tui_job(
        &job.prompt,
        "queued",
        job.route.as_deref(),
        job.role.as_deref(),
    )?;
    let mut out = format!(
        "Job {} queued for retry as {}\n  status: queued\n  prompt: {}\n\nNext:\n  tiffany-loop /jobs show {}\n  tiffany-loop /queue run",
        short_uuid_for_cli(job.id),
        short_uuid_for_cli(retry.id),
        truncate_for_cli(&job.prompt.replace('\n', " "), 160),
        short_uuid_for_cli(retry.id)
    );
    if emit_retry_prompt {
        out.push_str(&format!(
            "\n  retry prompt: {}",
            escape_retry_prompt_for_tui(&job.prompt)
        ));
    }
    out.push_str("\n\n");
    out.push_str(&format_tui_jobs_cli(store, parent_limit)?);
    Ok(out)
}

fn escape_retry_prompt_for_tui(prompt: &str) -> String {
    let mut escaped = String::with_capacity(prompt.len());
    for ch in prompt.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn cancel_tui_job(store: &SessionStore, selector: &str) -> Result<String> {
    let job = resolve_tui_job(store, selector)?;
    if matches!(
        job.status.as_str(),
        "done" | "failed" | "cancelled" | "removed" | "skipped"
    ) {
        return Ok(format!(
            "Job {} already {}\n\n{}",
            short_uuid_for_cli(job.id),
            job.status,
            format_tui_job_detail_cli(store, &job)
        ));
    }

    let message = if job.status == "running" {
        "cancel requested from jobs; active process may finish if already running"
    } else {
        "cancelled by user from jobs"
    };
    store.set_tui_job_status(job.id, "cancelled", None, Some(message))?;
    let job = store.get_tui_job(job.id)?.unwrap_or(job);
    Ok(format!(
        "Job {} cancelled\n\n{}",
        short_uuid_for_cli(job.id),
        format_tui_job_detail_cli(store, &job)
    ))
}

fn resolve_tui_job(store: &SessionStore, selector: &str) -> Result<TuiJob> {
    let selector = selector.trim();
    if selector.is_empty() {
        anyhow::bail!("job id is required");
    }

    if matches!(selector, "." | "last") {
        return store
            .list_tui_jobs(1)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no persisted jobs yet"));
    }

    if let Ok(id) = uuid::Uuid::parse_str(selector) {
        return store
            .get_tui_job(id)?
            .ok_or_else(|| anyhow::anyhow!("tui job not found: {selector}"));
    }

    let matches = store.find_tui_jobs_by_id_prefix(selector, 20)?;
    match matches.len() {
        0 => anyhow::bail!("tui job not found: {selector}"),
        1 => Ok(matches.into_iter().next().expect("single job")),
        _ => {
            let ids = matches
                .iter()
                .map(|job| short_uuid_for_cli(job.id))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!("ambiguous tui job prefix: {selector} ({ids})")
        }
    }
}

fn format_tui_job_detail_cli(store: &SessionStore, job: &TuiJob) -> String {
    orchestrator::tui_jobs::format_tui_job_detail(
        store,
        job,
        orchestrator::tui_jobs::TuiJobsSurface::Cli,
    )
}

fn short_uuid_for_cli(id: uuid::Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn attach_tui_job_progress(
    store: &SessionStore,
    job_id: uuid::Uuid,
    event: &orchestrator::pipeline::orchestrator::RunProgress,
) {
    use orchestrator::pipeline::orchestrator::RunProgress;

    match event {
        RunProgress::WorkerStarted { task_id, role, .. } => {
            let _ = store.attach_tui_job_worker(job_id, Some(*task_id), Some(role), None, None);
        }
        RunProgress::WorkerThreadWaiting {
            task_id,
            role,
            thread_id,
            native_session_id,
            ..
        }
        | RunProgress::WorkerThreadReady {
            task_id,
            role,
            thread_id,
            native_session_id,
            ..
        } => {
            let _ = store.attach_tui_job_worker(
                job_id,
                Some(*task_id),
                Some(role),
                Some(*thread_id),
                native_session_id.as_deref(),
            );
        }
        RunProgress::WorkerRecovery {
            task_id,
            role,
            thread_id,
            native_session_id,
            ..
        } => {
            let _ = store.attach_tui_job_worker(
                job_id,
                Some(*task_id),
                Some(role),
                Some(*thread_id),
                native_session_id.as_deref(),
            );
        }
        RunProgress::WorkerDone { task_id, role, .. } => {
            let _ = store.attach_tui_job_completed_task(job_id, *task_id, Some(role));
        }
        _ => {}
    }
}

#[cfg(unix)]
fn prepare_detached_process(command: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;

    command.process_group(0)
}

#[cfg(not(unix))]
fn prepare_detached_process(command: &mut Command) -> &mut Command {
    command
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_is_running(_pid: u32) -> bool {
    false
}

fn sanitize_cc_agent_hint(agent: Option<&str>) -> Option<String> {
    agent
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
        .map(str::to_string)
}

async fn run_ab_mode(
    cfg: &Config,
    orch: &Orchestrator,
    prompt: String,
    tags: Vec<String>,
    worker: Option<&str>,
    cc_agent: Option<&str>,
) -> Result<()> {
    let routes = ab_worker_routes(cfg, worker)?;
    let cc_agent = sanitize_cc_agent_hint(cc_agent);
    println!(
        "A/B dual-run: {} vs {}",
        routes[0].as_str(),
        routes[1].as_str()
    );

    let mut summaries = Vec::new();
    for (idx, route) in routes.iter().enumerate() {
        println!("\n[A{}] running worker route: {}", idx + 1, route);
        let mut task = Task::new(prompt.clone());
        task.tags = tags.clone();
        task.agent_hint = Some(route.clone());
        task.cc_agent_hint = cc_agent.clone();
        let run = orch.run(task).await;
        let summary = AbRunSummary::from_run(idx, route.clone(), run, orch).await;
        println!(
            "[A{}] {} — {} completed task(s), score bytes={}",
            idx + 1,
            summary.status_label(),
            summary.completed_count,
            summary.score_bytes
        );
        if let Some(error) = summary.error.as_deref() {
            println!("[A{}] error: {}", idx + 1, error);
        }
        summaries.push(summary);
    }

    let winner = pick_ab_winner(&summaries)?;
    let winner_summary = summaries
        .get(winner)
        .expect("AbJudge returned an existing route index");
    println!(
        "\n✓ A/B selected: A{} ({})",
        winner + 1,
        winner_summary.route
    );
    print_ab_summary(&summaries);
    Ok(())
}

fn print_completed_tasks(results: &[Task]) {
    println!("\n✓ Completed {} task(s):", results.len());
    for t in results {
        println!(
            "  - {} [{}] {}",
            t.id,
            format!("{:?}", t.role).to_lowercase(),
            t.prompt
        );
    }
}

fn ab_worker_routes(cfg: &Config, requested_worker: Option<&str>) -> Result<[String; 2]> {
    let available = configured_worker_routes(cfg);
    let Some(first) = requested_worker
        .and_then(|worker| runtime::normalize_agent_hint_with_roles(worker, &cfg.roles))
        .or_else(|| available.first().cloned())
    else {
        anyhow::bail!(
            "A/B dual-run needs at least two configured worker roles. Add worker-cc and worker-codex with `orchestrator roles register ...`."
        );
    };

    if !cfg.roles.contains_key(&first) {
        anyhow::bail!(
            "A/B worker route '{}' is not registered. Available worker roles: {}",
            first,
            available.join(", ")
        );
    }

    let second = available
        .iter()
        .find(|route| route.as_str() != first.as_str())
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "A/B dual-run needs two distinct configured worker roles. Available worker roles: {}",
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            )
        })?;

    Ok([first, second])
}

fn configured_worker_routes(cfg: &Config) -> Vec<String> {
    let mut routes = cfg
        .roles
        .iter()
        .filter(|(name, role)| {
            is_worker_route_name(name) && cfg.runtime_config(&role.runtime).is_some()
        })
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    routes.sort_by_key(|name| worker_route_sort_key(name));
    routes.dedup();
    routes
}

fn worker_route_sort_key(name: &str) -> (u8, String) {
    match name {
        "worker-cc" => (0, name.to_string()),
        "worker-codex" => (1, name.to_string()),
        "worker-gemini" => (2, name.to_string()),
        _ => (3, name.to_string()),
    }
}

fn is_worker_route_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    (normalized.contains("worker") || normalized.contains("executor"))
        && !normalized.contains("reviewer")
}

#[derive(Debug)]
struct AbRunSummary {
    index: usize,
    route: String,
    completed_count: usize,
    score_bytes: usize,
    ok: bool,
    error: Option<String>,
}

impl AbRunSummary {
    async fn from_run(
        index: usize,
        route: String,
        run: Result<Vec<Task>>,
        orch: &Orchestrator,
    ) -> Self {
        match run {
            Ok(tasks) => {
                let score_bytes = ab_score_bytes(&tasks, orch).await;
                let ok = tasks
                    .iter()
                    .any(|task| task.status == TaskStatus::Completed);
                Self {
                    index,
                    route,
                    completed_count: tasks
                        .iter()
                        .filter(|task| task.status == TaskStatus::Completed)
                        .count(),
                    score_bytes,
                    ok,
                    error: None,
                }
            }
            Err(err) => Self {
                index,
                route,
                completed_count: 0,
                score_bytes: usize::MAX / 4,
                ok: false,
                error: Some(format!("{err:#}")),
            },
        }
    }

    fn status_label(&self) -> &'static str {
        if self.ok {
            "ok"
        } else {
            "failed"
        }
    }
}

async fn ab_score_bytes(tasks: &[Task], orch: &Orchestrator) -> usize {
    let worker_sessions = worker_sessions_for_tasks(orch, tasks);
    let mut total = 0usize;
    for session in worker_sessions {
        let Some(adapter) = orch.adapters.get(&session.agent) else {
            continue;
        };
        match adapter.get_diff(&session).await {
            Ok(diff) if !diff.trim().is_empty() => {
                total = total.saturating_add(diff.len());
            }
            _ => {
                total = total.saturating_add(session_log_size(orch, &session));
            }
        }
    }
    total
}

fn worker_sessions_for_tasks(orch: &Orchestrator, tasks: &[Task]) -> Vec<Session> {
    let task_ids = tasks
        .iter()
        .map(|task| task.id)
        .collect::<std::collections::HashSet<_>>();
    orch.session_store
        .list(10_000)
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.role == Role::Worker && task_ids.contains(&session.task_id))
        .collect()
}

fn session_log_size(orch: &Orchestrator, session: &Session) -> usize {
    std::fs::metadata(orch.session_store.log_path(session.id))
        .map(|metadata| usize::try_from(metadata.len()).unwrap_or(usize::MAX / 4))
        .unwrap_or(0)
}

fn pick_ab_winner(summaries: &[AbRunSummary]) -> Result<usize> {
    let judge_inputs = summaries
        .iter()
        .map(|summary| ("x".repeat(summary.score_bytes.min(1024 * 1024)), summary.ok))
        .collect::<Vec<_>>();
    AbJudge::pick(&judge_inputs)
}

fn print_ab_summary(summaries: &[AbRunSummary]) {
    println!("\nA/B summary:");
    for summary in summaries {
        println!(
            "  A{} {:<18} status={} completed={} score_bytes={}",
            summary.index + 1,
            summary.route,
            summary.status_label(),
            summary.completed_count,
            summary.score_bytes
        );
    }
}

fn run_tiffany_tui(config_path: &Path) -> Result<Option<std::process::ExitStatus>> {
    if tiffany_install::legacy_tui_forced() {
        return Ok(None);
    }

    let Some(tiffany_bin) = tiffany_install::find_tiffany_binary() else {
        return Ok(None);
    };
    let orchestrator_bin =
        std::env::current_exe().context("could not resolve current orchestrator executable")?;

    let mut command = Command::new(tiffany_bin);
    command
        .arg("orchestrator")
        .arg("--bin")
        .arg(orchestrator_bin)
        .arg("--orchestrator-config")
        .arg(config_path);

    Ok(Some(
        command
            .status()
            .context("failed to launch tiffany-loop UI")?,
    ))
}

fn print_status(config_path: &Path) -> Result<()> {
    println!("tiffany-loop {}", env!("CARGO_PKG_VERSION"));
    println!(
        "orchestrator: {}",
        tiffany_install::current_orchestrator_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(current executable unknown)".to_string())
    );
    match tiffany_install::source_checkout() {
        Some(source) => println!("source:       {}", source.summary()),
        None => println!("source:       release install"),
    }
    let tiffany_binary = tiffany_install::resolve_tiffany_binary();
    let tiffany_ready = matches!(tiffany_binary.as_ref(), Some(binary) if binary.verified);
    match tiffany_binary.as_ref() {
        Some(binary) => {
            let verified = if binary.verified { "" } else { " (not found)" };
            println!(
                "ui binary:   {} ({}){}",
                binary.path.display(),
                binary.source_label(),
                verified
            );
        }
        None => {
            println!("ui binary:   not found; `orchestrator tui` will use legacy fallback");
        }
    }
    for command in tiffany_install::resolve_tiffany_shell_commands() {
        println!(
            "{:<13}{}",
            format!("{}:", command.name),
            command.status_detail()
        );
    }
    println!(
        "ui mode:     {}",
        if tiffany_install::legacy_tui_forced() {
            "legacy forced by ORCHESTRATOR_LEGACY_TUI"
        } else {
            "tiffany-loop when available"
        }
    );

    if let Some((home, source)) = tiffany_install::resolved_tiffany_home() {
        let (sqlite_home, sqlite_source) = tiffany_install::resolved_tiffany_sqlite_home(&home);
        println!("tiffany home: {} ({})", home.display(), source.label());
        println!(
            "tiffany db:   {} ({})",
            sqlite_home.display(),
            sqlite_source.label()
        );
        println!("tiffany cfg:  {}", home.join("config.toml").display());
    } else {
        println!("tiffany home: unknown; set TIFFANY_HOME");
    }

    let expanded_config = orchestrator::config::expand_home(config_path);
    println!("orch config:  {}", expanded_config.display());
    println!(
        "bridge:       {}",
        tiffany_install::launch_command_preview(config_path)
    );

    let (config_loaded, config_issues) = match Config::load(config_path) {
        Ok(cfg) => {
            println!("db:           {}", cfg.behavior.db_path.display());
            println!("logs:         {}", cfg.behavior.session_log_dir.display());
            println!("mux:          {:?}", cfg.behavior.mux);
            println!(
                "claude:       bypass_permissions={}",
                cfg.behavior.cc_bypass_permissions
            );
            println!(
                "providers:    {}",
                status_name_summary(cfg.providers.keys().cloned().collect(), 4)
            );
            println!(
                "models:       {}",
                status_name_summary(cfg.models.iter().map(|model| model.id.clone()).collect(), 4)
            );
            println!(
                "roles:        {}",
                status_name_summary(cfg.roles.keys().cloned().collect(), 5)
            );
            println!(
                "worker:       {}",
                runtime::default_worker_role(&cfg.roles).unwrap_or_else(|| "(none)".to_string())
            );
            let config_issues = status_config_issues(&cfg);
            println!(
                "health:       {}",
                status_config_health_from_issues(&config_issues)
            );
            (true, config_issues)
        }
        Err(err) => {
            println!("config load:  {err:#}");
            (false, Vec::new())
        }
    };
    if mux::zellij::in_zellij() {
        println!(
            "zellij:       in session {}",
            std::env::var("ZELLIJ_SESSION_NAME").unwrap_or_default()
        );
    }

    for action in status_actions(
        config_loaded,
        &config_issues,
        tiffany_ready,
        tiffany_install::legacy_tui_forced(),
    ) {
        println!("{:<14}{}", format!("{}:", action.label), action.command);
    }
    Ok(())
}

fn status_name_summary(mut names: Vec<String>, limit: usize) -> String {
    names.sort();
    names.dedup();
    if names.is_empty() {
        return "0".to_string();
    }
    let shown = names.iter().take(limit).cloned().collect::<Vec<_>>();
    let suffix = if names.len() > shown.len() {
        format!(", +{}", names.len() - shown.len())
    } else {
        String::new()
    };
    format!("{} ({})", names.len(), shown.join(", ") + &suffix)
}

#[cfg(test)]
fn status_config_health(cfg: &Config) -> String {
    status_config_health_from_issues(&status_config_issues(cfg))
}

fn status_config_health_from_issues(issues: &[String]) -> String {
    if issues.is_empty() {
        "ok".to_string()
    } else {
        format!(
            "{} issue(s): {}",
            issues.len(),
            status_issue_summary(issues, 3)
        )
    }
}

fn status_config_issues(cfg: &Config) -> Vec<String> {
    let mut issues = Vec::new();
    if cfg.providers.is_empty() {
        issues.push("no providers".to_string());
    }
    if cfg.models.is_empty() {
        issues.push("no models".to_string());
    }
    if cfg.roles.is_empty() {
        issues.push("no roles".to_string());
    }

    let role_linked_models = cfg
        .roles
        .values()
        .map(|role| role.model.as_str())
        .collect::<HashSet<_>>();
    let role_linked_providers = cfg
        .roles
        .values()
        .filter_map(|role| cfg.models.iter().find(|model| model.id == role.model))
        .map(|model| model.provider.as_str())
        .collect::<HashSet<_>>();

    for (provider_name, provider) in &cfg.providers {
        let provider_is_required = role_linked_providers.contains(provider_name.as_str());
        if provider_is_required
            && provider.kind != "ollama"
            && provider
                .api_key
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
        {
            issues.push(format!("{provider_name} api key missing"));
        }
        if provider_is_required && provider_needs_base_url(provider_name, provider) {
            issues.push(format!("{provider_name} base_url missing"));
        }
    }

    let model_ids = cfg
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<HashSet<_>>();
    for model in &cfg.models {
        if role_linked_models.contains(model.id.as_str())
            && !cfg.providers.contains_key(&model.provider)
        {
            issues.push(format!("{} provider {} missing", model.id, model.provider));
        }
    }
    for (role_name, role) in &cfg.roles {
        if !model_ids.contains(role.model.as_str()) {
            issues.push(format!("{role_name} model {} missing", role.model));
        }
        if cfg.runtime_config(&role.runtime).is_none() {
            issues.push(format!("{role_name} runtime {} missing", role.runtime));
        }
    }
    if runtime::default_worker_role(&cfg.roles).is_none() {
        issues.push("no default worker".to_string());
    }

    issues.sort();
    issues.dedup();
    issues
}

fn provider_needs_base_url(
    provider_name: &str,
    provider: &orchestrator::config::ProviderConfig,
) -> bool {
    provider.kind.eq_ignore_ascii_case("openai")
        && !provider_name.eq_ignore_ascii_case("openai")
        && provider
            .base_url
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
}

fn status_issue_summary(issues: &[String], limit: usize) -> String {
    let summary = status_issue_categories(issues);
    let shown = summary.iter().take(limit).cloned().collect::<Vec<_>>();
    let suffix = if summary.len() > shown.len() {
        format!("; +{}", summary.len() - shown.len())
    } else {
        String::new()
    };
    shown.join("; ") + &suffix
}

fn status_issue_categories(issues: &[String]) -> Vec<String> {
    let mut provider_auth = Vec::new();
    let mut provider_links = Vec::new();
    let mut config_basics = Vec::new();
    let mut role_wiring = Vec::new();
    let mut other = Vec::new();

    for issue in issues {
        if let Some(provider) = issue.strip_suffix(" api key missing") {
            provider_auth.push(provider.to_string());
        } else if is_missing_provider_link(issue) {
            provider_links.push(issue.clone());
        } else if issue == "no providers" || issue == "no models" || issue == "no roles" {
            config_basics.push(issue.clone());
        } else if issue.contains(" model ")
            || issue.contains(" runtime ")
            || issue == "no default worker"
        {
            role_wiring.push(issue.clone());
        } else {
            other.push(issue.clone());
        }
    }

    let mut categories = Vec::new();
    if !provider_auth.is_empty() {
        provider_auth.sort();
        categories.push(format!(
            "provider auth missing for {}: {}",
            provider_auth.len(),
            status_join_limited(&provider_auth, 4)
        ));
    }
    if !provider_links.is_empty() {
        categories.push(format!(
            "model provider links: {}",
            status_join_limited(&provider_links, 3)
        ));
    }
    if !config_basics.is_empty() {
        categories.push(format!(
            "config incomplete: {}",
            status_join_limited(&config_basics, 3)
        ));
    }
    if !role_wiring.is_empty() {
        categories.push(format!(
            "role/model wiring: {}",
            status_join_limited(&role_wiring, 3)
        ));
    }
    categories.extend(other);
    categories
}

fn status_join_limited(items: &[String], limit: usize) -> String {
    let shown = items.iter().take(limit).cloned().collect::<Vec<_>>();
    let suffix = if items.len() > shown.len() {
        format!(", +{}", items.len() - shown.len())
    } else {
        String::new()
    };
    shown.join(", ") + &suffix
}

fn is_missing_provider_link(issue: &str) -> bool {
    issue.contains(" provider ") && issue.ends_with(" missing")
}

fn is_provider_setup_issue(issue: &str) -> bool {
    issue == "no providers"
        || issue.ends_with(" api key missing")
        || issue.ends_with(" base_url missing")
        || is_missing_provider_link(issue)
}

fn missing_provider_base_url(issue: &str) -> Option<&str> {
    issue.strip_suffix(" base_url missing")
}

fn missing_provider_auth(issue: &str) -> Option<&str> {
    issue.strip_suffix(" api key missing")
}

fn missing_model_provider_link(issue: &str) -> Option<(&str, &str)> {
    let (model, rest) = issue.split_once(" provider ")?;
    let provider = rest.strip_suffix(" missing")?;
    Some((model, provider))
}

fn is_role_setup_issue(issue: &str) -> bool {
    issue == "no models"
        || issue == "no roles"
        || issue == "no default worker"
        || issue.contains(" model ")
        || issue.contains(" runtime ")
}

fn missing_role_model(issue: &str) -> Option<(&str, &str)> {
    let (role, rest) = issue.split_once(" model ")?;
    let model = rest.strip_suffix(" missing")?;
    Some((role, model))
}

fn missing_role_runtime(issue: &str) -> Option<(&str, &str)> {
    let (role, rest) = issue.split_once(" runtime ")?;
    let runtime = rest.strip_suffix(" missing")?;
    Some((role, runtime))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusAction {
    label: &'static str,
    command: String,
}

fn status_actions(
    config_loaded: bool,
    config_issues: &[String],
    tiffany_ready: bool,
    legacy_tui_forced: bool,
) -> Vec<StatusAction> {
    if !config_loaded {
        return vec![
            StatusAction {
                label: "next",
                command: "orchestrator setup".into(),
            },
            StatusAction {
                label: "check",
                command: "orchestrator doctor".into(),
            },
        ];
    }

    if legacy_tui_forced {
        return vec![
            StatusAction {
                label: "next",
                command: "unset ORCHESTRATOR_LEGACY_TUI, then run `orchestrator tui`".into(),
            },
            StatusAction {
                label: "check",
                command: "orchestrator doctor".into(),
            },
        ];
    }

    if !config_issues.is_empty() {
        let mut actions = Vec::new();
        if config_issues
            .iter()
            .any(|issue| is_provider_setup_issue(issue))
        {
            let provider_action_start = actions.len();
            if let Some(provider) = config_issues
                .iter()
                .find_map(|issue| missing_provider_base_url(issue))
            {
                actions.push(StatusAction {
                    label: "fix endpoint",
                    command: format!(
                        "tiffany-loop then `/provider endpoint {provider} <url>`, or `orchestrator config provider setup {provider} --endpoint <url>`"
                    ),
                });
            }
            if let Some(provider) = config_issues
                .iter()
                .find_map(|issue| missing_provider_auth(issue))
            {
                actions.push(StatusAction {
                    label: "fix auth",
                    command: format!(
                        "tiffany-loop then `/provider env {provider} <ENV_NAME>`, or `orchestrator config provider setup {provider} --env <ENV_NAME>`"
                    ),
                });
            }
            if let Some((model, provider)) = config_issues
                .iter()
                .find_map(|issue| missing_model_provider_link(issue))
            {
                actions.push(StatusAction {
                    label: "fix provider",
                    command: format!(
                        "configure provider `{provider}` for model `{model}`: `orchestrator config provider setup {provider} --env <ENV_NAME>`"
                    ),
                });
            }
            if actions.len() == provider_action_start {
                actions.push(StatusAction {
                    label: "fix provider",
                    command: "tiffany-loop then /provider, or `orchestrator config provider setup <provider> --env <ENV_NAME>`".into(),
                });
            }
        }
        if config_issues.iter().any(|issue| is_role_setup_issue(issue)) {
            let role_action_start = actions.len();
            if let Some((role, runtime)) = config_issues
                .iter()
                .find_map(|issue| missing_role_runtime(issue))
            {
                actions.push(StatusAction {
                    label: "fix runtime",
                    command: format!(
                        "fix `{role}` runtime with `/role {role}` or `orchestrator roles register {role} --provider <provider> --model-name <api-model> --runtime {runtime}`"
                    ),
                });
            }
            if let Some((role, model)) = config_issues
                .iter()
                .find_map(|issue| missing_role_model(issue))
            {
                actions.push(StatusAction {
                    label: "fix role",
                    command: format!(
                        "fix `{role}` model with `/role {role}` or `orchestrator roles register {role} --provider <provider> --model-name <api-model> --runtime <runtime-id>`; use `--model {model}` only if reusing that internal id"
                    ),
                });
            }
            if config_issues
                .iter()
                .any(|issue| issue == "no default worker")
            {
                actions.push(StatusAction {
                    label: "add worker",
                    command: "add worker with `orchestrator roles register worker-cc --provider <provider> --model-name <api-model> --runtime claude-code --agent-teams`".into(),
                });
            }
            if actions.len() == role_action_start {
                actions.push(StatusAction {
                    label: "fix role",
                    command: "tiffany-loop then /role, or `orchestrator roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime-id>`".into(),
                });
            }
        }
        if actions.is_empty() {
            actions.push(StatusAction {
                label: "next",
                command: "orchestrator setup, or `tiffany-loop` then /provider + /role".into(),
            });
        }
        actions.push(StatusAction {
            label: "check",
            command: "orchestrator doctor".into(),
        });
        return actions;
    }

    if tiffany_ready {
        vec![
            StatusAction {
                label: "next",
                command: "tiffany-loop".into(),
            },
            StatusAction {
                label: "check",
                command: "orchestrator doctor".into(),
            },
        ]
    } else {
        vec![
            StatusAction {
                label: "next",
                command: "install tiffany-loop or run `./scripts/tiffany-dev` from source".into(),
            },
            StatusAction {
                label: "check",
                command: "orchestrator doctor".into(),
            },
        ]
    }
}

fn show_config(config_path: &Path) -> Result<()> {
    let config_path_display = config_path.display().to_string();
    println!("=== Orchestrator config ===\n");
    println!(
        "config file:    {} (use `orchestrator init` to bootstrap)",
        config_path_display
    );

    match Config::load(config_path) {
        Ok(cfg) => {
            println!("\nBehavior:");
            println!(
                "  worktree_base:    {}",
                cfg.behavior.worktree_base.display()
            );
            println!("  db_path:          {}", cfg.behavior.db_path.display());
            println!(
                "  session_log_dir:  {}",
                cfg.behavior.session_log_dir.display()
            );
            println!("  mux:              {:?}", cfg.behavior.mux);
            println!("  log_level:        {}", cfg.behavior.log_level);
            println!("  enable_critic:    {}", cfg.behavior.enable_critic);
            println!("  enable_reviewer:  {}", cfg.behavior.enable_reviewer);
            println!("  max_replan:       {}", cfg.behavior.max_replan);
            println!(
                "  cc_bypass_permissions: {}",
                cfg.behavior.cc_bypass_permissions
            );
            println!("  enable_ab_judge:  {}", cfg.behavior.enable_ab_judge);

            println!("\nProviders ({}):", cfg.providers.len());
            for (name, p) in &cfg.providers {
                let key_status = match &p.api_key {
                    Some(k) if !k.is_empty() => "✓ set".to_string(),
                    Some(_) => "(empty)".to_string(),
                    None => "—".to_string(),
                };
                let endpoint = p.base_url.as_deref().unwrap_or("—");
                println!(
                    "  - {:<10} type={:<10} api_key={} base_url={}",
                    name, p.kind, key_status, endpoint
                );
            }

            println!("\nModels ({}):", cfg.models.len());
            for m in &cfg.models {
                println!("  - {:<14} {} (provider: {})", m.id, m.name, m.provider);
            }

            println!("\nRoles ({}):", cfg.roles.len());
            for (name, r) in &cfg.roles {
                let at = if r.agent_teams { " [agent_teams]" } else { "" };
                println!(
                    "  - {:<14} → model={:<14} runtime={}{}",
                    name, r.model, r.runtime, at
                );
            }

            println!("\nTag overrides ({}):", cfg.overrides.len());
            for o in &cfg.overrides {
                println!("  - {} → {}", o.tag, o.role);
            }
        }
        Err(e) => {
            println!("  ⚠ could not load config: {:#}", e);
        }
    }

    let am = orchestrator::agent_md::AgentMd::load();
    println!("\n─── AGENTS.md (orchestrator's own instructions) ───\n");
    if am.content.is_empty() {
        println!("  (none found)");
        println!("  create one to set platform-level rules:");
        println!("    mkdir -p ~/.orchestrator");
        println!("    cat > ~/.orchestrator/AGENTS.md << 'EOF'");
        println!("    # My orchestrator rules");
        println!("    Default worker: sonnet");
        println!("    Default critic: opus");
        println!("    This project uses uv not pip");
        println!("    Always run `pytest` before declaring done");
        println!("    EOF");
    } else {
        for line in am.content.lines() {
            println!("  │ {}", line);
        }
    }
    println!("\nAGENTS.md sources:");
    if am.sources.is_empty() {
        println!("  (none)");
    } else {
        for s in &am.sources {
            println!("  ✓ {}", s.display());
        }
    }

    let cc = cc_config::CCConfig::load();
    println!("\n=== Claude Code config (inherited) ===\n");
    println!("CLAUDE.md ({} chars):", cc.system_prompt.len());
    if cc.system_prompt.is_empty() {
        println!("  (none found)");
    } else {
        for line in cc.system_prompt.lines().take(20) {
            println!("  │ {}", line);
        }
        if cc.system_prompt.lines().count() > 20 {
            println!(
                "  │ ... ({} more lines)",
                cc.system_prompt.lines().count() - 20
            );
        }
    }

    println!("\nCC Settings:");
    println!("  model:          {:?}", cc.settings.model);
    println!("  permission_mode: {:?}", cc.settings.permission_mode);
    println!("  allowed_tools:  {:?}", cc.settings.allowed_tools);
    println!("  disabled_mcpjson: {}", cc.settings.disabled_mcpjson);

    println!("\nCC Agents ({}):", cc.agents.len());
    for a in &cc.agents {
        println!("  - {} ({})", a.name, a.source.display());
        if !a.description.is_empty() {
            println!("      {}", a.description);
        }
        if !a.tools.is_empty() {
            println!("      tools: {:?}", a.tools);
        }
    }

    println!("\nCC Commands ({}):", cc.commands.len());
    for c in &cc.commands {
        println!("  - /{} ({})", c.name, c.source.display());
        if !c.description.is_empty() {
            println!("      {}", c.description);
        }
    }

    println!("\nMCP servers ({}):", cc.mcp_servers.len());
    for s in &cc.mcp_servers {
        println!("  - {} → {} {}", s.name, s.command, s.args.join(" "));
    }

    println!("\nCC Prior sessions ({}):", cc.prior_session_ids.len());
    for s in cc.prior_session_ids.iter().take(10) {
        println!("  - {}", s);
    }
    if cc.prior_session_ids.len() > 10 {
        println!("  ... ({} more)", cc.prior_session_ids.len() - 10);
    }

    println!("\nAll sources:");
    for s in &cc.sources {
        println!("  ✓ {}", s);
    }
    Ok(())
}

fn set_role_model(config_path: &Path, role: &str, model_id: &str) -> Result<()> {
    let mut cfg = Config::load(config_path)?;
    if !cfg.models.iter().any(|m| m.id == model_id) {
        anyhow::bail!(
            "unknown model id '{}'. Available: {}",
            model_id,
            cfg.models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let entry = cfg.roles.entry(role.to_string());
    let role_config = match entry {
        std::collections::hash_map::Entry::Occupied(mut o) => {
            o.get_mut().model = model_id.to_string();
            o.into_mut().clone()
        }
        std::collections::hash_map::Entry::Vacant(v) => {
            // Default runtime based on role name
            let runtime = if role.contains("cc") {
                "claude-code"
            } else {
                "codex"
            }
            .to_string();
            v.insert(orchestrator::config::RoleConfig {
                model: model_id.to_string(),
                runtime,
                agent_teams: false,
            })
            .clone()
        }
    };
    Config::write_role_to_config_file(config_path, role, &role_config)?;
    println!("✓ {} → {}", role, model_id);
    Ok(())
}

fn get_role_model(config_path: &Path, role: &str) -> Result<()> {
    let cfg = Config::load(config_path)?;
    if let Some(r) = cfg.roles.get(role) {
        println!("{}: {} (runtime: {})", role, r.model, r.runtime);
    } else {
        println!(
            "role '{}' not found in config. Available: {}",
            role,
            cfg.roles.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(())
}

fn apply_role_models(config_path: &Path, assignments: &[(&str, &str)]) -> Result<()> {
    let mut cfg = Config::load(config_path)?;
    for (role, model_id) in assignments {
        if !cfg.models.iter().any(|m| m.id == *model_id) {
            anyhow::bail!("unknown model id '{}' for role '{}'", model_id, role);
        }
        if let Some(r) = cfg.roles.get_mut(*role) {
            r.model = model_id.to_string();
            Config::write_role_to_config_file(config_path, role, r)?;
        }
    }
    for (role, model_id) in assignments {
        println!("✓ {} → {}", role, model_id);
    }
    Ok(())
}

fn handle_roles(config_path: &Path, action: crate::RolesCmd) -> Result<()> {
    match action {
        crate::RolesCmd::List => print_roles(config_path, None),
        crate::RolesCmd::Show { role } => print_roles(config_path, role.as_deref()),
        crate::RolesCmd::Options => print_role_options(config_path),
        crate::RolesCmd::Register {
            role,
            model,
            runtime,
            provider,
            model_name,
            agent_teams,
            no_agent_teams,
        } => register_role(
            config_path,
            &role,
            model.as_deref(),
            &runtime,
            provider.as_deref(),
            model_name.as_deref(),
            agent_teams,
            no_agent_teams,
        ),
        crate::RolesCmd::Delete { role } => delete_role(config_path, &role),
        crate::RolesCmd::Profile {
            name,
            planner,
            critic,
            reviewer,
            worker_cc,
            worker_codex,
            worker_gemini,
            dry_run,
        } => save_role_profile(
            config_path,
            &name,
            &[
                ("planner", planner.as_deref()),
                ("critic", critic.as_deref()),
                ("reviewer", reviewer.as_deref()),
                ("worker-cc", worker_cc.as_deref()),
                ("worker-codex", worker_codex.as_deref()),
                ("worker-gemini", worker_gemini.as_deref()),
            ],
            dry_run,
        ),
    }
}

fn handle_thread(config_path: &Path, action: crate::ThreadCmd) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let store = orchestrator::core::session_store::SessionStore::open(
        &cfg.behavior.session_log_dir,
        &cfg.behavior.db_path,
    )?;

    match action {
        crate::ThreadCmd::List => {
            let threads = store.worker_threads_in_current_scope_family()?;
            println!("{}", worker_thread_list(&cfg, &threads));
            Ok(())
        }
        crate::ThreadCmd::Show { role } => {
            let threads = store.worker_threads_in_current_scope_family()?;
            let Some(thread) = find_worker_thread(&threads, &role) else {
                if cfg.roles.contains_key(&role) {
                    println!("{}", missing_worker_thread_detail(&cfg, &role));
                    return Ok(());
                }
                anyhow::bail!(
                    "worker thread not found for role '{role}'. Available worker roles: {}",
                    available_worker_thread_roles(&cfg, &threads)
                );
            };
            println!("{}", worker_thread_detail(&cfg, thread));
            Ok(())
        }
        crate::ThreadCmd::Clear { role } => {
            let threads = store.worker_threads_in_current_scope_family()?;
            let Some(thread) = find_worker_thread(&threads, &role) else {
                if cfg.roles.contains_key(&role) {
                    println!(
                        "Worker thread {}\n  status: no worker thread yet\n  native session: none\n\nNothing to clear.",
                        role
                    );
                    return Ok(());
                }
                anyhow::bail!(
                    "worker thread not found for role '{role}'. Available worker roles: {}",
                    available_worker_thread_roles(&cfg, &threads)
                );
            };
            let previous = thread
                .native_session_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .unwrap_or("none")
                .to_string();
            store.clear_worker_thread_native_session(thread.id)?;
            println!(
                "Worker thread reset\n  role: {}\n  Tiffany thread: {}\n  cleared native session: {}\n  next run: starts a fresh native {} session\n\nKept: worker thread id, last Tiffany session, and conversation context.",
                thread.role,
                thread.id,
                previous,
                thread.agent
            );
            Ok(())
        }
        crate::ThreadCmd::Export {
            role,
            format,
            out,
            clipboard,
        } => {
            let threads = store.worker_threads_in_current_scope_family()?;
            let Some(thread) = find_worker_thread(&threads, &role) else {
                if cfg.roles.contains_key(&role) {
                    anyhow::bail!("worker thread '{role}' has no captured Tiffany session yet");
                }
                anyhow::bail!(
                    "worker thread not found for role '{role}'. Available worker roles: {}",
                    available_worker_thread_roles(&cfg, &threads)
                );
            };
            println!(
                "{}",
                export_worker_thread_session(&store, thread, format, out.as_deref(), clipboard)?
            );
            Ok(())
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct NativeChatStoreFile {
    #[serde(default)]
    conversations: Vec<NativeChatConversationFile>,
}

#[derive(Debug, serde::Deserialize)]
struct NativeChatConversationFile {
    id: String,
    cwd: String,
    #[serde(default)]
    created_at_unix: u64,
    #[serde(default)]
    updated_at_unix: u64,
    #[serde(default)]
    turns: Vec<NativeChatTurnFile>,
}

#[derive(Debug, serde::Deserialize)]
struct NativeChatTurnFile {
    user_prompt: String,
    result: String,
    #[serde(default)]
    captured_at_unix: u64,
    #[serde(default)]
    events: Vec<NativeChatEventFile>,
}

#[derive(Debug, serde::Deserialize)]
struct NativeChatEventFile {
    role: String,
    status: String,
    title: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    worker_role: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    worker_thread_id: Option<String>,
    #[serde(default)]
    native_session_id: Option<String>,
}

fn import_native_sessions_file(
    store: &orchestrator::core::session_store::SessionStore,
    path: &Path,
) -> Result<NativeImportReport> {
    let body =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: NativeChatStoreFile =
        serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
    let mut report = NativeImportReport {
        conversations: 0,
        turns: 0,
        events: 0,
    };
    for conversation in raw.conversations {
        let Some(conversation) = native_conversation_from_file(conversation) else {
            continue;
        };
        let imported = store.upsert_native_conversation(&conversation)?;
        report.conversations += imported.conversations;
        report.turns += imported.turns;
        report.events += imported.events;
    }
    Ok(report)
}

fn native_conversation_from_file(
    conversation: NativeChatConversationFile,
) -> Option<NativeConversation> {
    let id = trimmed_nonempty(conversation.id)?;
    let cwd = trimmed_nonempty(conversation.cwd)?;
    let turns = conversation
        .turns
        .into_iter()
        .enumerate()
        .filter_map(|(idx, turn)| native_turn_from_file(idx as u32, turn))
        .collect::<Vec<_>>();
    (!turns.is_empty()).then_some(NativeConversation {
        id,
        cwd,
        created_at_unix: conversation.created_at_unix,
        updated_at_unix: conversation.updated_at_unix,
        turns,
    })
}

fn native_turn_from_file(turn_index: u32, turn: NativeChatTurnFile) -> Option<NativeTurn> {
    let user_prompt = trimmed_nonempty(turn.user_prompt)?;
    let result = trimmed_nonempty(turn.result)?;
    let events = turn
        .events
        .into_iter()
        .enumerate()
        .filter_map(|(idx, event)| native_event_from_file(idx as u32, event))
        .collect::<Vec<_>>();
    Some(NativeTurn {
        turn_index,
        user_prompt,
        result,
        captured_at_unix: turn.captured_at_unix,
        events,
    })
}

fn native_event_from_file(event_index: u32, event: NativeChatEventFile) -> Option<NativeEvent> {
    Some(NativeEvent {
        event_index,
        role: trimmed_nonempty(event.role)?,
        status: trimmed_nonempty(event.status)?,
        title: trimmed_nonempty(event.title)?,
        kind: event.kind.and_then(trimmed_nonempty),
        content: event.content.and_then(trimmed_nonempty),
        agent: event.agent.and_then(trimmed_nonempty),
        worker_role: event.worker_role.and_then(trimmed_nonempty),
        model: event.model.and_then(trimmed_nonempty),
        provider: event.provider.and_then(trimmed_nonempty),
        task_id: event.task_id.and_then(trimmed_nonempty),
        worker_thread_id: event.worker_thread_id.and_then(trimmed_nonempty),
        native_session_id: event.native_session_id.and_then(trimmed_nonempty),
    })
}

#[derive(Clone, Debug, Default)]
struct NativeHistoryCliFilter {
    role: Option<String>,
    thread: Option<String>,
    native: Option<String>,
    kind: Option<String>,
}

impl NativeHistoryCliFilter {
    fn new(
        role: Option<String>,
        thread: Option<String>,
        native: Option<String>,
        kind: Option<String>,
    ) -> Self {
        Self {
            role: role.and_then(trimmed_nonempty),
            thread: thread.and_then(trimmed_nonempty),
            native: native.and_then(trimmed_nonempty),
            kind: kind.and_then(trimmed_nonempty),
        }
    }

    fn is_active(&self) -> bool {
        self.role.is_some() || self.thread.is_some() || self.native.is_some() || self.kind.is_some()
    }

    fn display(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(role) = &self.role {
            parts.push(format!("role={role}"));
        }
        if let Some(thread) = &self.thread {
            parts.push(format!("thread={thread}"));
        }
        if let Some(native) = &self.native {
            parts.push(format!("native={native}"));
        }
        if let Some(kind) = &self.kind {
            parts.push(format!("kind={kind}"));
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    fn matches_event(&self, event: &NativeEvent) -> bool {
        if let Some(role) = &self.role {
            let matches_role = event.worker_role.as_deref() == Some(role.as_str())
                || event.agent.as_deref() == Some(role.as_str())
                || event.role == *role;
            if !matches_role {
                return false;
            }
        }
        if let Some(thread) = &self.thread {
            let matches_thread = event
                .worker_thread_id
                .as_deref()
                .is_some_and(|id| id == thread || id.starts_with(thread));
            if !matches_thread {
                return false;
            }
        }
        if let Some(native) = &self.native {
            let matches_native = event
                .native_session_id
                .as_deref()
                .is_some_and(|id| id == native || id.starts_with(native));
            if !matches_native {
                return false;
            }
        }
        if let Some(kind) = &self.kind {
            let matches_kind = event
                .kind
                .as_deref()
                .is_some_and(|event_kind| native_history_cli_kind_matches(event_kind, kind));
            if !matches_kind {
                return false;
            }
        }
        true
    }
}

fn native_history_cli_kind_matches(event_kind: &str, filter: &str) -> bool {
    let event_kind = normalize_native_history_cli_kind(event_kind);
    let filter = normalize_native_history_cli_kind(filter);
    event_kind == filter || event_kind.starts_with(&filter)
}

fn normalize_native_history_cli_kind(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

fn filter_native_conversation_for_cli(
    mut conversation: NativeConversation,
    filter: &NativeHistoryCliFilter,
) -> NativeConversation {
    if !filter.is_active() {
        return conversation;
    }
    conversation.turns = conversation
        .turns
        .into_iter()
        .filter_map(|mut turn| {
            turn.events.retain(|event| filter.matches_event(event));
            (!turn.events.is_empty()).then_some(turn)
        })
        .collect();
    conversation
}

fn format_native_history_cli(
    conversation: Option<&NativeConversation>,
    cwd: &str,
    filter: &NativeHistoryCliFilter,
) -> String {
    let Some(conversation) = conversation else {
        let mut out = format!("Native history\n  cwd: {cwd}\n  status: no saved native history");
        out.push_str(
            "\n\nNext:\n  tiffany-loop /history sync\n  orchestrator sessions import-native",
        );
        return out;
    };

    let event_count = conversation
        .turns
        .iter()
        .map(|turn| turn.events.len())
        .sum::<usize>();
    let mut out = format!(
        "Native history\n  cwd: {}\n  session: {}\n  turns: {}  events: {}",
        conversation.cwd,
        conversation.id,
        conversation.turns.len(),
        event_count
    );
    if let Some(filter) = filter.display() {
        out.push_str(&format!("\n  filter: {filter}"));
    }
    if conversation.turns.is_empty() {
        out.push_str("\n\nNo matching native events.");
        out.push_str("\n\nNext:\n  orchestrator sessions native-history --format text");
        return out;
    }

    for turn in &conversation.turns {
        out.push_str(&format!(
            "\n\nTurn {}\n  user: {}\n  result: {}",
            turn.turn_index + 1,
            truncate_for_cli(&one_line_cli(&turn.user_prompt), 120),
            truncate_for_cli(&one_line_cli(&turn.result), 140)
        ));
        for event in turn.events.iter().take(40) {
            out.push_str(&format_native_event_cli(event));
        }
        if turn.events.len() > 40 {
            out.push_str(&format!(
                "\n  ... {} more event(s); use --format json for complete data",
                turn.events.len() - 40
            ));
        }
    }

    out.push_str("\n\nNext:");
    out.push_str("\n  tiffany-loop /history full");
    out.push_str("\n  tiffany-loop /history compact");
    if let Some(role) = filter
        .role
        .as_deref()
        .or_else(|| first_native_history_role(conversation))
    {
        out.push_str(&format!("\n  tiffany-loop /continue open {role}"));
    }
    out
}

fn format_native_event_cli(event: &NativeEvent) -> String {
    let role = event
        .worker_role
        .as_deref()
        .or(event.agent.as_deref())
        .unwrap_or(event.role.as_str());
    let kind = event.kind.as_deref().unwrap_or(event.status.as_str());
    let mut out = format!(
        "\n  - {} · {} · {}",
        kind,
        role,
        truncate_for_cli(&event.title, 96)
    );
    if let Some(thread) = event.worker_thread_id.as_deref() {
        out.push_str(&format!("\n    thread: {thread}"));
    }
    if let Some(native) = event.native_session_id.as_deref() {
        out.push_str(&format!("\n    native: {native}"));
    }
    if let Some(content) = event.content.as_deref() {
        let content = orchestrator::agent_events::humanize_jsonish(content, 2_000);
        for line in content.lines().take(8) {
            out.push_str(&format!("\n    {}", truncate_for_cli(line, 160)));
        }
        let hidden = content.lines().count().saturating_sub(8);
        if hidden > 0 {
            out.push_str(&format!(
                "\n    ... {hidden} more line(s); use --format json for full content"
            ));
        }
    }
    out
}

fn one_line_cli(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_native_history_role(conversation: &NativeConversation) -> Option<&str> {
    conversation.turns.iter().find_map(|turn| {
        turn.events.iter().find_map(|event| {
            event
                .worker_role
                .as_deref()
                .or(event.agent.as_deref())
                .or(Some(event.role.as_str()))
                .filter(|role| !role.trim().is_empty())
        })
    })
}

fn trimmed_nonempty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn default_native_sessions_path() -> Option<PathBuf> {
    native_home_candidates()
        .into_iter()
        .map(|home| home.join(TIFFANY_NATIVE_SESSIONS_FILE))
        .find(|path| path.is_file())
}

fn native_home_candidates() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    for env in ["TIFFANY_HOME", "CODEX_HOME"] {
        if let Some(path) = std::env::var_os(env).filter(|value| !value.is_empty()) {
            push_unique_path(&mut homes, PathBuf::from(path));
        }
    }
    if let Some(home) = dirs::home_dir() {
        push_unique_path(&mut homes, home.join(".tiffany"));
        push_unique_path(&mut homes, home.join(".codex"));
    }
    homes
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn export_worker_thread_session(
    store: &orchestrator::core::session_store::SessionStore,
    thread: &WorkerThread,
    format: crate::SessionExportFormatArg,
    out: Option<&Path>,
    clipboard: bool,
) -> Result<String> {
    let session_id = thread.last_session_id.with_context(|| {
        format!(
            "worker thread '{}' has no captured Tiffany session yet",
            thread.role
        )
    })?;
    let session = store
        .get_many(&[session_id])?
        .into_iter()
        .next()
        .with_context(|| {
            format!(
                "last Tiffany session not found for role '{}': {session_id}",
                thread.role
            )
        })?;
    let format = match format {
        crate::SessionExportFormatArg::Markdown => SessionExportFormat::Markdown,
        crate::SessionExportFormatArg::Html => SessionExportFormat::Html,
    };

    if clipboard {
        let body = orchestrator::session_export::render_session_markdown(store, &session)?;
        copy_to_clipboard_cli(&body)?;
        return Ok(format!(
            "Worker thread session exported\n  role: {}\n  Tiffany thread: {}\n  session: {}\n  target: clipboard\n  bytes: {}\n\nAction: paste into Claude Code or another review tool to continue manually.",
            thread.role,
            thread.id,
            orchestrator::session_export::short_session_id(&session),
            body.len()
        ));
    }

    let path = if let Some(path) = out {
        let body = match format {
            SessionExportFormat::Markdown => {
                orchestrator::session_export::render_session_markdown(store, &session)?
            }
            SessionExportFormat::Html => {
                orchestrator::session_export::render_session_html(store, &session)?
            }
        };
        write_session_export(path, &body)?;
        path.to_path_buf()
    } else {
        orchestrator::session_export::export_session_to_file(store, &session, format)?.path
    };

    Ok(format!(
        "Worker thread session exported\n  role: {}\n  Tiffany thread: {}\n  session: {}\n  target: {}\n\nAction: open the export for full selectable history, or paste it into the native worker to continue manually.",
        thread.role,
        thread.id,
        orchestrator::session_export::short_session_id(&session),
        path.display()
    ))
}

fn worker_thread_list(cfg: &Config, threads: &[WorkerThread]) -> String {
    let mut out = format!(
        "Worker threads\n  stored threads: {}\n\nRoles:",
        threads.len()
    );
    let roles = ordered_worker_thread_roles(cfg, threads);
    if roles.is_empty() {
        out.push_str("\n  no worker roles configured");
    } else {
        for role in roles {
            out.push('\n');
            if let Some(thread) = find_worker_thread(threads, &role) {
                out.push_str(&worker_thread_summary(cfg, thread));
            } else {
                out.push_str(&missing_worker_thread_summary(cfg, &role));
            }
        }
    }
    out.push_str(
        "\n\nDetails: orchestrator thread show <role>  Export: orchestrator thread export <role>  Fresh start: orchestrator thread clear <role>",
    );
    out
}

fn ordered_worker_thread_roles(cfg: &Config, threads: &[WorkerThread]) -> Vec<String> {
    let mut roles = cfg
        .roles
        .keys()
        .filter(|role| role.contains("worker"))
        .cloned()
        .collect::<Vec<_>>();
    roles.sort_by_key(|role| worker_role_sort_key(role));
    for thread in threads {
        if !roles.iter().any(|role| role == &thread.role) {
            roles.push(thread.role.clone());
        }
    }
    roles
}

fn worker_role_sort_key(role: &str) -> (u8, String) {
    match role {
        "worker-cc" => (0, role.to_string()),
        "worker-codex" => (1, role.to_string()),
        "worker-gemini" => (2, role.to_string()),
        _ => (3, role.to_string()),
    }
}

fn find_worker_thread<'a>(threads: &'a [WorkerThread], selector: &str) -> Option<&'a WorkerThread> {
    let role_matches = threads
        .iter()
        .filter(|thread| thread.role == selector)
        .collect::<Vec<_>>();
    if !role_matches.is_empty() {
        return role_matches
            .iter()
            .copied()
            .find(|thread| {
                thread
                    .native_session_id
                    .as_deref()
                    .is_some_and(|id| !id.trim().is_empty())
            })
            .or_else(|| role_matches.first().copied());
    }

    threads
        .iter()
        .find(|thread| thread.id.to_string().starts_with(selector))
        .or_else(|| {
            threads.iter().find(|thread| {
                thread
                    .native_session_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with(selector))
            })
        })
}

fn worker_thread_summary(cfg: &Config, thread: &WorkerThread) -> String {
    let native = thread
        .native_session_id
        .as_deref()
        .map(short_text_id)
        .unwrap_or_else(|| "none".to_string());
    let last = thread
        .last_session_id
        .as_ref()
        .map(short_uuid)
        .unwrap_or_else(|| "none".to_string());
    format!(
        "  ● {:<18} {} · {} · scope {} · thread {} · native {} · last {}",
        thread.role,
        thread.runtime,
        worker_thread_model_label(cfg, thread),
        short_worker_thread_scope(&thread.scope),
        short_uuid(&thread.id),
        native,
        last
    )
}

fn missing_worker_thread_summary(cfg: &Config, role: &str) -> String {
    let detail = cfg
        .roles
        .get(role)
        .map(|role_cfg| format!("{} · {}", role_cfg.runtime, role_cfg.model))
        .unwrap_or_else(|| "not configured".to_string());
    format!("  ○ {:<18} {} · no worker thread yet", role, detail)
}

fn worker_thread_detail(cfg: &Config, thread: &WorkerThread) -> String {
    worker_thread_detail_card(cfg, thread)
}

fn worker_thread_detail_card(cfg: &Config, thread: &WorkerThread) -> String {
    let native_session = thread
        .native_session_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or("none");
    let last_session = thread
        .last_session_id
        .as_ref()
        .map(uuid::Uuid::to_string)
        .unwrap_or_else(|| "none".to_string());
    let worktree = thread
        .worktree_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "none".to_string());
    let status = worker_thread_status_hint(thread);
    let resume = native_thread_resume_command(cfg, thread);
    let handoff = native_thread_handoff_command(cfg, thread);
    let tui_resume = cli_tui_continue_command(thread);
    let legacy_handoff = legacy_handoff_continue_command(thread);
    format!(
        "Worker thread {}\nSession card\n  role: {}\n  scope: {}\n  status: {}\n  reuse: same role in the same project keeps Tiffany thread, native session, and worktree\n\nBinding\n  runtime: {}\n  agent: {}\n  model: {}\n  provider: {}\n\nNative session\n  Tiffany thread: {}\n  native session: {}\n  last Tiffany session: {}\n  worktree: {}\n\nCommands\n  native resume: {}\n  native handoff: {}\n  TUI resume: {}\n  legacy handoff: {}\n\nTimestamps\n  created: {}\n  updated: {}\n\nStatus: {}\nAction: open tiffany-loop and run {} to continue in the native CLI.\nAction: {} saves a handoff package in the legacy terminal TUI.\nAction: orchestrator thread export {} writes the last Tiffany session for handoff.\nAction: orchestrator thread clear {} resets only the native CLI session id for a fresh next run.",
        short_uuid(&thread.id),
        thread.role,
        thread.scope,
        status,
        thread.runtime,
        thread.agent,
        worker_thread_model_label(cfg, thread),
        thread.provider.as_deref().unwrap_or("none"),
        thread.id,
        native_session,
        last_session,
        worktree,
        resume,
        handoff,
        tui_resume,
        legacy_handoff,
        thread.created_at.to_rfc3339(),
        thread.updated_at.to_rfc3339(),
        status,
        tui_resume,
        legacy_handoff,
        thread.role,
        thread.role
    )
}

fn missing_worker_thread_detail(cfg: &Config, role: &str) -> String {
    let detail = cfg
        .roles
        .get(role)
        .map(|role_cfg| format!("{} · {}", role_cfg.runtime, role_cfg.model))
        .unwrap_or_else(|| "configured role".to_string());
    format!(
        "Worker thread {}\n  role: {}\n  configured: {}\n  native session: none\n  status: no worker thread yet\n\nNext: run a task with this role to create and persist one.",
        role, role, detail
    )
}

fn worker_thread_model_label(cfg: &Config, thread: &WorkerThread) -> String {
    if let Some(provider) = thread
        .provider
        .as_deref()
        .filter(|provider| !provider.is_empty())
    {
        return format!("{provider}/{}", thread.model);
    }
    cfg.models
        .iter()
        .find(|model| model.id == thread.model)
        .map(|model| format!("{}/{}", model.provider, model.name))
        .unwrap_or_else(|| thread.model.clone())
}

fn native_thread_resume_command(cfg: &Config, thread: &WorkerThread) -> String {
    let Some(native_session_id) = thread
        .native_session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return "none".to_string();
    };
    let Some(runtime) = native_thread_runtime(thread) else {
        return "none".to_string();
    };
    let binary = shell_quote_arg(&native_thread_binary(cfg, thread, runtime));
    let native_session_id = shell_quote_arg(native_session_id);
    match runtime {
        roles::cli_subprocess::RoleCliRuntime::ClaudeCode => {
            format!("{binary} --resume {native_session_id}")
        }
        roles::cli_subprocess::RoleCliRuntime::Codex => {
            format!("{binary} exec resume {native_session_id}")
        }
        roles::cli_subprocess::RoleCliRuntime::Gemini => {
            format!("{binary} --resume {native_session_id}")
        }
    }
}

fn native_thread_handoff_command(cfg: &Config, thread: &WorkerThread) -> String {
    let cwd = thread
        .worktree_path
        .as_deref()
        .map(shell_quote_path)
        .unwrap_or_else(|| ".".to_string());
    let command = native_thread_interactive_resume_command(cfg, thread);
    if command == "none" {
        return "none".to_string();
    }
    format!("cd {cwd} && {command}")
}

fn native_thread_interactive_resume_command(cfg: &Config, thread: &WorkerThread) -> String {
    let Some(native_session_id) = thread
        .native_session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return "none".to_string();
    };
    let Some(runtime) = native_thread_runtime(thread) else {
        return "none".to_string();
    };
    let binary = shell_quote_arg(&native_thread_binary(cfg, thread, runtime));
    let native_session_id = shell_quote_arg(native_session_id);
    match runtime {
        roles::cli_subprocess::RoleCliRuntime::ClaudeCode => {
            format!("{binary} --resume {native_session_id}")
        }
        roles::cli_subprocess::RoleCliRuntime::Codex => {
            format!("{binary} resume {native_session_id}")
        }
        roles::cli_subprocess::RoleCliRuntime::Gemini => {
            format!("{binary} --resume {native_session_id}")
        }
    }
}

fn cli_tui_continue_command(thread: &WorkerThread) -> String {
    if thread.agent == "claude-code" || thread.runtime == "claude-code" {
        format!("/continue open {}", thread.role)
    } else if thread.agent == "codex" || thread.runtime == "codex" {
        format!("/continue open {}", thread.role)
    } else if thread.agent == "gemini" || thread.runtime == "gemini" {
        format!("/continue open {}", thread.role)
    } else {
        "none".to_string()
    }
}

fn legacy_handoff_continue_command(thread: &WorkerThread) -> String {
    if thread.agent == "claude-code" || thread.runtime == "claude-code" {
        "/continue claude".to_string()
    } else if thread.agent == "codex" || thread.runtime == "codex" {
        "/continue codex".to_string()
    } else if thread.agent == "gemini" || thread.runtime == "gemini" {
        "/continue gemini".to_string()
    } else {
        "none".to_string()
    }
}

fn native_thread_runtime(thread: &WorkerThread) -> Option<roles::cli_subprocess::RoleCliRuntime> {
    roles::cli_subprocess::RoleCliRuntime::from_runtime_id(&thread.runtime)
        .or_else(|| roles::cli_subprocess::RoleCliRuntime::from_runtime_id(&thread.agent))
}

fn native_thread_binary(
    cfg: &Config,
    thread: &WorkerThread,
    runtime: roles::cli_subprocess::RoleCliRuntime,
) -> String {
    cfg.runtimes
        .get(&thread.runtime)
        .and_then(|runtime| runtime.binary.as_deref())
        .or_else(|| {
            cfg.runtimes
                .get(&thread.agent)
                .and_then(|runtime| runtime.binary.as_deref())
        })
        .map(str::trim)
        .filter(|binary| !binary.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| native_thread_default_binary(runtime).to_string())
}

fn native_thread_default_binary(runtime: roles::cli_subprocess::RoleCliRuntime) -> &'static str {
    match runtime {
        roles::cli_subprocess::RoleCliRuntime::ClaudeCode => "claude",
        roles::cli_subprocess::RoleCliRuntime::Codex => "codex",
        roles::cli_subprocess::RoleCliRuntime::Gemini => "gemini",
    }
}

fn shell_quote_path(path: &Path) -> String {
    shell_quote_arg(&path.display().to_string())
}

fn shell_quote_arg(value: &str) -> String {
    if value.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '=' | '+')
    }) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn worker_thread_status_hint(thread: &WorkerThread) -> &'static str {
    if thread.agent == "gemini" || thread.runtime == "gemini" {
        if thread.native_session_id.is_some() {
            return "ready for Gemini native resume; Tiffany will reuse latest/index for the same role";
        }
        return "no Gemini native session captured yet; next successful worker run starts fresh";
    }
    if thread.native_session_id.is_some() {
        "ready for native resume; Tiffany will reuse this session for the same role"
    } else {
        "no native session captured yet; next successful worker run starts fresh"
    }
}

fn available_worker_thread_roles(cfg: &Config, threads: &[WorkerThread]) -> String {
    let roles = ordered_worker_thread_roles(cfg, threads);
    if roles.is_empty() {
        "(none)".to_string()
    } else {
        roles.join(", ")
    }
}

fn short_uuid(id: &uuid::Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn short_text_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.chars().count() <= 12 {
        trimmed.to_string()
    } else {
        format!("{}...", trimmed.chars().take(12).collect::<String>())
    }
}

fn short_worker_thread_scope(scope: &str) -> String {
    scope
        .strip_prefix("cwd:")
        .and_then(|path| std::path::Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .map(|name| format!("cwd:{name}"))
        .unwrap_or_else(|| scope.to_string())
}

fn print_roles(config_path: &Path, selected_role: Option<&str>) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let threads = role_worker_threads_for_cli(&cfg);
    if let Some(role) = selected_role {
        let Some(role_cfg) = cfg.roles.get(role) else {
            anyhow::bail!(
                "unknown role '{}'. Available: {}",
                role,
                available_roles_for_cli(&cfg)
            );
        };
        let thread = threads
            .as_deref()
            .and_then(|threads| find_worker_thread(threads, role));
        println!("{}", role_detail_for_cli(&cfg, role, role_cfg, thread));
        return Ok(());
    }

    println!("Registered roles:");
    let mut roles = cfg.roles.iter().collect::<Vec<_>>();
    roles.sort_by(|a, b| a.0.cmp(b.0));
    if roles.is_empty() {
        println!("  (none)");
    }
    for (role, role_cfg) in roles {
        let thread = threads
            .as_deref()
            .and_then(|threads| find_worker_thread(threads, role));
        println!("  {}", role_detail_for_cli(&cfg, role, role_cfg, thread));
    }
    println!(
        "\nRegister: orchestrator roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime-id>"
    );
    Ok(())
}

fn role_worker_threads_for_cli(cfg: &Config) -> Option<Vec<WorkerThread>> {
    SessionStore::open(&cfg.behavior.session_log_dir, &cfg.behavior.db_path)
        .and_then(|store| store.worker_threads_in_current_scope_family())
        .ok()
}

fn print_role_options(config_path: &Path) -> Result<()> {
    let cfg = Config::load(config_path)?;
    println!("Role options");
    println!("  providers: {}", available_providers_for_cli(&cfg));
    println!("  runtimes: {}", available_runtimes_for_cli(&cfg));
    println!();
    println!("Models:");
    if cfg.models.is_empty() {
        println!("  (none)");
    }
    let mut models = cfg.models.iter().collect::<Vec<_>>();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    for model in models {
        println!("  {}", role_model_option_for_cli(&cfg, model));
    }
    println!();
    println!("Runtime presets:");
    for preset in role_runtime_presets_for_cli(&cfg) {
        println!("  {preset}");
    }
    println!();
    println!("Register: orchestrator roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime-id>");
    println!("Profile: orchestrator roles profile dev --worker-cc provider/model@claude-code --worker-codex provider/model@codex --worker-gemini provider/model@gemini");
    Ok(())
}

fn role_model_option_for_cli(cfg: &Config, model: &orchestrator::config::ModelConfig) -> String {
    let provider_ready = cfg.providers.contains_key(&model.provider);
    let runtimes = compatible_runtime_ids_for_model(cfg, model);
    let teams = if runtimes.iter().any(|runtime| {
        cfg.runtime_config(runtime)
            .is_some_and(|runtime| runtime.supports_agent_teams)
    }) {
        "auto"
    } else {
        "off"
    };
    let health = if !provider_ready {
        format!("provider-missing:{}", model.provider)
    } else if runtimes.is_empty() {
        "runtime-missing".to_string()
    } else {
        "ready".to_string()
    };
    let symbol = if health == "ready" { "✓" } else { "⚠" };
    let runtime_label = if runtimes.is_empty() {
        "-".to_string()
    } else {
        runtimes.join(",")
    };
    let roles = suggested_roles_for_runtime_ids(&runtimes);
    format!(
        "{symbol} {:<18} provider={:<12} api_model={:<28} runtimes={:<22} teams={:<4} roles={:<34} health={}",
        model.id, model.provider, model.name, runtime_label, teams, roles, health
    )
}

fn compatible_runtime_ids_for_model(
    cfg: &Config,
    model: &orchestrator::config::ModelConfig,
) -> Vec<String> {
    let mut runtimes = cfg
        .runtimes
        .keys()
        .filter(|runtime| {
            runtime_target_for_id(runtime)
                .is_some_and(|target| model_supports_runtime(cfg, model, target))
        })
        .cloned()
        .collect::<Vec<_>>();
    runtimes.sort();
    runtimes
}

fn runtime_target_for_id(runtime: &str) -> Option<RuntimeTarget> {
    match roles::cli_subprocess::RoleCliRuntime::from_runtime_id(runtime)? {
        roles::cli_subprocess::RoleCliRuntime::ClaudeCode => Some(RuntimeTarget::Claude),
        roles::cli_subprocess::RoleCliRuntime::Codex => Some(RuntimeTarget::Codex),
        roles::cli_subprocess::RoleCliRuntime::Gemini => Some(RuntimeTarget::Gemini),
    }
}

fn suggested_roles_for_runtime_ids(runtimes: &[String]) -> String {
    let mut roles = Vec::new();
    if runtimes
        .iter()
        .any(|runtime| runtime == "claude-code" || runtime == "claude")
    {
        roles.push("worker-cc");
    }
    if runtimes.iter().any(|runtime| runtime == "codex") {
        roles.extend(["worker-codex", "planner", "critic", "reviewer"]);
    }
    if runtimes
        .iter()
        .any(|runtime| runtime == "gemini" || runtime == "gemini-cli")
    {
        roles.push("worker-gemini");
    }
    if roles.is_empty() {
        "-".to_string()
    } else {
        roles.join(",")
    }
}

fn role_runtime_presets_for_cli(cfg: &Config) -> Vec<String> {
    let mut runtimes = cfg.runtimes.iter().collect::<Vec<_>>();
    runtimes.sort_by(|a, b| a.0.cmp(b.0));
    runtimes
        .into_iter()
        .map(|(id, runtime)| {
            let teams = if runtime.supports_agent_teams {
                "teams=auto"
            } else {
                "teams=off"
            };
            let binary = runtime
                .binary
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("default");
            format!(
                "• {:<12} type={:<10} binary={:<18} {}",
                id, runtime.kind, binary, teams
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn register_role(
    config_path: &Path,
    role: &str,
    model: Option<&str>,
    runtime: &str,
    provider: Option<&str>,
    model_name: Option<&str>,
    agent_teams: bool,
    no_agent_teams: bool,
) -> Result<()> {
    if agent_teams && no_agent_teams {
        anyhow::bail!("--agent-teams and --no-agent-teams cannot both be set");
    }
    let cfg = Config::load(config_path)?;
    let prepared = prepare_role_registration(
        &cfg,
        role,
        RoleBindingSpec {
            model,
            runtime,
            provider,
            model_name,
        },
        AgentTeamsChoice {
            agent_teams,
            no_agent_teams,
        },
    )?;

    if let Some(model_cfg) = &prepared.model_write {
        Config::write_model_to_config_file(config_path, model_cfg)?;
        println!(
            "✓ model {} registered: provider={} name={}",
            model_cfg.id, model_cfg.provider, model_cfg.name
        );
    }

    Config::write_role_to_config_file(config_path, role, &prepared.role_cfg)?;
    println!("✓ role {} registered", role);
    println!("  model: {}", prepared.role_cfg.model);
    println!("  runtime: {}", prepared.role_cfg.runtime);
    println!("  agent teams: {}", prepared.role_cfg.agent_teams);
    match role {
        "planner" | "critic" | "reviewer" => {
            println!(
                "\nThis updates the fixed {} slot used by future orchestrator runs.",
                role
            );
            println!("Restart the tiffany-loop TUI if it is already open.");
        }
        _ => {
            println!("\nUse it with:");
            println!("  orchestrator run \"...\" --worker {}", role);
            println!("  /roles use {}   (legacy terminal chat)", role);
        }
    }
    Ok(())
}

fn delete_role(config_path: &Path, role: &str) -> Result<()> {
    if Config::delete_role_from_config_file(config_path, role)? {
        println!("✓ role {} deleted", role);
        println!("  kept: models, providers, runtimes, worker threads, and session history");
        println!("  next: orchestrator roles list");
        println!("  optional: orchestrator thread clear {role}");
    } else {
        let cfg = Config::load(config_path)?;
        println!("role {} not found", role);
        println!("  available: {}", available_roles_for_cli(&cfg));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct RoleBindingSpec<'a> {
    model: Option<&'a str>,
    runtime: &'a str,
    provider: Option<&'a str>,
    model_name: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct AgentTeamsChoice {
    agent_teams: bool,
    no_agent_teams: bool,
}

struct PreparedRoleRegistration {
    role: String,
    model_write: Option<orchestrator::config::ModelConfig>,
    role_cfg: orchestrator::config::RoleConfig,
}

fn prepare_role_registration(
    cfg: &Config,
    role: &str,
    spec: RoleBindingSpec<'_>,
    teams_choice: AgentTeamsChoice,
) -> Result<PreparedRoleRegistration> {
    let Some(runtime_cfg) = cfg.runtimes.get(spec.runtime) else {
        anyhow::bail!(
            "unknown runtime '{}'. Available: {}",
            spec.runtime,
            available_runtimes_for_cli(cfg)
        );
    };

    let resolved_model_id = resolve_role_model_id(cfg, spec.model, spec.provider, spec.model_name)?;
    let existing_model = cfg.models.iter().find(|m| m.id == resolved_model_id);
    let model_write = match existing_model {
        Some(existing) if spec.provider.is_some() || spec.model_name.is_some() => {
            let provider = spec.provider.unwrap_or(existing.provider.as_str());
            if !cfg.providers.contains_key(provider) {
                anyhow::bail!(
                    "unknown provider '{}'. Available: {}",
                    provider,
                    available_providers_for_cli(cfg)
                );
            }
            Some(orchestrator::config::ModelConfig {
                id: resolved_model_id.clone(),
                provider: provider.to_string(),
                name: spec
                    .model_name
                    .unwrap_or(existing.name.as_str())
                    .to_string(),
            })
        }
        Some(_) => None,
        None => {
            let Some(provider) = spec.provider else {
                anyhow::bail!(
                    "unknown model '{}'. Available: {}\nTo register it inline, use provider/model-name@runtime.",
                    resolved_model_id,
                    available_models_for_cli(cfg)
                );
            };
            if !cfg.providers.contains_key(provider) {
                anyhow::bail!(
                    "unknown provider '{}'. Available: {}",
                    provider,
                    available_providers_for_cli(cfg)
                );
            }
            Some(orchestrator::config::ModelConfig {
                id: resolved_model_id.clone(),
                provider: provider.to_string(),
                name: spec
                    .model_name
                    .unwrap_or(resolved_model_id.as_str())
                    .to_string(),
            })
        }
    };

    let teams = if teams_choice.no_agent_teams {
        false
    } else if teams_choice.agent_teams {
        true
    } else {
        default_agent_teams(role, spec.runtime, runtime_cfg)
    };
    Ok(PreparedRoleRegistration {
        role: role.to_string(),
        model_write,
        role_cfg: orchestrator::config::RoleConfig {
            model: resolved_model_id,
            runtime: spec.runtime.to_string(),
            agent_teams: teams,
        },
    })
}

fn save_role_profile(
    config_path: &Path,
    name: &str,
    bindings: &[(&str, Option<&str>)],
    dry_run: bool,
) -> Result<()> {
    let mut cfg = Config::load(config_path)?;
    let mut prepared = Vec::new();
    for (role, raw) in bindings {
        let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            continue;
        };
        let parsed = parse_role_profile_binding(raw)
            .with_context(|| format!("parsing binding for role '{role}'"))?;
        let item = prepare_role_registration(
            &cfg,
            role,
            parsed,
            AgentTeamsChoice {
                agent_teams: false,
                no_agent_teams: false,
            },
        )?;
        if let Some(model_cfg) = &item.model_write {
            upsert_config_model(&mut cfg, model_cfg.clone());
        }
        cfg.roles.insert(item.role.clone(), item.role_cfg.clone());
        prepared.push(item);
    }

    if prepared.is_empty() {
        anyhow::bail!(
            "profile '{name}' has no role bindings; pass at least one of --planner, --critic, --reviewer, --worker-cc, --worker-codex, --worker-gemini"
        );
    }

    if dry_run {
        println!("Role profile dry-run: {name}");
    } else {
        println!("Role profile saved: {name}");
    }
    for item in &prepared {
        if !dry_run {
            if let Some(model_cfg) = &item.model_write {
                Config::write_model_to_config_file(config_path, model_cfg)?;
            }
            Config::write_role_to_config_file(config_path, &item.role, &item.role_cfg)?;
        }
        println!(
            "  ✓ {:<13} model={} runtime={} agent_teams={}",
            item.role, item.role_cfg.model, item.role_cfg.runtime, item.role_cfg.agent_teams
        );
        if let Some(model_cfg) = &item.model_write {
            println!(
                "    model {} -> {}/{}",
                model_cfg.id, model_cfg.provider, model_cfg.name
            );
        }
    }
    println!("\nNext: orchestrator roles list");
    println!("Next: orchestrator doctor");
    Ok(())
}

fn upsert_config_model(cfg: &mut Config, model_cfg: orchestrator::config::ModelConfig) {
    if let Some(existing) = cfg.models.iter_mut().find(|model| model.id == model_cfg.id) {
        *existing = model_cfg;
    } else {
        cfg.models.push(model_cfg);
    }
}

fn parse_role_profile_binding(raw: &str) -> Result<RoleBindingSpec<'_>> {
    let Some((model_part, runtime)) = raw.rsplit_once('@') else {
        anyhow::bail!("expected model@runtime or provider/model-name@runtime");
    };
    let model_part = model_part.trim();
    let runtime = runtime.trim();
    if model_part.is_empty() || runtime.is_empty() {
        anyhow::bail!("model and runtime cannot be empty");
    }
    if let Some((provider, model_name)) = model_part.split_once('/') {
        let provider = provider.trim();
        let model_name = model_name.trim();
        if provider.is_empty() || model_name.is_empty() {
            anyhow::bail!("provider/model-name cannot contain empty parts");
        }
        Ok(RoleBindingSpec {
            model: None,
            runtime,
            provider: Some(provider),
            model_name: Some(model_name),
        })
    } else {
        Ok(RoleBindingSpec {
            model: Some(model_part),
            runtime,
            provider: None,
            model_name: None,
        })
    }
}

fn resolve_role_model_id(
    cfg: &Config,
    model: Option<&str>,
    provider: Option<&str>,
    model_name: Option<&str>,
) -> Result<String> {
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        return Ok(model.to_string());
    }
    let Some(provider) = provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
    else {
        anyhow::bail!("--model is required unless --provider and --model-name are supplied");
    };
    let Some(model_name) = model_name
        .map(str::trim)
        .filter(|model_name| !model_name.is_empty())
    else {
        anyhow::bail!("--model-name is required when --model is omitted");
    };
    Ok(cfg.derive_model_id(provider, model_name))
}

fn default_agent_teams(
    role: &str,
    runtime_id: &str,
    runtime_cfg: &orchestrator::config::RuntimeConfig,
) -> bool {
    runtime_cfg.supports_agent_teams
        && matches!(runtime_id, "claude-code" | "claude")
        && (role.contains("worker") || role.contains("executor") || role == "worker-cc")
}

fn role_detail_for_cli(
    cfg: &Config,
    role: &str,
    role_cfg: &orchestrator::config::RoleConfig,
    thread: Option<&WorkerThread>,
) -> String {
    let model_entry = cfg.models.iter().find(|m| m.id == role_cfg.model);
    let model = model_entry
        .map(|m| m.id.as_str())
        .unwrap_or(role_cfg.model.as_str());
    let provider = model_entry.map(|m| m.provider.as_str()).unwrap_or("-");
    let api_model = model_entry.map(|m| m.name.as_str()).unwrap_or("-");
    let health = role_health_for_cli(cfg, role_cfg, model_entry);
    let mut detail = format!(
        "{:<14} model={:<18} provider={:<12} api_model={:<28} runtime={:<12} teams={} health={}",
        role, model, provider, api_model, role_cfg.runtime, role_cfg.agent_teams, health
    );
    if role.contains("worker") || thread.is_some() {
        let (thread_id, native_id, last_id) = role_thread_status_tokens(thread);
        detail.push_str(&format!(
            " thread={thread_id} native={native_id} last={last_id}"
        ));
    }
    detail
}

fn role_thread_status_tokens(thread: Option<&WorkerThread>) -> (String, String, String) {
    let Some(thread) = thread else {
        return ("none".to_string(), "none".to_string(), "none".to_string());
    };
    let thread_id = short_uuid(&thread.id);
    let native_id = thread
        .native_session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(short_text_id)
        .unwrap_or_else(|| "none".to_string());
    let last_id = thread
        .last_session_id
        .as_ref()
        .map(short_uuid)
        .unwrap_or_else(|| "none".to_string());
    (thread_id, native_id, last_id)
}

fn role_health_for_cli(
    cfg: &Config,
    role_cfg: &orchestrator::config::RoleConfig,
    model_entry: Option<&orchestrator::config::ModelConfig>,
) -> String {
    let mut issues = Vec::new();
    match model_entry {
        Some(model) => {
            if !cfg.providers.contains_key(&model.provider) {
                issues.push(format!("provider-missing:{}", model.provider));
            }
        }
        None => issues.push(format!("model-missing:{}", role_cfg.model)),
    }
    if cfg.runtime_config(&role_cfg.runtime).is_none() {
        issues.push(format!("runtime-missing:{}", role_cfg.runtime));
    }
    if issues.is_empty() {
        "ready".to_string()
    } else {
        issues.join(",")
    }
}

fn available_models_for_cli(cfg: &Config) -> String {
    let mut values = cfg.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>();
    values.sort();
    if values.is_empty() {
        "(none)".into()
    } else {
        values.join(", ")
    }
}

fn available_runtimes_for_cli(cfg: &Config) -> String {
    let mut values = cfg.runtimes.keys().map(String::as_str).collect::<Vec<_>>();
    values.sort();
    if values.is_empty() {
        "(none)".into()
    } else {
        values.join(", ")
    }
}

fn available_providers_for_cli(cfg: &Config) -> String {
    let mut values = cfg.providers.keys().map(String::as_str).collect::<Vec<_>>();
    values.sort();
    if values.is_empty() {
        "(none)".into()
    } else {
        values.join(", ")
    }
}

fn available_roles_for_cli(cfg: &Config) -> String {
    let mut values = cfg.roles.keys().map(String::as_str).collect::<Vec<_>>();
    values.sort();
    if values.is_empty() {
        "(none)".into()
    } else {
        values.join(", ")
    }
}

fn apply_claude_preset(
    config_path: &Path,
    planner: &str,
    critic: &str,
    worker_cc: &str,
    worker_codex: Option<&str>,
    reviewer: &str,
) -> Result<()> {
    let mut assignments: Vec<(&str, &str)> = vec![
        ("planner", planner),
        ("critic", critic),
        ("worker-cc", worker_cc),
        ("reviewer", reviewer),
    ];
    if let Some(wc) = worker_codex {
        assignments.push(("worker-codex", wc));
    }
    apply_role_models(config_path, &assignments)?;
    println!("\n✓ Switched to Claude preset (requires ANTHROPIC_API_KEY)");
    Ok(())
}

fn write_config(cfg: &Config, path: &Path) -> Result<()> {
    let path = orchestrator::config::expand_home(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let yaml = serde_yaml::to_string(cfg)?;
    std::fs::write(&path, yaml).with_context(|| format!("writing config to {}", path.display()))?;
    Ok(())
}

// ── ANSI colors (minimal) ────────────────────────────────────

mod ansi {
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";
    pub const RESET: &str = "\x1b[0m";
    pub const CYAN: &str = "\x1b[36m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
}

fn c(color: &str, s: &str) -> String {
    format!("{}{}{}", color, s, ansi::RESET)
}

// ── Interactive setup wizard ─────────────────────────────────

fn run_wizard(config_path: &Path) -> Result<()> {
    use std::io::{self, Write};

    println!(
        "\n{}{}",
        c(ansi::BOLD, "⚡ tiffany-loop setup wizard"),
        c(ansi::DIM, " (providers, models, roles)")
    );
    println!();

    let expanded_config_path = orchestrator::config::expand_home(config_path);
    let mut cfg = if expanded_config_path.exists() {
        Config::load(config_path).unwrap_or_else(|_| default_setup_config())
    } else {
        default_setup_config()
    };

    // ── Step 1: pick providers ─────────────────────────────
    println!("{}", c(ansi::BOLD, "Step 1: pick your LLM providers"));
    println!("(multi-select, comma-separated; or just press Enter for Claude-only)\n");

    let available_providers = [
        (
            "anthropic",
            "Anthropic (Claude) — recommended",
            "ANTHROPIC_API_KEY",
            "https://console.anthropic.com/",
        ),
        (
            "openai",
            "OpenAI (GPT-4o, etc.)",
            "OPENAI_API_KEY",
            "https://platform.openai.com/",
        ),
        (
            "google",
            "Google (Gemini)",
            "GOOGLE_API_KEY",
            "https://aistudio.google.com/",
        ),
        (
            "deepseek",
            "DeepSeek (OpenAI-compatible, cheap)",
            "DEEPSEEK_API_KEY",
            "https://platform.deepseek.com/",
        ),
        (
            "mistral",
            "Mistral AI (OpenAI-compatible)",
            "MISTRAL_API_KEY",
            "https://console.mistral.ai/",
        ),
        (
            "cohere",
            "Cohere (Command R+)",
            "COHERE_API_KEY",
            "https://dashboard.cohere.com/",
        ),
        (
            "ollama",
            "Ollama (local, free)",
            "OLLAMA_HOST",
            "http://localhost:11434",
        ),
        (
            "custom",
            "Custom OpenAI-compatible endpoint",
            "CUSTOM_API_KEY",
            "",
        ),
    ];

    for (i, (name, desc, env, _url)) in available_providers.iter().enumerate() {
        println!(
            "  {}{}{}  {} {}",
            c(ansi::CYAN, &format!("[{}]", i + 1)),
            c(ansi::DIM, &format!(" {:<10}", name)),
            c(ansi::RESET, ""),
            desc,
            c(ansi::DIM, &format!("({})", env))
        );
    }
    println!();

    print!("{}", c(ansi::YELLOW, "Choose providers [1] (Enter=1): "));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    let picks: Vec<usize> = if input.is_empty() {
        vec![1]
    } else {
        input
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    };
    let chosen: Vec<&(&str, &str, &str, &str)> = picks
        .iter()
        .filter_map(|&i| available_providers.get(i - 1))
        .collect();

    println!(
        "\n{} chosen: {}",
        c(ansi::GREEN, "✓"),
        chosen
            .iter()
            .map(|(n, _, _, _)| c(ansi::BOLD, n))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // ── Step 2: configure each provider ────────────────────
    println!("\n{}", c(ansi::BOLD, "Step 2: configure each provider"));
    for (name, _desc, env_var, default_url) in &chosen {
        let name_str: &str = name;
        let env_val = std::env::var(env_var).unwrap_or_default();
        if !env_val.is_empty() {
            println!(
                "  ✓ {}: {} found in env ({})",
                c(ansi::GREEN, name_str),
                c(ansi::DIM, &env_val),
                env_var
            );
            let entry = cfg.providers.entry(name.to_string()).or_insert(
                orchestrator::config::ProviderConfig {
                    kind: provider_kind(name).to_string(),
                    api_key: None,
                    base_url: None,
                },
            );
            entry.kind = provider_kind(name).to_string();
            if entry.api_key.is_none() {
                entry.api_key = Some(format!("${{{}}}", env_var));
            }
            // Set default base_url for OpenAI-compatible providers
            if !default_url.is_empty() && entry.base_url.is_none() {
                entry.base_url = Some(default_url.to_string());
            }
        } else {
            print!(
                "  {} API key for {} [or env var name, Enter=skip]: ",
                c(ansi::YELLOW, "?"),
                name
            );
            io::stdout().flush()?;
            let mut key_input = String::new();
            io::stdin().read_line(&mut key_input)?;
            let key_input = key_input.trim();
            let entry = cfg.providers.entry(name.to_string()).or_insert(
                orchestrator::config::ProviderConfig {
                    kind: provider_kind(name).to_string(),
                    api_key: None,
                    base_url: None,
                },
            );
            entry.kind = provider_kind(name).to_string();
            if !key_input.is_empty() {
                entry.api_key = Some(key_input.to_string());
            }
            // For OpenAI-compatible and custom providers, ask for base_url
            let needs_url = matches!(
                *name,
                "openai" | "deepseek" | "mistral" | "cohere" | "custom" | "ollama"
            );
            if needs_url {
                let prompt = if *name == "custom" {
                    format!(
                        "  {} Base URL for custom endpoint [{}]: ",
                        c(ansi::YELLOW, "?"),
                        default_url
                    )
                } else {
                    format!("  {} Base URL [{}]: ", c(ansi::YELLOW, "?"), default_url)
                };
                print!("{}", prompt);
                io::stdout().flush()?;
                let mut url_input = String::new();
                io::stdin().read_line(&mut url_input)?;
                let url = url_input.trim();
                if !url.is_empty() {
                    entry.base_url = Some(url.to_string());
                } else if !default_url.is_empty() {
                    entry.base_url = Some(default_url.to_string());
                }
            }
        }
    }

    // ── Step 3: define models ──────────────────────────────
    println!("\n{}", c(ansi::BOLD, "Step 3: register models"));
    register_default_models_for_configured_providers(&mut cfg);
    println!(
        "  ✓ {} model(s) registered for configured providers",
        c(ansi::GREEN, &cfg.models.len().to_string())
    );

    // ── Step 4: assign models to roles ──────────────────────
    println!("\n{}", c(ansi::BOLD, "Step 4: assign models to roles"));
    let default_assignments = default_role_assignments(&cfg)?;
    for role in [
        "planner",
        "critic",
        "reviewer",
        "worker-cc",
        "worker-codex",
        "worker-gemini",
    ] {
        cfg.roles.remove(role);
    }
    for assignment in &default_assignments {
        print!(
            "  {} {} [{} / {}]: ",
            c(ansi::YELLOW, "?"),
            assignment.role,
            assignment.model,
            assignment.runtime
        );
        io::stdout().flush()?;
        let mut role_input = String::new();
        io::stdin().read_line(&mut role_input)?;
        let chosen_model = role_input.trim();
        let model_id = if chosen_model.is_empty() {
            assignment.model.to_string()
        } else {
            if !cfg.models.iter().any(|model| model.id == chosen_model) {
                anyhow::bail!(
                    "unknown model id '{}' for role '{}'. Available: {}",
                    chosen_model,
                    assignment.role,
                    cfg.models
                        .iter()
                        .map(|model| model.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            chosen_model.to_string()
        };
        cfg.roles.insert(
            assignment.role.to_string(),
            orchestrator::config::RoleConfig {
                model: model_id,
                runtime: assignment.runtime.to_string(),
                agent_teams: assignment.agent_teams,
            },
        );
    }

    // ── Step 5: tag overrides ──────────────────────────────
    println!("\n{}", c(ansi::BOLD, "Step 5: tag-based routing overrides"));
    println!("(comma-separated tag:role pairs, Enter=accept defaults)\n");
    let default_overrides = default_tag_overrides(&cfg);
    for (tag, role) in &default_overrides {
        println!("  {} → {}", c(ansi::CYAN, tag), c(ansi::BOLD, role));
    }
    print!("  Override (e.g. \"docs:worker-codex,test:worker-cc\") or Enter=keep: ");
    io::stdout().flush()?;
    let mut ov_input = String::new();
    io::stdin().read_line(&mut ov_input)?;
    let ov_input = ov_input.trim();
    cfg.overrides = if ov_input.is_empty() {
        default_overrides
            .iter()
            .map(|(t, r)| orchestrator::config::OverrideConfig {
                tag: (*t).to_string(),
                role: r.clone(),
            })
            .collect()
    } else {
        ov_input
            .split(',')
            .filter_map(|pair| {
                let mut parts = pair.split(':');
                let tag = parts.next()?.trim().to_string();
                let role = parts.next()?.trim().to_string();
                Some(orchestrator::config::OverrideConfig { tag, role })
            })
            .collect()
    };

    // ── Step 6: behavior ────────────────────────────────────
    println!(
        "\n{}",
        c(ansi::BOLD, "Step 6: behavior defaults (Enter=accept)")
    );
    cfg.behavior.worktree_base = std::path::PathBuf::from(prompt_default(
        "Worktree base",
        "~/.orchestrator/worktrees",
    )?);
    cfg.behavior.db_path =
        std::path::PathBuf::from(prompt_default("DB path", "~/.orchestrator/state.db")?);
    cfg.behavior.session_log_dir = std::path::PathBuf::from(prompt_default(
        "Session log dir",
        "~/.orchestrator/sessions",
    )?);
    print!("  Use zellij for terminal mux? [Y/n]: ");
    io::stdout().flush()?;
    let mut mux_input = String::new();
    io::stdin().read_line(&mut mux_input)?;
    cfg.behavior.mux = if mux_input.trim().to_lowercase().starts_with('n') {
        orchestrator::config::MuxKind::None
    } else {
        orchestrator::config::MuxKind::Zellij
    };

    // ── Save ────────────────────────────────────────────────
    write_config(&cfg, config_path)?;
    println!(
        "\n{} Config written to {}",
        c(ansi::GREEN, "✓ done."),
        c(ansi::BOLD, &config_path.display().to_string())
    );
    println!("\n  Check: {}", c(ansi::CYAN, "orchestrator doctor"));
    println!("  Start: {}", c(ansi::CYAN, "tiffany-loop"));
    Ok(())
}

fn prompt_default(label: &str, default: &str) -> Result<String> {
    use std::io::{self, Write};
    print!("  {} [{}]: ", label, c(ansi::DIM, default));
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    let s = s.trim();
    if s.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(s.to_string())
    }
}

#[derive(Clone, Copy)]
struct DefaultModelTemplate {
    id: &'static str,
    provider: &'static str,
    name: &'static str,
}

const DEFAULT_MODEL_TEMPLATES: &[DefaultModelTemplate] = &[
    DefaultModelTemplate {
        id: "opus",
        provider: "anthropic",
        name: "claude-opus-4-6",
    },
    DefaultModelTemplate {
        id: "sonnet",
        provider: "anthropic",
        name: "claude-sonnet-4-6",
    },
    DefaultModelTemplate {
        id: "haiku",
        provider: "anthropic",
        name: "claude-haiku-4-5",
    },
    DefaultModelTemplate {
        id: "gpt4o",
        provider: "openai",
        name: "gpt-4o",
    },
    DefaultModelTemplate {
        id: "gpt4o-mini",
        provider: "openai",
        name: "gpt-4o-mini",
    },
    DefaultModelTemplate {
        id: "minimax-m3-claude",
        provider: "minimax",
        name: "MiniMax-M3",
    },
    DefaultModelTemplate {
        id: "minimax-m3-codex",
        provider: "minimax",
        name: "MiniMax-M3",
    },
    DefaultModelTemplate {
        id: "gemini-pro",
        provider: "google",
        name: "gemini-1.5-pro",
    },
    DefaultModelTemplate {
        id: "deepseek-chat",
        provider: "deepseek",
        name: "deepseek-chat",
    },
    DefaultModelTemplate {
        id: "mistral-large",
        provider: "mistral",
        name: "mistral-large-latest",
    },
    DefaultModelTemplate {
        id: "llama3",
        provider: "ollama",
        name: "llama3",
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct DefaultRoleAssignment {
    role: &'static str,
    model: String,
    runtime: &'static str,
    agent_teams: bool,
}

fn default_setup_config() -> Config {
    let mut cfg = serde_yaml::from_str::<Config>(include_str!("../config.example.yaml"))
        .unwrap_or_else(|_| Config::default());
    cfg.providers.clear();
    cfg.models.clear();
    cfg.roles.clear();
    cfg.overrides.clear();
    cfg
}

fn register_default_models_for_configured_providers(cfg: &mut Config) {
    cfg.models
        .retain(|model| cfg.providers.contains_key(&model.provider));
    for template in DEFAULT_MODEL_TEMPLATES {
        if cfg.providers.contains_key(template.provider)
            && !cfg.models.iter().any(|model| model.id == template.id)
        {
            cfg.models.push(orchestrator::config::ModelConfig {
                id: template.id.to_string(),
                provider: template.provider.to_string(),
                name: template.name.to_string(),
            });
        }
    }
}

fn default_role_assignments(cfg: &Config) -> Result<Vec<DefaultRoleAssignment>> {
    let claude_primary = pick_runtime_model(
        cfg,
        RuntimeTarget::Claude,
        &["sonnet", "minimax-m3-claude", "opus", "haiku"],
    );
    let claude_smart = pick_runtime_model(
        cfg,
        RuntimeTarget::Claude,
        &["opus", "sonnet", "minimax-m3-claude", "haiku"],
    );
    let claude_cheap = pick_runtime_model(
        cfg,
        RuntimeTarget::Claude,
        &["haiku", "sonnet", "minimax-m3-claude", "opus"],
    );
    let codex_primary = pick_runtime_model(
        cfg,
        RuntimeTarget::Codex,
        &[
            "gpt4o",
            "minimax-m3-codex",
            "deepseek-chat",
            "mistral-large",
            "llama3",
            "gpt4o-mini",
        ],
    );
    let codex_cheap = pick_runtime_model(
        cfg,
        RuntimeTarget::Codex,
        &[
            "gpt4o-mini",
            "minimax-m3-codex",
            "deepseek-chat",
            "llama3",
            "gpt4o",
        ],
    );
    let gemini_primary = pick_runtime_model(cfg, RuntimeTarget::Gemini, &["gemini-pro"]);

    let Some(planner) = codex_primary
        .clone()
        .or_else(|| claude_primary.clone())
        .or_else(|| gemini_primary.clone())
    else {
        anyhow::bail!(
            "no configured model can drive Claude Code, Codex, or Gemini runtimes; add anthropic/minimax/openai-compatible/ollama/google provider first"
        );
    };
    let planner_runtime = if codex_primary.as_deref() == Some(planner.as_str()) {
        "codex"
    } else if gemini_primary.as_deref() == Some(planner.as_str()) {
        "gemini"
    } else {
        "claude-code"
    };

    let critic = claude_smart
        .clone()
        .or_else(|| codex_primary.clone())
        .or_else(|| gemini_primary.clone())
        .expect("planner availability checked above");
    let critic_runtime = if claude_smart.as_deref() == Some(critic.as_str()) {
        "claude-code"
    } else if gemini_primary.as_deref() == Some(critic.as_str()) {
        "gemini"
    } else {
        "codex"
    };

    let reviewer = codex_cheap
        .clone()
        .or_else(|| claude_cheap.clone())
        .or_else(|| gemini_primary.clone())
        .expect("planner availability checked above");
    let reviewer_runtime = if codex_cheap.as_deref() == Some(reviewer.as_str()) {
        "codex"
    } else if gemini_primary.as_deref() == Some(reviewer.as_str()) {
        "gemini"
    } else {
        "claude-code"
    };

    let mut assignments = vec![
        DefaultRoleAssignment {
            role: "planner",
            model: planner,
            runtime: planner_runtime,
            agent_teams: false,
        },
        DefaultRoleAssignment {
            role: "critic",
            model: critic,
            runtime: critic_runtime,
            agent_teams: false,
        },
        DefaultRoleAssignment {
            role: "reviewer",
            model: reviewer,
            runtime: reviewer_runtime,
            agent_teams: false,
        },
    ];
    if let Some(model) = claude_primary {
        assignments.push(DefaultRoleAssignment {
            role: "worker-cc",
            model,
            runtime: "claude-code",
            agent_teams: true,
        });
    }
    if let Some(model) = codex_primary {
        assignments.push(DefaultRoleAssignment {
            role: "worker-codex",
            model,
            runtime: "codex",
            agent_teams: false,
        });
    }
    if let Some(model) = gemini_primary {
        assignments.push(DefaultRoleAssignment {
            role: "worker-gemini",
            model,
            runtime: "gemini",
            agent_teams: false,
        });
    }
    Ok(assignments)
}

#[derive(Clone, Copy)]
enum RuntimeTarget {
    Claude,
    Codex,
    Gemini,
}

fn pick_runtime_model(
    cfg: &Config,
    runtime: RuntimeTarget,
    preferred_ids: &[&str],
) -> Option<String> {
    for id in preferred_ids {
        if let Some(model) = cfg
            .models
            .iter()
            .find(|model| model.id == *id && model_supports_runtime(cfg, model, runtime))
        {
            return Some(model.id.clone());
        }
    }
    cfg.models
        .iter()
        .find(|model| model_supports_runtime(cfg, model, runtime))
        .map(|model| model.id.clone())
}

fn model_supports_runtime(
    cfg: &Config,
    model: &orchestrator::config::ModelConfig,
    runtime: RuntimeTarget,
) -> bool {
    let Some(provider) = cfg.providers.get(&model.provider) else {
        return false;
    };
    match runtime {
        RuntimeTarget::Claude => {
            provider.kind.eq_ignore_ascii_case("anthropic")
                || model.provider.eq_ignore_ascii_case("minimax")
        }
        RuntimeTarget::Codex => {
            provider.kind.eq_ignore_ascii_case("openai")
                || provider.kind.eq_ignore_ascii_case("ollama")
        }
        RuntimeTarget::Gemini => provider.kind.eq_ignore_ascii_case("google"),
    }
}

fn default_tag_overrides(cfg: &Config) -> Vec<(&'static str, String)> {
    let fallback_role = runtime::default_worker_role(&cfg.roles).unwrap_or_else(|| {
        cfg.roles
            .keys()
            .find(|role| role.contains("worker"))
            .cloned()
            .unwrap_or_else(|| "worker-cc".to_string())
    });
    let refactor_role = if cfg.roles.contains_key("worker-cc") {
        "worker-cc".to_string()
    } else {
        fallback_role.clone()
    };
    let fast_role = if cfg.roles.contains_key("worker-codex") {
        "worker-codex".to_string()
    } else if cfg.roles.contains_key("worker-gemini") {
        "worker-gemini".to_string()
    } else {
        refactor_role.clone()
    };

    let defaults = [
        ("refactor", refactor_role),
        ("boilerplate", fast_role.clone()),
        ("test", fast_role),
    ];
    defaults
        .into_iter()
        .filter(|(_, role)| cfg.roles.contains_key(role))
        .collect()
}

#[derive(Clone, Copy)]
struct ProviderPreset {
    id: &'static str,
    kind: &'static str,
    env: Option<&'static str>,
    endpoint: Option<&'static str>,
    description: &'static str,
}

const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "openai",
        kind: "openai",
        env: Some("OPENAI_API_KEY"),
        endpoint: None,
        description: "OpenAI",
    },
    ProviderPreset {
        id: "anthropic",
        kind: "anthropic",
        env: Some("ANTHROPIC_API_KEY"),
        endpoint: None,
        description: "Anthropic Claude",
    },
    ProviderPreset {
        id: "google",
        kind: "google",
        env: Some("GOOGLE_API_KEY"),
        endpoint: None,
        description: "Google Gemini",
    },
    ProviderPreset {
        id: "minimax",
        kind: "openai",
        env: Some("MINIMAX_API_KEY"),
        endpoint: Some("https://api.minimaxi.com/v1"),
        description: "MiniMax OpenAI-compatible endpoint",
    },
    ProviderPreset {
        id: "deepseek",
        kind: "openai",
        env: Some("DEEPSEEK_API_KEY"),
        endpoint: Some("https://api.deepseek.com/v1"),
        description: "DeepSeek OpenAI-compatible endpoint",
    },
    ProviderPreset {
        id: "openrouter",
        kind: "openai",
        env: Some("OPENROUTER_API_KEY"),
        endpoint: Some("https://openrouter.ai/api/v1"),
        description: "OpenRouter OpenAI-compatible endpoint",
    },
    ProviderPreset {
        id: "moonshot",
        kind: "openai",
        env: Some("MOONSHOT_API_KEY"),
        endpoint: Some("https://api.moonshot.ai/v1"),
        description: "Moonshot/Kimi OpenAI-compatible endpoint",
    },
    ProviderPreset {
        id: "mistral",
        kind: "openai",
        env: Some("MISTRAL_API_KEY"),
        endpoint: Some("https://api.mistral.ai/v1"),
        description: "Mistral OpenAI-compatible endpoint",
    },
    ProviderPreset {
        id: "ollama",
        kind: "ollama",
        env: None,
        endpoint: Some("http://localhost:11434"),
        description: "Ollama local runtime",
    },
    ProviderPreset {
        id: "custom",
        kind: "openai",
        env: Some("CUSTOM_API_KEY"),
        endpoint: None,
        description: "Custom OpenAI-compatible endpoint",
    },
];

fn provider_preset(name: &str) -> Option<ProviderPreset> {
    let normalized = name.to_ascii_lowercase();
    PROVIDER_PRESETS
        .iter()
        .copied()
        .find(|preset| preset.id == normalized)
}

fn provider_kind(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "anthropic" => "anthropic",
        "openai" => "openai",
        "google" => "google",
        "deepseek" => "openai",   // OpenAI-compatible
        "minimax" => "openai",    // OpenAI-compatible
        "openrouter" => "openai", // OpenAI-compatible
        "moonshot" => "openai",   // OpenAI-compatible
        "mistral" => "openai",    // OpenAI-compatible
        "cohere" => "openai",     // OpenAI-compatible
        "ollama" => "ollama",
        "custom" => "openai", // OpenAI-compatible custom endpoint
        _ => "openai",
    }
}

fn set_provider_key(
    config_path: &Path,
    provider: &str,
    kind: Option<&str>,
    key: Option<&str>,
) -> Result<()> {
    let key_value = match key {
        Some(k) => k.to_string(),
        None => {
            use std::io::{self, Write};
            print!("API key for {}: ", provider);
            io::stdout().flush()?;
            let mut s = String::new();
            io::stdin().read_line(&mut s)?;
            s.trim().to_string()
        }
    };
    Config::write_provider_key_to_config_file(
        config_path,
        provider,
        kind.unwrap_or_else(|| provider_kind(provider)),
        &key_value,
    )?;
    println!("✓ {} api_key set", provider);
    Ok(())
}

fn set_provider_endpoint(
    config_path: &Path,
    provider: &str,
    kind: Option<&str>,
    url: &str,
) -> Result<()> {
    Config::write_provider_endpoint_to_config_file(
        config_path,
        provider,
        kind.unwrap_or_else(|| provider_kind(provider)),
        url,
    )?;
    println!("✓ {} endpoint set to {}", provider, url);
    Ok(())
}

fn list_provider_presets() -> Result<()> {
    println!("Built-in provider presets:");
    for preset in PROVIDER_PRESETS {
        println!(
            "  {:<11} type={:<9} env={:<18} endpoint={:<31} {}",
            preset.id,
            preset.kind,
            preset.env.unwrap_or("-"),
            preset.endpoint.unwrap_or("-"),
            preset.description
        );
    }
    println!();
    println!("Example:");
    println!("  ./scripts/tiffany-dev config provider setup minimax");
    println!("  ./scripts/tiffany-dev config provider delete minimax");
    println!("  ./scripts/tiffany-dev config provider setup custom --env CUSTOM_API_KEY --endpoint https://llm.example.com/v1");
    Ok(())
}

fn list_providers(config_path: &Path) -> Result<()> {
    let path = orchestrator::config::expand_home(config_path);
    if !path.exists() {
        println!("No orchestrator config found at {}", path.display());
        println!("Create one with:");
        println!("  ./scripts/tiffany-dev config provider setup minimax");
        return Ok(());
    }

    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading config at {}", path.display()))?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw)
        .with_context(|| format!("parsing config at {}", path.display()))?;
    let Some(providers) = yaml
        .get("providers")
        .and_then(serde_yaml::Value::as_mapping)
    else {
        println!("No providers configured in {}", path.display());
        return Ok(());
    };

    if providers.is_empty() {
        println!("No providers configured in {}", path.display());
        return Ok(());
    }

    let loaded_config = Config::load(config_path).ok();
    println!("Providers ({})", path.display());
    println!(
        "  {:<12} {:<10} {:<24} {:<30} {:<22} roles",
        "provider", "type", "api_key", "endpoint", "models"
    );

    let mut rows = providers.iter().collect::<Vec<_>>();
    rows.sort_by_key(|(key, _)| key.as_str().unwrap_or_default().to_string());
    for (provider, value) in rows {
        let provider = provider.as_str().unwrap_or("<invalid>");
        let kind = value
            .get("type")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("-");
        let api_key = value
            .get("api_key")
            .and_then(serde_yaml::Value::as_str)
            .map(redact_provider_key)
            .unwrap_or_else(|| "-".to_string());
        let endpoint = value
            .get("base_url")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("-");
        let (models, roles) = loaded_config
            .as_ref()
            .map(|cfg| provider_model_role_summary(cfg, provider))
            .unwrap_or_else(|| ("-".to_string(), "-".to_string()));
        println!(
            "  {:<12} {:<10} {:<24} {:<30} {:<22} {}",
            provider, kind, api_key, endpoint, models, roles
        );
    }
    Ok(())
}

fn show_provider(config_path: &Path, provider: &str) -> Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("loading config at {}", config_path.display()))?;
    print!("{}", format_provider_detail_for_cli(&cfg, provider));
    Ok(())
}

fn format_provider_detail_for_cli(cfg: &Config, provider: &str) -> String {
    let Some(provider_cfg) = cfg.providers.get(provider) else {
        return format!(
            "Unknown provider: {provider}\n\
             Available providers: {}\n\n\
             Create one with:\n\
               tiffany-loop config provider setup {provider} --env <ENV_VAR>\n\
             Then bind a role:\n\
               tiffany-loop roles register <role> --provider {provider} --model-name <api-model> --runtime <runtime>\n",
            available_providers_for_cli(cfg)
        );
    };

    let (models, roles) = provider_model_role_summary(cfg, provider);
    let auth = if provider_auth_ready_for_cli(provider_cfg) {
        "set"
    } else {
        "missing"
    };
    let endpoint = provider_cfg
        .base_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("provider default");

    let mut out = format!(
        "Provider {provider}\n\
           type: {}\n\
           auth: {auth}\n\
           endpoint: {endpoint}\n\
           models: {models}\n\
           roles: {roles}\n\n\
         Model bindings:\n",
        provider_cfg.kind
    );
    let mut bound = cfg
        .models
        .iter()
        .filter(|model| model.provider == provider)
        .map(|model| format!("  {} -> {}", model.id, model.name))
        .collect::<Vec<_>>();
    bound.sort();
    if bound.is_empty() {
        out.push_str("  none\n");
    } else {
        for line in bound {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(&format!(
        "Actions: /provider edit {provider}, /role <role>, /roles register <role> --provider {provider} --model-name <api-model> --runtime <runtime>, /doctor\n"
    ));
    out
}

fn provider_auth_ready_for_cli(cfg: &orchestrator::config::ProviderConfig) -> bool {
    let kind = cfg.kind.to_ascii_lowercase();
    if matches!(kind.as_str(), "ollama" | "local" | "none") {
        return true;
    }
    cfg.api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
}

fn provider_model_role_summary(cfg: &Config, provider: &str) -> (String, String) {
    let mut model_ids = cfg
        .models
        .iter()
        .filter(|model| model.provider == provider)
        .map(|model| model.id.clone())
        .collect::<Vec<_>>();
    model_ids.sort();
    model_ids.dedup();

    let model_id_set = model_ids.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut roles = cfg
        .roles
        .iter()
        .filter(|(_, role)| model_id_set.contains(role.model.as_str()))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    roles.sort();
    roles.dedup();

    (
        compact_name_summary(&model_ids, 3),
        compact_name_summary(&roles, 3),
    )
}

fn compact_name_summary(items: &[String], limit: usize) -> String {
    if items.is_empty() {
        return "-".to_string();
    }
    let mut summary = items
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    if items.len() > limit {
        summary.push_str(&format!(",+{}", items.len() - limit));
    }
    summary
}

struct SelectOption {
    label: String,
    hint: String,
}

impl SelectOption {
    fn new(label: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: hint.into(),
        }
    }
}

fn run_provider_setup_ui(config_path: &Path, dry_run: bool, check_env: bool) -> Result<()> {
    println!(
        "\n{} {}",
        c(ansi::BOLD, "tiffany-loop provider config"),
        c(ansi::DIM, "(OpenClaw-style guided config)")
    );
    println!(
        "{}",
        c(
            ansi::DIM,
            "Use Up/Down to choose, Enter to apply, Esc to cancel."
        )
    );

    let action_options = vec![
        SelectOption::new("setup provider", "create or update provider credentials"),
        SelectOption::new("delete provider", "remove one provider from config"),
        SelectOption::new("list providers", "show configured providers"),
        SelectOption::new("cancel", "leave config unchanged"),
    ];
    let action_idx = select_option("Action", &action_options, 0)?
        .ok_or_else(|| anyhow::anyhow!("provider config cancelled"))?;
    match action_idx {
        0 => run_provider_setup_flow(config_path, dry_run, check_env),
        1 => run_provider_delete_ui(config_path, dry_run),
        2 => list_providers(config_path),
        _ => {
            println!("cancelled");
            Ok(())
        }
    }
}

fn run_provider_setup_flow(config_path: &Path, dry_run: bool, check_env: bool) -> Result<()> {
    let provider_options = PROVIDER_PRESETS
        .iter()
        .map(|preset| {
            SelectOption::new(
                preset.id,
                format!(
                    "{} · env={} · endpoint={}",
                    preset.description,
                    preset.env.unwrap_or("-"),
                    preset.endpoint.unwrap_or("provider default")
                ),
            )
        })
        .collect::<Vec<_>>();
    let provider_idx = select_option("Provider", &provider_options, 0)?
        .ok_or_else(|| anyhow::anyhow!("provider setup cancelled"))?;
    let provider = PROVIDER_PRESETS[provider_idx].id;
    let preset = provider_preset(provider);

    let kind_options = vec![
        SelectOption::new("openai", "OpenAI-compatible Chat Completions style"),
        SelectOption::new("anthropic", "Claude API style"),
        SelectOption::new("google", "Gemini API style"),
        SelectOption::new("ollama", "local Ollama runtime"),
    ];
    let default_kind = preset
        .map(|preset| preset.kind)
        .unwrap_or_else(|| provider_kind(provider));
    let kind_default_idx = kind_options
        .iter()
        .position(|option| option.label == default_kind)
        .unwrap_or(0);
    let kind_idx = select_option("Provider type", &kind_options, kind_default_idx)?
        .ok_or_else(|| anyhow::anyhow!("provider setup cancelled"))?;
    let kind = kind_options[kind_idx].label.clone();

    let mut auth_options = Vec::new();
    if let Some(env) = preset.and_then(|preset| preset.env) {
        auth_options.push(SelectOption::new(
            "env",
            format!("store ${{{env}}} reference ({})", env_status(env)),
        ));
    } else {
        auth_options.push(SelectOption::new("env", "choose an env var reference"));
    }
    auth_options.push(SelectOption::new(
        "literal key",
        "stores a redacted API key value in config",
    ));
    auth_options.push(SelectOption::new(
        "no key",
        "local provider or credential supplied elsewhere",
    ));
    let auth_default_idx = if provider.eq_ignore_ascii_case("ollama") {
        2
    } else {
        0
    };
    let auth_idx = select_option("Auth source", &auth_options, auth_default_idx)?
        .ok_or_else(|| anyhow::anyhow!("provider setup cancelled"))?;

    let mut selected_env: Option<String> = None;
    let mut selected_key: Option<String> = None;
    match auth_idx {
        0 => {
            let env = select_env_name(preset.and_then(|preset| preset.env))?;
            selected_env = Some(env);
        }
        1 => {
            println!(
                "{}",
                c(
                    ansi::YELLOW,
                    "Literal key will be written to config. Env refs are safer for shared configs."
                )
            );
            let key = prompt_secret("API key")?;
            if key.trim().is_empty() {
                anyhow::bail!("api key cannot be empty");
            }
            selected_key = Some(key);
        }
        _ => {}
    }

    let endpoint_options = endpoint_options_for_preset(preset);
    let endpoint_idx = select_option("Endpoint", &endpoint_options, 0)?
        .ok_or_else(|| anyhow::anyhow!("provider setup cancelled"))?;
    let selected_endpoint = match endpoint_options[endpoint_idx].label.as_str() {
        "custom" => {
            let default = preset.and_then(|preset| preset.endpoint).unwrap_or("");
            let value = prompt_line("Endpoint URL", default)?;
            if value.trim().is_empty() {
                Some("none".to_string())
            } else {
                Some(value)
            }
        }
        "none" => Some("none".to_string()),
        _ => preset
            .and_then(|preset| preset.endpoint)
            .map(str::to_string)
            .or_else(|| Some("none".to_string())),
    };

    println!();
    println!("{}", c(ansi::BOLD, "Review"));
    println!("  provider: {}", c(ansi::CYAN, provider));
    println!("  type:     {}", kind);
    println!(
        "  auth:     {}",
        provider_auth_preview(selected_env.as_deref(), selected_key.as_deref())
    );
    println!(
        "  endpoint: {}",
        selected_endpoint.as_deref().unwrap_or("provider default")
    );
    println!(
        "  writes:   {}",
        provider_setup_command_preview(
            provider,
            &kind,
            selected_env.as_deref(),
            selected_key.as_deref(),
            selected_endpoint.as_deref(),
        )
    );

    let confirm_options = vec![
        SelectOption::new("write config", "save to ~/.orchestrator/config.yaml"),
        SelectOption::new("cancel", "leave config unchanged"),
    ];
    let confirm_idx = select_option("Confirm", &confirm_options, 0)?
        .ok_or_else(|| anyhow::anyhow!("provider setup cancelled"))?;
    if confirm_idx != 0 {
        println!("cancelled");
        return Ok(());
    }

    setup_provider(
        config_path,
        provider,
        Some(&kind),
        selected_key.as_deref(),
        selected_env.as_deref(),
        selected_endpoint.as_deref(),
        dry_run,
        check_env,
    )
}

fn run_provider_delete_ui(config_path: &Path, dry_run: bool) -> Result<()> {
    let providers = configured_provider_names(config_path)?;
    if providers.is_empty() {
        println!("No configured providers to delete.");
        return Ok(());
    }

    let options = providers
        .iter()
        .map(|provider| SelectOption::new(provider, "remove from providers"))
        .collect::<Vec<_>>();
    let provider_idx = select_option("Delete provider", &options, 0)?
        .ok_or_else(|| anyhow::anyhow!("provider delete cancelled"))?;
    let provider = &providers[provider_idx];

    println!();
    println!("{}", c(ansi::BOLD, "Review"));
    println!("  delete: {}", c(ansi::YELLOW, provider));
    println!("  writes: config provider delete {provider}");
    println!(
        "{}",
        c(
            ansi::DIM,
            "Only providers.<name> is removed. Models/roles are left unchanged."
        )
    );

    let confirm_options = vec![
        SelectOption::new(
            "delete provider",
            "remove it from ~/.orchestrator/config.yaml",
        ),
        SelectOption::new("cancel", "leave config unchanged"),
    ];
    let confirm_idx = select_option("Confirm delete", &confirm_options, 1)?
        .ok_or_else(|| anyhow::anyhow!("provider delete cancelled"))?;
    if confirm_idx != 0 {
        println!("cancelled");
        return Ok(());
    }

    delete_provider(config_path, provider, dry_run)
}

fn configured_provider_names(config_path: &Path) -> Result<Vec<String>> {
    let path = orchestrator::config::expand_home(config_path);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading config at {}", path.display()))?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw)
        .with_context(|| format!("parsing config at {}", path.display()))?;
    let Some(providers) = yaml
        .get("providers")
        .and_then(serde_yaml::Value::as_mapping)
    else {
        return Ok(Vec::new());
    };
    let mut names = providers
        .keys()
        .filter_map(serde_yaml::Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

fn select_env_name(default_env: Option<&str>) -> Result<String> {
    let mut envs = Vec::<&str>::new();
    if let Some(default_env) = default_env {
        envs.push(default_env);
    }
    for env in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GOOGLE_API_KEY",
        "MINIMAX_API_KEY",
        "DEEPSEEK_API_KEY",
        "OPENROUTER_API_KEY",
        "MOONSHOT_API_KEY",
        "MISTRAL_API_KEY",
        "CUSTOM_API_KEY",
    ] {
        if !envs.contains(&env) {
            envs.push(env);
        }
    }

    let mut options = envs
        .iter()
        .map(|env| SelectOption::new(*env, format!("env ref · {}", env_status(env))))
        .collect::<Vec<_>>();
    options.push(SelectOption::new("custom", "type another env var name"));
    let selected = select_option("API key env var", &options, 0)?
        .ok_or_else(|| anyhow::anyhow!("provider setup cancelled"))?;
    let value = if options[selected].label == "custom" {
        prompt_line("Env var name", default_env.unwrap_or("CUSTOM_API_KEY"))?
    } else {
        options[selected].label.clone()
    };
    let value = value.trim().trim_start_matches('$').to_string();
    validate_env_name(&value)?;
    Ok(value)
}

fn endpoint_options_for_preset(preset: Option<ProviderPreset>) -> Vec<SelectOption> {
    let mut options = Vec::new();
    if let Some(endpoint) = preset.and_then(|preset| preset.endpoint) {
        options.push(SelectOption::new(
            "preset",
            format!("use preset endpoint {endpoint}"),
        ));
    } else {
        options.push(SelectOption::new(
            "provider default",
            "do not write a base_url",
        ));
    }
    options.push(SelectOption::new("custom", "type a base_url"));
    options.push(SelectOption::new("none", "remove endpoint from this write"));
    options
}

fn provider_auth_preview(env: Option<&str>, key: Option<&str>) -> String {
    if let Some(env) = env {
        return format!("${{{}}} ({})", env, env_status(env));
    }
    if key.map(str::trim).is_some_and(|value| !value.is_empty()) {
        return "literal key (<redacted>)".to_string();
    }
    "none".to_string()
}

fn provider_setup_command_preview(
    provider: &str,
    kind: &str,
    env: Option<&str>,
    key: Option<&str>,
    endpoint: Option<&str>,
) -> String {
    let mut parts = vec![
        "config".to_string(),
        "provider".to_string(),
        "setup".to_string(),
        provider.to_string(),
        "--type".to_string(),
        kind.to_string(),
    ];
    if let Some(env) = env.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push("--env".to_string());
        parts.push(env.trim_start_matches('$').to_string());
    }
    if key.map(str::trim).is_some_and(|value| !value.is_empty()) {
        parts.push("--key".to_string());
        parts.push("<redacted>".to_string());
    }
    if let Some(endpoint) = endpoint.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push("--endpoint".to_string());
        parts.push(endpoint.to_string());
    }
    parts.join(" ")
}

fn env_status(env: &str) -> &'static str {
    if std::env::var(env).unwrap_or_default().is_empty() {
        "unset"
    } else {
        "set"
    }
}

fn select_option(
    title: &str,
    options: &[SelectOption],
    default_idx: usize,
) -> Result<Option<usize>> {
    use std::io::IsTerminal;

    if options.is_empty() {
        anyhow::bail!("no options available for {title}");
    }
    let default_idx = default_idx.min(options.len().saturating_sub(1));
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return select_option_from_prompt(title, options, default_idx);
    }
    select_option_from_keys(title, options, default_idx)
}

fn select_option_from_prompt(
    title: &str,
    options: &[SelectOption],
    default_idx: usize,
) -> Result<Option<usize>> {
    use std::io::{self, Write};

    println!();
    println!("{}", c(ansi::BOLD, title));
    for (idx, option) in options.iter().enumerate() {
        println!(
            "  {}. {:<16} {}",
            idx + 1,
            option.label,
            c(ansi::DIM, &option.hint)
        );
    }
    print!(
        "{}",
        c(
            ansi::YELLOW,
            &format!("Choose [{}], q to cancel: ", default_idx + 1)
        )
    );
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.eq_ignore_ascii_case("q") || input.eq_ignore_ascii_case("esc") {
        return Ok(None);
    }
    if input.is_empty() {
        return Ok(Some(default_idx));
    }
    let idx = input
        .parse::<usize>()
        .ok()
        .and_then(|value| value.checked_sub(1))
        .filter(|idx| *idx < options.len())
        .ok_or_else(|| anyhow::anyhow!("invalid selection '{input}'"))?;
    Ok(Some(idx))
}

fn select_option_from_keys(
    title: &str,
    options: &[SelectOption],
    default_idx: usize,
) -> Result<Option<usize>> {
    use crossterm::cursor::{MoveToColumn, MoveUp};
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};
    use crossterm::execute;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
    use std::io::{self, Write};

    struct RawGuard;
    impl Drop for RawGuard {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
        }
    }

    enable_raw_mode()?;
    let _guard = RawGuard;
    let mut stdout = io::stdout();
    let mut selected = default_idx;
    let mut rendered_lines = 0usize;
    let visible = 8usize;

    loop {
        if rendered_lines > 0 {
            execute!(
                stdout,
                MoveUp(rendered_lines as u16),
                MoveToColumn(0),
                Clear(ClearType::FromCursorDown)
            )?;
        }
        rendered_lines = render_key_menu(&mut stdout, title, options, selected, visible)?;
        stdout.flush()?;

        match event::read()? {
            Event::Key(key) => match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => selected = (selected + 1).min(options.len() - 1),
                KeyCode::Home => selected = 0,
                KeyCode::End => selected = options.len() - 1,
                KeyCode::PageUp => selected = selected.saturating_sub(visible),
                KeyCode::PageDown => selected = (selected + visible).min(options.len() - 1),
                KeyCode::Char('k') if key.modifiers.is_empty() => {
                    selected = selected.saturating_sub(1)
                }
                KeyCode::Char('j') if key.modifiers.is_empty() => {
                    selected = (selected + 1).min(options.len() - 1)
                }
                KeyCode::Char(ch) if ch.is_ascii_digit() && options.len() <= 9 => {
                    if let Some(value) = ch.to_digit(10) {
                        if value > 0 && (value as usize) <= options.len() {
                            selected = value as usize - 1;
                        }
                    }
                }
                KeyCode::Enter => {
                    clear_key_menu(&mut stdout, rendered_lines)?;
                    return Ok(Some(selected));
                }
                KeyCode::Esc => {
                    clear_key_menu(&mut stdout, rendered_lines)?;
                    return Ok(None);
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    clear_key_menu(&mut stdout, rendered_lines)?;
                    return Ok(None);
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn render_key_menu(
    stdout: &mut impl std::io::Write,
    title: &str,
    options: &[SelectOption],
    selected: usize,
    visible: usize,
) -> Result<usize> {
    let start = dropdown_window_start(selected, options.len(), visible);
    let end = (start + visible).min(options.len());
    let mut lines = 0usize;

    write!(
        stdout,
        "\r{} {}\r\n",
        c(ansi::BOLD, title),
        c(ansi::DIM, "↑/↓ Enter Esc")
    )?;
    lines += 1;
    for (idx, option) in options.iter().enumerate().take(end).skip(start) {
        let marker = if idx == selected {
            c(ansi::CYAN, "›")
        } else {
            " ".to_string()
        };
        let number = if options.len() <= 9 {
            format!("{}.", idx + 1)
        } else {
            "  ".to_string()
        };
        let label = if idx == selected {
            c(ansi::BOLD, &option.label)
        } else {
            option.label.clone()
        };
        write!(
            stdout,
            "\r{} {} {:<18} {}\r\n",
            marker,
            c(ansi::DIM, &number),
            label,
            c(ansi::DIM, &option.hint)
        )?;
        lines += 1;
    }
    if options.len() > visible {
        write!(
            stdout,
            "\r{}\r\n",
            c(
                ansi::DIM,
                &format!("showing {}-{} of {}", start + 1, end, options.len())
            )
        )?;
        lines += 1;
    }
    Ok(lines)
}

fn clear_key_menu(stdout: &mut impl std::io::Write, rendered_lines: usize) -> Result<()> {
    use crossterm::cursor::{MoveToColumn, MoveUp};
    use crossterm::execute;
    use crossterm::terminal::{Clear, ClearType};

    if rendered_lines > 0 {
        execute!(
            stdout,
            MoveUp(rendered_lines as u16),
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown)
        )?;
    }
    Ok(())
}

fn dropdown_window_start(selected: usize, len: usize, visible: usize) -> usize {
    if len <= visible {
        0
    } else if selected >= visible {
        selected + 1 - visible
    } else {
        0
    }
}

fn prompt_line(label: &str, default: &str) -> Result<String> {
    use std::io::{self, Write};

    if default.is_empty() {
        print!("{}: ", c(ansi::YELLOW, label));
    } else {
        print!("{} [{}]: ", c(ansi::YELLOW, label), default);
    }
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let value = value.trim();
    if value.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(value.to_string())
    }
}

fn prompt_secret(label: &str) -> Result<String> {
    use crossterm::cursor::{MoveLeft, MoveToColumn};
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};
    use crossterm::execute;
    use crossterm::style::Print;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use std::io::{self, IsTerminal, Write};

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return prompt_line(label, "");
    }

    struct RawGuard;
    impl Drop for RawGuard {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
        }
    }

    print!("{}: ", c(ansi::YELLOW, label));
    io::stdout().flush()?;
    enable_raw_mode()?;
    let _guard = RawGuard;
    let mut stdout = io::stdout();
    let mut value = String::new();
    loop {
        match event::read()? {
            Event::Key(key) => match key.code {
                KeyCode::Enter => {
                    write!(stdout, "\r\n")?;
                    stdout.flush()?;
                    return Ok(value);
                }
                KeyCode::Esc => {
                    write!(stdout, "\r\n")?;
                    stdout.flush()?;
                    anyhow::bail!("provider setup cancelled");
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    write!(stdout, "\r\n")?;
                    stdout.flush()?;
                    anyhow::bail!("provider setup cancelled");
                }
                KeyCode::Backspace => {
                    if value.pop().is_some() {
                        execute!(stdout, MoveLeft(1), Print(" "), MoveLeft(1))?;
                        stdout.flush()?;
                    }
                }
                KeyCode::Char(ch)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    value.push(ch);
                    execute!(stdout, Print("*"))?;
                    stdout.flush()?;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    value.clear();
                    execute!(
                        stdout,
                        MoveToColumn(0),
                        Print(format!("{}: ", c(ansi::YELLOW, label)))
                    )?;
                    stdout.flush()?;
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn delete_provider(config_path: &Path, provider: &str, dry_run: bool) -> Result<()> {
    let provider = provider.trim();
    if provider.is_empty() {
        anyhow::bail!("provider is required");
    }
    let configured = configured_provider_names(config_path)?;
    let exists = configured.iter().any(|name| name == provider);
    if dry_run {
        println!("Provider delete dry-run");
        println!(
            "  config:   {}",
            orchestrator::config::expand_home(config_path).display()
        );
        println!("  provider: {}", provider);
        println!(
            "  status:   {}",
            if exists { "configured" } else { "not found" }
        );
        println!("  command:  config provider delete {}", provider);
        return Ok(());
    }

    if Config::delete_provider_from_config_file(config_path, provider)? {
        println!("✓ {} provider deleted", provider);
    } else {
        println!("No provider named '{}' was configured", provider);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn setup_provider(
    config_path: &Path,
    provider: &str,
    kind: Option<&str>,
    key: Option<&str>,
    env: Option<&str>,
    endpoint: Option<&str>,
    dry_run: bool,
    check_env: bool,
) -> Result<()> {
    let provider = provider.trim();
    if provider.is_empty() {
        anyhow::bail!("provider is required");
    }
    if key.is_some() && env.is_some() {
        anyhow::bail!("use either --key or --env, not both");
    }

    let preset = provider_preset(provider);
    let kind = kind
        .filter(|value| !value.trim().is_empty())
        .map(str::trim)
        .or_else(|| preset.map(|preset| preset.kind))
        .unwrap_or_else(|| provider_kind(provider));

    let key_value = provider_setup_key_value(provider, key, env, preset)?;
    if check_env {
        if let Some(env_name) = key_value.as_deref().and_then(secret_ref_env_name) {
            if std::env::var(env_name).unwrap_or_default().is_empty() {
                anyhow::bail!("env var {env_name} is not set");
            }
        }
    }
    let endpoint_value = provider_setup_endpoint(endpoint, preset);

    if key_value.is_none() && endpoint_value.is_none() {
        anyhow::bail!(
            "nothing to write for provider '{}'; pass --env, --key, or --endpoint",
            provider
        );
    }

    if dry_run {
        println!("Provider setup dry-run");
        println!(
            "  config:   {}",
            orchestrator::config::expand_home(config_path).display()
        );
        println!("  provider: {}", provider);
        println!("  type:     {}", kind);
        println!(
            "  api_key:  {}",
            key_value
                .as_deref()
                .map(redact_provider_key)
                .unwrap_or_else(|| "-".to_string())
        );
        println!("  endpoint: {}", endpoint_value.as_deref().unwrap_or("-"));
        return Ok(());
    }

    if let Some(key_value) = key_value {
        Config::write_provider_key_to_config_file(config_path, provider, kind, &key_value)?;
        println!("✓ {} api_key set", provider);
    }
    if let Some(endpoint_value) = endpoint_value {
        Config::write_provider_endpoint_to_config_file(
            config_path,
            provider,
            kind,
            &endpoint_value,
        )?;
        println!("✓ {} endpoint set to {}", provider, endpoint_value);
    }
    Ok(())
}

fn provider_setup_key_value(
    provider: &str,
    key: Option<&str>,
    env: Option<&str>,
    preset: Option<ProviderPreset>,
) -> Result<Option<String>> {
    if let Some(key) = key.map(str::trim).filter(|value| !value.is_empty()) {
        return Ok(Some(key.to_string()));
    }
    if let Some(env) = env.map(str::trim).filter(|value| !value.is_empty()) {
        if matches!(env, "none" | "off" | "-") {
            return Ok(None);
        }
        let env = env.trim_start_matches('$');
        validate_env_name(env)?;
        return Ok(Some(format!("${{{env}}}")));
    }
    if let Some(env) = preset.and_then(|preset| preset.env) {
        return Ok(Some(format!("${{{env}}}")));
    }
    if provider.eq_ignore_ascii_case("ollama") {
        return Ok(None);
    }
    Ok(None)
}

fn provider_setup_endpoint(
    endpoint: Option<&str>,
    preset: Option<ProviderPreset>,
) -> Option<String> {
    let Some(endpoint) = endpoint.map(str::trim) else {
        return preset
            .and_then(|preset| preset.endpoint)
            .map(str::to_string);
    };
    if endpoint.is_empty() || matches!(endpoint, "none" | "off" | "-") {
        None
    } else if endpoint == "default" {
        preset
            .and_then(|preset| preset.endpoint)
            .map(str::to_string)
    } else {
        Some(endpoint.to_string())
    }
}

fn validate_env_name(env: &str) -> Result<()> {
    if env.is_empty() {
        anyhow::bail!("env var name cannot be empty");
    }
    if !env
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
    {
        anyhow::bail!("env var name '{env}' must use A-Z, 0-9, or _");
    }
    Ok(())
}

fn secret_ref_env_name(value: &str) -> Option<&str> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix("${") {
        return rest.strip_suffix('}');
    }
    value.strip_prefix('$')
}

fn redact_provider_key(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "-".to_string();
    }
    if let Some(env) = secret_ref_env_name(value) {
        let status = if std::env::var(env).unwrap_or_default().is_empty() {
            "unset"
        } else {
            "set"
        };
        return format!("${{{env}}} ({status})");
    }
    let len = value.chars().count();
    if len <= 8 {
        "***".to_string()
    } else {
        let head = value.chars().take(4).collect::<String>();
        let tail = value
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        format!("{head}...{tail}")
    }
}

fn apply_expensive_orchestrators(
    config_path: &Path,
    orchestrator_model: &str,
    reviewer_model: &str,
    worker_model: Option<&str>,
) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let default_worker = worker_model.unwrap_or_else(|| {
        for cheap in &[
            "MiniMax-M3",
            "gpt-4o-mini",
            "haiku",
            "deepseek-chat",
            "llama3",
            "gpt4o-mini",
        ] {
            if cfg.models.iter().any(|m| m.id == *cheap) {
                return *cheap;
            }
        }
        cfg.models.first().map(|m| m.id.as_str()).unwrap_or("haiku")
    });

    let assignments = vec![
        ("planner", orchestrator_model),
        ("critic", orchestrator_model),
        ("reviewer", reviewer_model),
        ("worker-cc", default_worker),
        ("worker-codex", default_worker),
    ];
    apply_role_models(config_path, &assignments)?;
    println!("\n✓ Routing preset applied:");
    println!(
        "  Orchestrators (planner/critic): {}",
        c(ansi::BOLD, orchestrator_model)
    );
    println!(
        "  Reviewer:                       {}",
        c(ansi::BOLD, reviewer_model)
    );
    println!(
        "  Workers:                         {}",
        c(ansi::BOLD, default_worker)
    );
    Ok(())
}

/// Apply a named roleset preset.
fn apply_roleset(config_path: &Path, name: &str) -> Result<()> {
    match name {
        "codex" | "codex-heavy" | "gpt" => {
            // Use whatever's available (MiniMax-M3 via anthropic, or gpt4o)
            let cfg = Config::load(config_path)?;
            let cheap_model = pick_cheap_model(&cfg, "gpt4o");
            let reviewer_model = pick_cheap_model(&cfg, "haiku");
            let cheap = cheap_model.as_str();
            let rev = reviewer_model.as_str();
            apply_role_models(
                config_path,
                &[
                    ("planner", cheap),
                    ("critic", cheap),
                    ("worker-cc", cheap),
                    ("worker-codex", cheap),
                    ("reviewer", rev),
                ],
            )?;
            // Favor worker-codex in tag overrides
            let mut cfg = Config::load(config_path)?;
            cfg.overrides = vec![
                orchestrator::config::OverrideConfig {
                    tag: "refactor".into(),
                    role: "worker-codex".into(),
                },
                orchestrator::config::OverrideConfig {
                    tag: "boilerplate".into(),
                    role: "worker-codex".into(),
                },
                orchestrator::config::OverrideConfig {
                    tag: "test".into(),
                    role: "worker-codex".into(),
                },
                orchestrator::config::OverrideConfig {
                    tag: "docs".into(),
                    role: "worker-codex".into(),
                },
            ];
            write_config(&cfg, config_path)?;
            println!("\n{} Codex-heavy roleset applied:", c(ansi::GREEN, "✓"));
            println!(
                "  Models: {} (all roles), reviewer={}",
                c(ansi::BOLD, &cheap_model),
                c(ansi::BOLD, &reviewer_model)
            );
            println!("  Default worker: codex CLI (needs OPENAI_API_KEY or custom config)");
            println!("  Tags route to worker-codex");
        }
        "claude" | "claude-heavy" | "cc" => {
            apply_role_models(
                config_path,
                &[
                    ("planner", "sonnet"),
                    ("critic", "opus"),
                    ("worker-cc", "sonnet"),
                    ("worker-codex", "sonnet"),
                    ("reviewer", "haiku"),
                ],
            )?;
            let mut cfg = Config::load(config_path)?;
            cfg.overrides = vec![
                orchestrator::config::OverrideConfig {
                    tag: "refactor".into(),
                    role: "worker-cc".into(),
                },
                orchestrator::config::OverrideConfig {
                    tag: "architecture".into(),
                    role: "worker-cc".into(),
                },
                orchestrator::config::OverrideConfig {
                    tag: "boilerplate".into(),
                    role: "worker-cc".into(),
                },
                orchestrator::config::OverrideConfig {
                    tag: "test".into(),
                    role: "worker-cc".into(),
                },
            ];
            write_config(&cfg, config_path)?;
            println!("\n{} Claude-heavy roleset applied:", c(ansi::GREEN, "✓"));
            println!("  planner/reviewer → sonnet/haiku");
            println!("  critic → opus (adversarial)");
            println!("  workers → sonnet");
            println!("  Tags route to worker-cc");
        }
        "economy" | "cheap" => {
            apply_expensive_orchestrators(config_path, "haiku", "haiku", Some("haiku"))?;
            println!("\n{} Economy roleset (all haiku):", c(ansi::GREEN, "✓"));
        }
        "quality" | "best" => {
            apply_expensive_orchestrators(config_path, "opus", "sonnet", Some("sonnet"))?;
            println!(
                "\n{} Quality roleset (opus everywhere):",
                c(ansi::GREEN, "✓")
            );
        }
        _ => {
            anyhow::bail!(
                "unknown roleset '{}'. Available: codex, claude, economy, quality",
                name
            );
        }
    }
    Ok(())
}

/// Pick the cheapest model that's available in the config.
fn pick_cheap_model(cfg: &Config, default: &str) -> String {
    for cheap in &[
        "MiniMax-M3",
        "gpt-4o-mini",
        "haiku",
        "deepseek-chat",
        "llama3",
        "gpt4o-mini",
    ] {
        if cfg.models.iter().any(|m| m.id == *cheap) {
            return cheap.to_string();
        }
    }
    if cfg.models.iter().any(|m| m.id == default) {
        return default.to_string();
    }
    cfg.models
        .first()
        .map(|m| m.id.clone())
        .unwrap_or_else(|| "haiku".to_string())
}

#[cfg(test)]
fn resolve_role_model(
    cfg: &Config,
    role: &str,
    override_model: Option<&str>,
    fallback: &str,
) -> Result<String> {
    if let Some(model) = override_model {
        if let Some(m) = cfg.models.iter().find(|m| m.id == model || m.name == model) {
            return Ok(m.name.clone());
        }
        anyhow::bail!(
            "unknown {} model override '{}'. Available models: {}",
            role,
            model,
            cfg.models
                .iter()
                .map(|m| format!("{} ({})", m.id, m.name))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(cfg
        .roles
        .get(role)
        .and_then(|r| cfg.models.iter().find(|m| m.id == r.model))
        .map(|m| m.name.clone())
        .unwrap_or_else(|| fallback.to_string()))
}

fn resolve_role_model_entry<'a>(
    cfg: &'a Config,
    role: &str,
    override_model: Option<&str>,
) -> Result<Option<&'a orchestrator::config::ModelConfig>> {
    if let Some(model) = override_model {
        if let Some(m) = cfg.models.iter().find(|m| m.id == model || m.name == model) {
            return Ok(Some(m));
        }
        anyhow::bail!(
            "unknown {} model override '{}'. Available models: {}",
            role,
            model,
            cfg.models
                .iter()
                .map(|m| format!("{} ({})", m.id, m.name))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(cfg
        .roles
        .get(role)
        .and_then(|r| cfg.models.iter().find(|m| m.id == r.model)))
}

fn resolve_role_cli_spec(
    cfg: &Config,
    role: &str,
    override_model: Option<&str>,
    fallback_model: &str,
    fallback_runtime: &str,
) -> Result<roles::cli_subprocess::RoleCliSpec> {
    let model_entry = resolve_role_model_entry(cfg, role, override_model)?;
    let model = model_entry
        .map(|m| m.name.clone())
        .unwrap_or_else(|| fallback_model.to_string());
    let runtime_id = cfg
        .roles
        .get(role)
        .map(|r| r.runtime.as_str())
        .unwrap_or(fallback_runtime);
    let runtime =
        roles::cli_subprocess::RoleCliRuntime::from_runtime_id(runtime_id).ok_or_else(|| {
            anyhow::anyhow!("role '{}' uses unsupported runtime '{}'", role, runtime_id)
        })?;
    let binary = cfg
        .runtime_config(runtime_id)
        .and_then(|rt| rt.binary.clone())
        .unwrap_or_else(|| default_binary_for_role_runtime(runtime));
    let mut spec = roles::cli_subprocess::RoleCliSpec::new(runtime, binary, model)
        .with_bypass_permissions(
            runtime == roles::cli_subprocess::RoleCliRuntime::ClaudeCode
                && cfg.behavior.cc_bypass_permissions,
        );
    if let Some(model_entry) = model_entry {
        match runtime {
            roles::cli_subprocess::RoleCliRuntime::ClaudeCode => {
                spec = apply_claude_provider_env(spec, cfg, model_entry);
            }
            roles::cli_subprocess::RoleCliRuntime::Codex => {
                spec = apply_codex_provider_config(spec, cfg, model_entry);
            }
            roles::cli_subprocess::RoleCliRuntime::Gemini => {
                spec = apply_gemini_provider_env(spec, cfg, model_entry);
            }
        }
    }
    Ok(spec)
}

fn apply_claude_provider_env(
    mut spec: roles::cli_subprocess::RoleCliSpec,
    cfg: &Config,
    model_entry: &orchestrator::config::ModelConfig,
) -> roles::cli_subprocess::RoleCliSpec {
    let Some(provider) = cfg.providers.get(&model_entry.provider) else {
        return spec;
    };
    let base_url = claude_base_url_for_provider(&model_entry.provider, provider);
    if !provider.kind.eq_ignore_ascii_case("anthropic") && base_url.is_none() {
        return spec;
    }
    if let Some(api_key) = provider
        .api_key
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        spec = spec
            .with_env("ANTHROPIC_AUTH_TOKEN", api_key)
            .with_env("ANTHROPIC_API_KEY", api_key);
    }
    if let Some(base_url) = base_url {
        spec = spec.with_env("ANTHROPIC_BASE_URL", base_url);
    }
    spec = spec
        .with_env("ANTHROPIC_MODEL", &model_entry.name)
        .with_env("ANTHROPIC_DEFAULT_SONNET_MODEL", &model_entry.name)
        .with_env("ANTHROPIC_DEFAULT_SONNET_MODEL_NAME", &model_entry.name)
        .with_env("ANTHROPIC_DEFAULT_OPUS_MODEL", &model_entry.name)
        .with_env("ANTHROPIC_DEFAULT_OPUS_MODEL_NAME", &model_entry.name)
        .with_env("ANTHROPIC_DEFAULT_HAIKU_MODEL", &model_entry.name);
    spec = spec.with_env("ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME", &model_entry.name);
    spec
}

fn claude_base_url_for_provider(
    provider_id: &str,
    provider: &orchestrator::config::ProviderConfig,
) -> Option<String> {
    let base_url = provider.base_url.as_deref()?.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return None;
    }
    if provider.kind.eq_ignore_ascii_case("anthropic") {
        return Some(base_url.to_string());
    }
    if provider_id.eq_ignore_ascii_case("minimax") {
        let root = base_url.strip_suffix("/v1").unwrap_or(base_url);
        return Some(format!("{}/anthropic", root.trim_end_matches('/')));
    }
    None
}

fn apply_codex_provider_config(
    mut spec: roles::cli_subprocess::RoleCliSpec,
    cfg: &Config,
    model_entry: &orchestrator::config::ModelConfig,
) -> roles::cli_subprocess::RoleCliSpec {
    let Some(provider) = cfg.providers.get(&model_entry.provider) else {
        return spec;
    };
    let provider_kind = provider.kind.to_ascii_lowercase();
    if provider_kind != "openai" && provider_kind != "ollama" {
        return spec;
    }

    let provider_id = model_entry.provider.as_str();
    spec = spec.with_config_override("model_provider", provider_id);

    if provider_id == "openai" {
        if let Some(base_url) = provider
            .base_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            spec = spec.with_config_override("openai_base_url", base_url);
        }
    } else {
        let prefix = format!("model_providers.{provider_id}");
        spec = spec
            .with_config_override(format!("{prefix}.name"), provider_id)
            .with_config_override(format!("{prefix}.wire_api"), "responses");
        if let Some(base_url) = provider
            .base_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            spec = spec.with_config_override(format!("{prefix}.base_url"), base_url);
        }
    }

    if let Some(api_key) = provider
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if provider_id == "openai" {
            if let Some(env) = provider_secret_env_name(api_key) {
                if let Ok(value) = std::env::var(env) {
                    spec = spec.with_env("OPENAI_API_KEY", value);
                }
            } else {
                spec = spec.with_env("OPENAI_API_KEY", api_key.trim());
            }
            return spec;
        }

        let (env_key, env_value) = codex_provider_env_key(provider_id, api_key);
        if let Some(value) = env_value {
            spec = spec.with_env(&env_key, value);
        }
        spec = spec.with_config_override(format!("model_providers.{provider_id}.env_key"), env_key);
    }

    spec
}

fn codex_provider_env_key(provider_id: &str, api_key: &str) -> (String, Option<String>) {
    let api_key = api_key.trim();
    if let Some(env) = provider_secret_env_name(api_key) {
        return (env.to_string(), None);
    }
    let normalized = provider_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    (
        format!("TIFFANY_{normalized}_API_KEY"),
        Some(api_key.to_string()),
    )
}

fn apply_gemini_provider_env(
    mut spec: roles::cli_subprocess::RoleCliSpec,
    cfg: &Config,
    model_entry: &orchestrator::config::ModelConfig,
) -> roles::cli_subprocess::RoleCliSpec {
    let Some(provider) = cfg.providers.get(&model_entry.provider) else {
        return spec;
    };
    if !provider.kind.eq_ignore_ascii_case("google") {
        return spec;
    }
    if let Some(api_key) = provider
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if let Some(env) = provider_secret_env_name(api_key) {
            if let Ok(value) = std::env::var(env) {
                spec = spec
                    .with_env("GEMINI_API_KEY", &value)
                    .with_env("GOOGLE_API_KEY", value);
            }
        } else {
            spec = spec
                .with_env("GEMINI_API_KEY", api_key.trim())
                .with_env("GOOGLE_API_KEY", api_key.trim());
        }
    }
    spec
}

fn provider_secret_env_name(value: &str) -> Option<&str> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix("${") {
        return rest.strip_suffix('}');
    }
    value.strip_prefix('$')
}

fn default_binary_for_role_runtime(runtime: roles::cli_subprocess::RoleCliRuntime) -> String {
    match runtime {
        roles::cli_subprocess::RoleCliRuntime::ClaudeCode => which::which("claude")
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "claude".to_string()),
        roles::cli_subprocess::RoleCliRuntime::Codex => which::which("codex")
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "codex".to_string()),
        roles::cli_subprocess::RoleCliRuntime::Gemini => which::which("gemini")
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "gemini".to_string()),
    }
}

fn write_session_export(path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

fn copy_to_clipboard_cli(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .context("starting pbcopy")?;

    #[cfg(target_os = "windows")]
    let mut child = Command::new("clip")
        .stdin(Stdio::piped())
        .spawn()
        .context("starting clip")?;

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("xclip -selection clipboard || xsel --clipboard --input || wl-copy")
        .stdin(Stdio::piped())
        .spawn()
        .context("starting xclip/xsel/wl-copy")?;

    {
        let mut stdin = child.stdin.take().context("clipboard stdin unavailable")?;
        stdin
            .write_all(text.as_bytes())
            .context("writing clipboard data")?;
    }
    let status = child.wait().context("waiting for clipboard command")?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("clipboard command exited with {status}");
    }
}

pub async fn build_orchestrator(
    cfg: &Config,
    no_critic: bool,
    no_reviewer: bool,
    planner_override: Option<&str>,
    critic_override: Option<&str>,
    reviewer_override: Option<&str>,
) -> Result<Orchestrator> {
    // Load Claude Code's existing config (CLAUDE.md, settings, agents, MCP, ...)
    let cc_config = Arc::new(cc_config::CCConfig::load());
    tracing::info!(
        "loaded CC config: {} chars system prompt, {} agents, {} MCP servers, {} prior sessions",
        cc_config.system_prompt.len(),
        cc_config.agents.len(),
        cc_config.mcp_servers.len(),
        cc_config.prior_session_ids.len(),
    );

    // Build worktree pool + session store
    let worktree_pool = Arc::new(storage::worktree::WorktreePool::new(
        &cfg.behavior.worktree_base,
    ));
    let session_store = Arc::new(orchestrator::core::session_store::SessionStore::open(
        &cfg.behavior.session_log_dir,
        &cfg.behavior.db_path,
    )?);

    // Build adapters
    let mut adapters: std::collections::HashMap<
        String,
        Arc<dyn orchestrator::core::worker::WorkerAdapter>,
    > = std::collections::HashMap::new();
    let provider_configs = Arc::new(cfg.providers.clone());

    for (name, rt) in &cfg.runtimes {
        let runtime = roles::cli_subprocess::RoleCliRuntime::from_runtime_id(name);
        match (rt.kind.as_str(), runtime) {
            ("subprocess", Some(roles::cli_subprocess::RoleCliRuntime::ClaudeCode)) => {
                let binary = rt.binary.clone().unwrap_or_else(|| "claude".to_string());
                let model = cfg
                    .models
                    .iter()
                    .find(|m| m.id == "sonnet")
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| "claude-sonnet-4-6".to_string());
                let adapter = Arc::new(adapters::claude_code::ClaudeCodeAdapter::new(
                    binary,
                    model,
                    rt.supports_agent_teams,
                    cfg.behavior.cc_bypass_permissions,
                    worktree_pool.clone(),
                    session_store.clone(),
                    cc_config.clone(),
                    provider_configs.clone(),
                ));
                adapters.insert(name.clone(), adapter);
            }
            ("subprocess", Some(roles::cli_subprocess::RoleCliRuntime::Codex)) => {
                let binary = rt.binary.clone().unwrap_or_else(|| "codex".to_string());
                let model = cfg
                    .models
                    .iter()
                    .find(|m| m.id == "gpt4o")
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| "gpt-4o".to_string());
                let adapter = Arc::new(adapters::codex_cli::CodexCLIAdapter::new(
                    binary,
                    model,
                    worktree_pool.clone(),
                    session_store.clone(),
                    provider_configs.clone(),
                ));
                adapters.insert(name.clone(), adapter);
            }
            ("subprocess", Some(roles::cli_subprocess::RoleCliRuntime::Gemini)) => {
                let binary = rt.binary.clone().unwrap_or_else(|| "gemini".to_string());
                let model = cfg
                    .models
                    .iter()
                    .find(|m| m.id == "gemini-pro")
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| "gemini-1.5-pro".to_string());
                let adapter = Arc::new(adapters::gemini_cli::GeminiCLIAdapter::new(
                    binary,
                    model,
                    worktree_pool.clone(),
                    session_store.clone(),
                    provider_configs.clone(),
                ));
                adapters.insert(name.clone(), adapter);
            }
            _ => {}
        }
    }

    // Build router
    let router = Arc::new(roles::router::CapabilityRouter::new_with_models(
        &cfg.roles,
        &cfg.overrides,
        &cfg.models,
    ));

    // Build planner / critic / reviewer from role runtime config.
    // Each role can run through Claude Code or Codex CLI while still sharing
    // the same structured planner/critic/reviewer prompts.
    let planner_spec =
        resolve_role_cli_spec(cfg, "planner", planner_override, "gpt-4o-mini", "codex")?;
    let critic_spec = resolve_role_cli_spec(
        cfg,
        "critic",
        critic_override,
        "claude-haiku-4-5",
        "claude-code",
    )?;
    let reviewer_spec =
        resolve_role_cli_spec(cfg, "reviewer", reviewer_override, "gpt-4o-mini", "codex")?;
    tracing::info!(
        "role CLI specs: planner={:?}, critic={:?}, reviewer={:?}",
        planner_spec,
        critic_spec,
        reviewer_spec
    );

    let planner: Arc<dyn roles::planner::Planner> = Arc::new(
        roles::cli_subprocess::ClaudeCodePlanner::from_spec(planner_spec),
    );
    let critic: Arc<dyn roles::critic::Critic> = Arc::new(
        roles::cli_subprocess::ClaudeCodeCritic::from_spec(critic_spec),
    );
    let reviewer: Arc<dyn roles::reviewer::Reviewer> = Arc::new(
        roles::cli_subprocess::ClaudeCodeReviewer::from_spec(reviewer_spec),
    );

    Ok(Orchestrator::new(
        planner,
        critic,
        reviewer,
        router,
        adapters,
        session_store,
        cfg.behavior.max_replan,
        cfg.behavior.enable_critic && !no_critic,
        cfg.behavior.enable_reviewer && !no_reviewer,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;
    use orchestrator::config::{ModelConfig, ProviderConfig, RoleConfig};

    fn config_with_models() -> Config {
        let mut cfg = Config::default();
        cfg.models = vec![
            ModelConfig {
                id: "sonnet".to_string(),
                provider: "anthropic".to_string(),
                name: "claude-sonnet-4-6".to_string(),
            },
            ModelConfig {
                id: "gpt4o".to_string(),
                provider: "openai".to_string(),
                name: "gpt-4o".to_string(),
            },
        ];
        cfg.roles.insert(
            "planner".to_string(),
            RoleConfig {
                model: "sonnet".to_string(),
                runtime: "claude-code".to_string(),
                agent_teams: false,
            },
        );
        cfg.runtimes.insert(
            "claude-code".to_string(),
            orchestrator::config::RuntimeConfig {
                kind: "subprocess".to_string(),
                binary: Some("claude-test".to_string()),
                supports_mcp: true,
                supports_agent_teams: true,
            },
        );
        cfg.runtimes.insert(
            "codex".to_string(),
            orchestrator::config::RuntimeConfig {
                kind: "subprocess".to_string(),
                binary: Some("codex-test".to_string()),
                supports_mcp: false,
                supports_agent_teams: false,
            },
        );
        cfg
    }

    fn provider(kind: &str) -> ProviderConfig {
        ProviderConfig {
            kind: kind.to_string(),
            api_key: Some("test-key".to_string()),
            base_url: None,
        }
    }

    #[test]
    fn provider_model_role_summary_links_models_and_roles() {
        let mut cfg = config_with_models();
        cfg.roles.insert(
            "reviewer".to_string(),
            RoleConfig {
                model: "sonnet".to_string(),
                runtime: "claude-code".to_string(),
                agent_teams: false,
            },
        );
        cfg.roles.insert(
            "worker-codex".to_string(),
            RoleConfig {
                model: "gpt4o".to_string(),
                runtime: "codex".to_string(),
                agent_teams: false,
            },
        );

        assert_eq!(
            provider_model_role_summary(&cfg, "anthropic"),
            ("sonnet".to_string(), "planner,reviewer".to_string())
        );
        assert_eq!(
            provider_model_role_summary(&cfg, "openai"),
            ("gpt4o".to_string(), "worker-codex".to_string())
        );
        assert_eq!(
            provider_model_role_summary(&cfg, "missing"),
            ("-".to_string(), "-".to_string())
        );
    }

    #[test]
    fn provider_detail_formats_model_and_role_bindings() {
        let mut cfg = config_with_models();
        cfg.providers
            .insert("openai".to_string(), provider("openai"));
        cfg.roles.insert(
            "worker-codex".to_string(),
            RoleConfig {
                model: "gpt4o".to_string(),
                runtime: "codex".to_string(),
                agent_teams: false,
            },
        );

        let rendered = format_provider_detail_for_cli(&cfg, "openai");

        assert!(rendered.contains("Provider openai"));
        assert!(rendered.contains("type: openai"));
        assert!(rendered.contains("auth: set"));
        assert!(rendered.contains("models: gpt4o"));
        assert!(rendered.contains("roles: worker-codex"));
        assert!(rendered.contains("gpt4o -> gpt-4o"));
        assert!(rendered.contains("/provider edit openai"));
    }

    #[test]
    fn provider_detail_guides_unknown_provider_setup() {
        let cfg = config_with_models();

        let rendered = format_provider_detail_for_cli(&cfg, "openai");

        assert!(rendered.contains("Unknown provider: openai"));
        assert!(rendered.contains("Available providers: (none)"));
        assert!(rendered.contains("tiffany-loop config provider setup openai"));
    }

    #[test]
    fn role_options_align_models_with_supported_runtimes() {
        let mut cfg = config_with_models();
        cfg.providers
            .insert("anthropic".to_string(), provider("anthropic"));
        cfg.providers
            .insert("openai".to_string(), provider("openai"));
        cfg.providers
            .insert("google".to_string(), provider("google"));
        cfg.models.push(ModelConfig {
            id: "gemini-pro".to_string(),
            provider: "google".to_string(),
            name: "gemini-2.5-pro".to_string(),
        });
        cfg.runtimes.insert(
            "gemini".to_string(),
            orchestrator::config::RuntimeConfig {
                kind: "subprocess".to_string(),
                binary: Some("gemini-test".to_string()),
                supports_mcp: false,
                supports_agent_teams: false,
            },
        );

        let sonnet = cfg
            .models
            .iter()
            .find(|model| model.id == "sonnet")
            .unwrap();
        let codex = cfg.models.iter().find(|model| model.id == "gpt4o").unwrap();
        let gemini = cfg
            .models
            .iter()
            .find(|model| model.id == "gemini-pro")
            .unwrap();

        assert_eq!(
            compatible_runtime_ids_for_model(&cfg, sonnet),
            vec!["claude-code"]
        );
        assert_eq!(compatible_runtime_ids_for_model(&cfg, codex), vec!["codex"]);
        assert_eq!(
            compatible_runtime_ids_for_model(&cfg, gemini),
            vec!["gemini"]
        );
        assert!(role_model_option_for_cli(&cfg, sonnet).contains("roles=worker-cc"));
        assert!(role_model_option_for_cli(&cfg, codex).contains("worker-codex"));
        assert!(role_model_option_for_cli(&cfg, gemini).contains("worker-gemini"));
    }

    #[test]
    fn role_detail_for_cli_surfaces_provider_api_model_and_health() {
        let mut cfg = config_with_models();
        cfg.providers
            .insert("anthropic".to_string(), provider("anthropic"));

        let planner = cfg.roles.get("planner").unwrap();
        let detail = role_detail_for_cli(&cfg, "planner", planner, None);
        assert!(detail.contains("model=sonnet"));
        assert!(detail.contains("provider=anthropic"));
        assert!(detail.contains("api_model=claude-sonnet-4-6"));
        assert!(detail.contains("runtime=claude-code"));
        assert!(detail.contains("health=ready"));

        let broken = RoleConfig {
            model: "missing-model".to_string(),
            runtime: "missing-runtime".to_string(),
            agent_teams: false,
        };
        let detail = role_detail_for_cli(&cfg, "broken", &broken, None);
        assert!(detail.contains("health=model-missing:missing-model"));
        assert!(detail.contains("runtime-missing:missing-runtime"));
    }

    #[test]
    fn role_detail_for_cli_surfaces_worker_thread_handoff_state() {
        let mut cfg = config_with_models();
        cfg.providers
            .insert("openai".to_string(), provider("openai"));
        let role = RoleConfig {
            model: "gpt4o".to_string(),
            runtime: "codex".to_string(),
            agent_teams: false,
        };
        let thread = WorkerThread {
            id: uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap(),
            scope: "tui:/tmp/project:session:abc".to_string(),
            role: "worker-codex".to_string(),
            runtime: "codex".to_string(),
            agent: "codex".to_string(),
            model: "gpt-4o".to_string(),
            provider: Some("openai".to_string()),
            worktree_path: Some(std::path::PathBuf::from("/tmp/project")),
            native_session_id: Some("codex-native-session-123456".to_string()),
            last_session_id: Some(
                uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            ),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let detail = role_detail_for_cli(&cfg, "worker-codex", &role, Some(&thread));

        assert!(detail.contains("provider=openai"));
        assert!(detail.contains("api_model=gpt-4o"));
        assert!(detail.contains("thread=00000000"));
        assert!(detail.contains("native=codex-native..."));
        assert!(detail.contains("last=00000000"));
    }

    #[test]
    fn register_role_can_create_model_from_provider_model_name() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");
        std::fs::write(
            &config_path,
            "providers:\n  minimax:\n    type: openai\n    api_key: ${MINIMAX_API_KEY}\nruntimes:\n  claude-code:\n    type: subprocess\n    binary: claude\n    supports_agent_teams: true\nmodels: []\nroles: {}\nbehavior: {}\n",
        )
        .unwrap();

        register_role(
            &config_path,
            "worker-cc",
            None,
            "claude-code",
            Some("minimax"),
            Some("MiniMax-M3"),
            true,
            false,
        )
        .unwrap();

        let body = std::fs::read_to_string(&config_path).unwrap();
        assert!(body.contains("${MINIMAX_API_KEY}"));
        assert!(body.contains("id: minimax-m3"));
        assert!(body.contains("provider: minimax"));
        assert!(body.contains("name: MiniMax-M3"));
        assert!(body.contains("worker-cc:"));
        assert!(body.contains("model: minimax-m3"));
        assert!(body.contains("runtime: claude-code"));
        assert!(body.contains("agent_teams: true"));
    }

    #[test]
    fn delete_role_removes_only_role_binding() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");
        std::fs::write(
            &config_path,
            "providers:\n  minimax:\n    type: openai\n    api_key: ${MINIMAX_API_KEY}\nruntimes:\n  codex:\n    type: subprocess\n    binary: codex\nmodels:\n  - id: minimax-m3\n    provider: minimax\n    name: MiniMax-M3\nroles:\n  worker-codex:\n    model: minimax-m3\n    runtime: codex\n    agent_teams: false\nbehavior: {}\n",
        )
        .unwrap();

        delete_role(&config_path, "worker-codex").unwrap();

        let body = std::fs::read_to_string(&config_path).unwrap();
        assert!(!body.contains("worker-codex:"));
        assert!(body.contains("minimax-m3"));
        assert!(body.contains("providers:"));
        assert!(body.contains("runtimes:"));

        delete_role(&config_path, "missing-role").unwrap();
    }

    #[test]
    fn parse_role_profile_binding_accepts_model_or_provider_model() {
        let model = parse_role_profile_binding("sonnet@claude-code").unwrap();
        assert_eq!(model.model, Some("sonnet"));
        assert_eq!(model.provider, None);
        assert_eq!(model.model_name, None);
        assert_eq!(model.runtime, "claude-code");

        let provider = parse_role_profile_binding("minimax/MiniMax-M3@claude-code").unwrap();
        assert_eq!(provider.model, None);
        assert_eq!(provider.provider, Some("minimax"));
        assert_eq!(provider.model_name, Some("MiniMax-M3"));
        assert_eq!(provider.runtime, "claude-code");

        assert!(parse_role_profile_binding("sonnet").is_err());
        assert!(parse_role_profile_binding("/MiniMax-M3@claude-code").is_err());
    }

    #[test]
    fn save_role_profile_writes_multiple_roles_and_inline_model() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");
        std::fs::write(
            &config_path,
            "providers:\n  anthropic:\n    type: anthropic\n    api_key: ${ANTHROPIC_API_KEY}\n  minimax:\n    type: openai\n    api_key: ${MINIMAX_API_KEY}\n    base_url: https://api.minimaxi.com/v1\n  google:\n    type: google\n    api_key: ${GOOGLE_API_KEY}\nruntimes:\n  claude-code:\n    type: subprocess\n    binary: claude\n    supports_agent_teams: true\n  codex:\n    type: subprocess\n    binary: codex\n  gemini:\n    type: subprocess\n    binary: gemini\nmodels:\n  - id: sonnet\n    provider: anthropic\n    name: claude-sonnet-4-6\nroles: {}\nbehavior: {}\n",
        )
        .unwrap();

        save_role_profile(
            &config_path,
            "dev",
            &[
                ("planner", Some("sonnet@claude-code")),
                ("worker-cc", Some("minimax/MiniMax-M3@claude-code")),
                ("worker-codex", Some("sonnet@codex")),
                ("worker-gemini", Some("google/gemini-2.5-pro@gemini")),
            ],
            false,
        )
        .unwrap();

        let body = std::fs::read_to_string(&config_path).unwrap();
        assert!(body.contains("planner:"));
        assert!(body.contains("worker-cc:"));
        assert!(body.contains("worker-codex:"));
        assert!(body.contains("worker-gemini:"));
        assert!(body.contains("id: minimax-m3"));
        assert!(body.contains("provider: minimax"));
        assert!(body.contains("name: MiniMax-M3"));
        assert!(body.contains("id: gemini-2-5-pro"));
        assert!(body.contains("provider: google"));
        assert!(body.contains("name: gemini-2.5-pro"));
        assert!(body.contains("agent_teams: true"));
        assert!(body.contains("runtime: codex"));
        assert!(body.contains("runtime: gemini"));
    }

    #[test]
    fn save_role_profile_allows_later_roles_to_reuse_new_model() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");
        std::fs::write(
            &config_path,
            "providers:\n  minimax:\n    type: openai\n    api_key: ${MINIMAX_API_KEY}\n    base_url: https://api.minimaxi.com/v1\nruntimes:\n  claude-code:\n    type: subprocess\n    binary: claude\n    supports_agent_teams: true\nmodels: []\nroles: {}\nbehavior: {}\n",
        )
        .unwrap();

        save_role_profile(
            &config_path,
            "reuse",
            &[
                ("planner", Some("minimax/MiniMax-M3@claude-code")),
                ("critic", Some("minimax-m3@claude-code")),
            ],
            false,
        )
        .unwrap();

        let body = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(body.matches("id: minimax-m3").count(), 1);
        assert!(body.contains("planner:"));
        assert!(body.contains("critic:"));
    }

    #[test]
    fn save_role_profile_dry_run_does_not_write_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");
        let original = "providers:\n  anthropic:\n    type: anthropic\n    api_key: ${ANTHROPIC_API_KEY}\nruntimes:\n  claude-code:\n    type: subprocess\n    binary: claude\nmodels:\n  - id: sonnet\n    provider: anthropic\n    name: claude-sonnet-4-6\nroles: {}\nbehavior: {}\n";
        std::fs::write(&config_path, original).unwrap();

        save_role_profile(
            &config_path,
            "preview",
            &[("planner", Some("sonnet@claude-code"))],
            true,
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    }

    #[test]
    fn role_model_override_accepts_model_id() {
        let cfg = config_with_models();
        let model = resolve_role_model(&cfg, "planner", Some("gpt4o"), "fallback").unwrap();
        assert_eq!(model, "gpt-4o");
    }

    #[test]
    fn role_model_override_accepts_model_name() {
        let cfg = config_with_models();
        let model =
            resolve_role_model(&cfg, "planner", Some("claude-sonnet-4-6"), "fallback").unwrap();
        assert_eq!(model, "claude-sonnet-4-6");
    }

    #[test]
    fn role_model_uses_configured_role_without_override() {
        let cfg = config_with_models();
        let model = resolve_role_model(&cfg, "planner", None, "fallback").unwrap();
        assert_eq!(model, "claude-sonnet-4-6");
    }

    #[test]
    fn role_model_rejects_unknown_override() {
        let cfg = config_with_models();
        let err = resolve_role_model(&cfg, "planner", Some("missing"), "fallback").unwrap_err();
        assert!(format!("{:#}", err).contains("unknown planner model override"));
    }

    #[test]
    fn role_cli_spec_uses_configured_runtime_binary_and_model_name() {
        let mut cfg = config_with_models();
        cfg.roles.insert(
            "planner".to_string(),
            RoleConfig {
                model: "gpt4o".to_string(),
                runtime: "codex".to_string(),
                agent_teams: false,
            },
        );

        let spec = resolve_role_cli_spec(&cfg, "planner", None, "fallback", "claude-code").unwrap();

        assert_eq!(
            spec.runtime,
            orchestrator::roles::cli_subprocess::RoleCliRuntime::Codex
        );
        assert_eq!(spec.binary, "codex-test");
        assert_eq!(spec.model, "gpt-4o");
    }

    #[test]
    fn role_cli_spec_injects_anthropic_provider_env_for_claude_runtime() {
        let mut cfg = config_with_models();
        cfg.providers.insert(
            "anthropic".to_string(),
            orchestrator::config::ProviderConfig {
                kind: "anthropic".to_string(),
                api_key: Some("sk-test-secret".to_string()),
                base_url: Some("https://api.minimaxi.com/anthropic".to_string()),
            },
        );

        let spec = resolve_role_cli_spec(&cfg, "planner", None, "fallback", "claude-code").unwrap();

        assert_eq!(
            spec.runtime,
            orchestrator::roles::cli_subprocess::RoleCliRuntime::ClaudeCode
        );
        assert_eq!(spec.model, "claude-sonnet-4-6");
        assert!(spec.bypass_permissions);
        assert!(spec.env.contains(&(
            "ANTHROPIC_BASE_URL".to_string(),
            "https://api.minimaxi.com/anthropic".to_string()
        )));
        assert!(spec.env.contains(&(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            "sk-test-secret".to_string()
        )));
        assert!(spec.env.contains(&(
            "ANTHROPIC_API_KEY".to_string(),
            "sk-test-secret".to_string()
        )));
        assert!(spec.env.contains(&(
            "ANTHROPIC_MODEL".to_string(),
            "claude-sonnet-4-6".to_string()
        )));
        assert!(spec.env.contains(&(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
            "claude-sonnet-4-6".to_string()
        )));
        assert!(spec.env.contains(&(
            "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME".to_string(),
            "claude-sonnet-4-6".to_string()
        )));
        assert!(spec.env.contains(&(
            "ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(),
            "claude-sonnet-4-6".to_string()
        )));
        assert!(spec.env.contains(&(
            "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME".to_string(),
            "claude-sonnet-4-6".to_string()
        )));
        assert!(spec.env.contains(&(
            "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
            "claude-sonnet-4-6".to_string()
        )));
        assert!(spec.env.contains(&(
            "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME".to_string(),
            "claude-sonnet-4-6".to_string()
        )));
        assert!(!format!("{spec:?}").contains("sk-test-secret"));
    }

    #[test]
    fn role_cli_spec_maps_minimax_openai_provider_for_claude_runtime() {
        let mut cfg = config_with_models();
        cfg.providers.insert(
            "minimax".to_string(),
            orchestrator::config::ProviderConfig {
                kind: "openai".to_string(),
                api_key: Some("sk-test-secret".to_string()),
                base_url: Some("https://api.minimaxi.com/v1".to_string()),
            },
        );
        cfg.models.push(ModelConfig {
            id: "minimax-m3-claude".to_string(),
            provider: "minimax".to_string(),
            name: "MiniMax-M3".to_string(),
        });
        cfg.roles.insert(
            "planner".to_string(),
            RoleConfig {
                model: "minimax-m3-claude".to_string(),
                runtime: "claude-code".to_string(),
                agent_teams: false,
            },
        );

        let spec = resolve_role_cli_spec(&cfg, "planner", None, "fallback", "codex").unwrap();

        assert_eq!(
            spec.runtime,
            orchestrator::roles::cli_subprocess::RoleCliRuntime::ClaudeCode
        );
        assert_eq!(spec.model, "MiniMax-M3");
        assert!(spec.env.contains(&(
            "ANTHROPIC_BASE_URL".to_string(),
            "https://api.minimaxi.com/anthropic".to_string()
        )));
        assert!(spec
            .env
            .contains(&("ANTHROPIC_MODEL".to_string(), "MiniMax-M3".to_string())));
        assert!(spec.env.contains(&(
            "ANTHROPIC_API_KEY".to_string(),
            "sk-test-secret".to_string()
        )));
        assert!(!format!("{spec:?}").contains("sk-test-secret"));
    }

    #[test]
    fn role_cli_spec_respects_disabled_claude_permission_bypass() {
        let mut cfg = config_with_models();
        cfg.behavior.cc_bypass_permissions = false;

        let spec = resolve_role_cli_spec(&cfg, "planner", None, "fallback", "claude-code").unwrap();

        assert_eq!(
            spec.runtime,
            orchestrator::roles::cli_subprocess::RoleCliRuntime::ClaudeCode
        );
        assert!(!spec.bypass_permissions);
    }

    #[test]
    fn role_cli_spec_injects_openai_compatible_provider_for_codex_runtime() {
        let mut cfg = config_with_models();
        cfg.providers.insert(
            "minimax".to_string(),
            orchestrator::config::ProviderConfig {
                kind: "openai".to_string(),
                api_key: Some("sk-test-secret".to_string()),
                base_url: Some("https://api.minimaxi.com/v1".to_string()),
            },
        );
        cfg.models.push(ModelConfig {
            id: "minimax-m3-codex".to_string(),
            provider: "minimax".to_string(),
            name: "MiniMax-M3".to_string(),
        });
        cfg.roles.insert(
            "worker-codex".to_string(),
            RoleConfig {
                model: "minimax-m3-codex".to_string(),
                runtime: "codex".to_string(),
                agent_teams: false,
            },
        );

        let spec = resolve_role_cli_spec(&cfg, "worker-codex", None, "fallback", "codex").unwrap();

        assert_eq!(
            spec.runtime,
            orchestrator::roles::cli_subprocess::RoleCliRuntime::Codex
        );
        assert_eq!(spec.model, "MiniMax-M3");
        assert!(spec.env.contains(&(
            "TIFFANY_MINIMAX_API_KEY".to_string(),
            "sk-test-secret".to_string()
        )));
        assert!(spec
            .config_overrides
            .contains(&("model_provider".to_string(), "minimax".to_string())));
        assert!(spec.config_overrides.contains(&(
            "model_providers.minimax.base_url".to_string(),
            "https://api.minimaxi.com/v1".to_string()
        )));
        assert!(spec.config_overrides.contains(&(
            "model_providers.minimax.wire_api".to_string(),
            "responses".to_string()
        )));
        assert!(spec.config_overrides.contains(&(
            "model_providers.minimax.env_key".to_string(),
            "TIFFANY_MINIMAX_API_KEY".to_string()
        )));
        assert!(!format!("{spec:?}").contains("sk-test-secret"));
    }

    #[test]
    fn role_cli_spec_injects_google_provider_env_for_gemini_runtime() {
        let mut cfg = config_with_models();
        cfg.runtimes.insert(
            "gemini".to_string(),
            orchestrator::config::RuntimeConfig {
                kind: "subprocess".to_string(),
                binary: Some("gemini-test".to_string()),
                supports_mcp: false,
                supports_agent_teams: false,
            },
        );
        cfg.providers.insert(
            "google".to_string(),
            orchestrator::config::ProviderConfig {
                kind: "google".to_string(),
                api_key: Some("google-secret".to_string()),
                base_url: None,
            },
        );
        cfg.models.push(ModelConfig {
            id: "gemini-pro".to_string(),
            provider: "google".to_string(),
            name: "gemini-2.5-pro".to_string(),
        });
        cfg.roles.insert(
            "worker-gemini".to_string(),
            RoleConfig {
                model: "gemini-pro".to_string(),
                runtime: "gemini".to_string(),
                agent_teams: false,
            },
        );

        let spec =
            resolve_role_cli_spec(&cfg, "worker-gemini", None, "fallback", "gemini").unwrap();

        assert_eq!(
            spec.runtime,
            orchestrator::roles::cli_subprocess::RoleCliRuntime::Gemini
        );
        assert_eq!(spec.binary, "gemini-test");
        assert_eq!(spec.model, "gemini-2.5-pro");
        assert!(spec
            .env
            .contains(&("GEMINI_API_KEY".to_string(), "google-secret".to_string())));
        assert!(spec
            .env
            .contains(&("GOOGLE_API_KEY".to_string(), "google-secret".to_string())));
        assert!(!format!("{spec:?}").contains("google-secret"));
    }

    #[tokio::test]
    async fn build_orchestrator_registers_gemini_runtime_adapter() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = config_with_models();
        cfg.behavior.worktree_base = tmp.path().join("worktrees");
        cfg.behavior.session_log_dir = tmp.path().join("sessions");
        cfg.behavior.db_path = tmp.path().join("state.sqlite");
        cfg.runtimes.insert(
            "gemini".to_string(),
            orchestrator::config::RuntimeConfig {
                kind: "subprocess".to_string(),
                binary: Some("gemini-test".to_string()),
                supports_mcp: false,
                supports_agent_teams: false,
            },
        );
        cfg.providers.insert(
            "google".to_string(),
            orchestrator::config::ProviderConfig {
                kind: "google".to_string(),
                api_key: Some("google-secret".to_string()),
                base_url: None,
            },
        );
        cfg.models.push(ModelConfig {
            id: "gemini-pro".to_string(),
            provider: "google".to_string(),
            name: "gemini-2.5-pro".to_string(),
        });
        cfg.roles.insert(
            "worker-gemini".to_string(),
            RoleConfig {
                model: "gemini-pro".to_string(),
                runtime: "gemini".to_string(),
                agent_teams: false,
            },
        );

        let orch = build_orchestrator(&cfg, true, true, None, None, None)
            .await
            .unwrap();

        let adapter = orch.adapters.get("gemini").expect("gemini adapter");
        assert_eq!(adapter.name(), "gemini");
    }

    #[test]
    fn worker_thread_cli_list_includes_roles_resume_state_and_actions() {
        let mut cfg = config_with_models();
        cfg.roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "sonnet".to_string(),
                runtime: "claude-code".to_string(),
                agent_teams: true,
            },
        );
        cfg.roles.insert(
            "worker-codex".to_string(),
            RoleConfig {
                model: "gpt4o".to_string(),
                runtime: "codex".to_string(),
                agent_teams: false,
            },
        );
        let thread = WorkerThread {
            id: uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap(),
            scope: "cwd:/tmp/tiffany-worker".to_string(),
            role: "worker-codex".to_string(),
            runtime: "codex".to_string(),
            agent: "codex".to_string(),
            model: "gpt-4o".to_string(),
            provider: Some("openai".to_string()),
            worktree_path: None,
            native_session_id: Some("codex-native-session-123456".to_string()),
            last_session_id: Some(
                uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            ),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let rendered = worker_thread_list(&cfg, &[thread]);

        assert!(rendered.contains("Worker threads"));
        assert!(rendered.contains("worker-cc"));
        assert!(rendered.contains("no worker thread yet"));
        assert!(rendered.contains("worker-codex"));
        assert!(rendered.contains("openai/gpt-4o"));
        assert!(rendered.contains("native codex-native..."));
        assert!(rendered.contains("orchestrator thread show <role>"));
        assert!(rendered.contains("orchestrator thread clear <role>"));
    }

    #[test]
    fn worker_thread_cli_detail_shows_native_resume_command() {
        let mut cfg = config_with_models();
        cfg.roles.insert(
            "worker-codex".to_string(),
            RoleConfig {
                model: "gpt4o".to_string(),
                runtime: "codex".to_string(),
                agent_teams: false,
            },
        );
        let thread = WorkerThread {
            id: uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap(),
            scope: "cwd:/tmp/tiffany-worker".to_string(),
            role: "worker-codex".to_string(),
            runtime: "codex".to_string(),
            agent: "codex".to_string(),
            model: "gpt-4o".to_string(),
            provider: Some("openai".to_string()),
            worktree_path: Some(std::path::PathBuf::from("/tmp/tiffany-worker")),
            native_session_id: Some("codex-native-session".to_string()),
            last_session_id: Some(
                uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            ),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let detail = worker_thread_detail(&cfg, &thread);

        assert!(detail.contains("Session card"));
        assert!(detail.contains("reuse: same role in the same project keeps Tiffany thread"));
        assert!(detail.contains("scope: cwd:/tmp/tiffany-worker"));
        assert!(detail.contains("Binding"));
        assert!(detail.contains("Native session"));
        assert!(detail.contains("Commands"));
        assert!(detail.contains("native session: codex-native-session"));
        assert!(detail.contains("native resume: codex-test exec resume codex-native-session"));
        assert!(detail.contains(
            "native handoff: cd /tmp/tiffany-worker && codex-test resume codex-native-session"
        ));
        assert!(detail.contains("TUI resume: /continue open worker-codex"));
        assert!(detail.contains("legacy handoff: /continue codex"));
        assert!(detail.contains("Action: open tiffany-loop and run /continue open worker-codex"));
        assert!(detail.contains("Action: /continue codex saves a handoff package"));
        assert!(detail.contains("Status: ready for native resume"));
        assert!(detail.contains("Action: orchestrator thread clear worker-codex"));
        assert!(detail.contains("/tmp/tiffany-worker"));
    }

    #[test]
    fn worker_thread_selection_prefers_recoverable_native_session_for_same_role() {
        let mut cfg = config_with_models();
        cfg.roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "sonnet".to_string(),
                runtime: "claude-code".to_string(),
                agent_teams: true,
            },
        );
        let now = chrono::Utc::now();
        let current_without_native = WorkerThread {
            id: uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000111").unwrap(),
            scope: "tui:/tmp/project:session:current".to_string(),
            role: "worker-cc".to_string(),
            runtime: "claude-code".to_string(),
            agent: "claude-code".to_string(),
            model: "sonnet".to_string(),
            provider: Some("anthropic".to_string()),
            worktree_path: Some(std::path::PathBuf::from("/tmp/project")),
            native_session_id: None,
            last_session_id: None,
            created_at: now,
            updated_at: now,
        };
        let previous_with_native = WorkerThread {
            id: uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000222").unwrap(),
            scope: "tui:/tmp/project:session:previous".to_string(),
            role: "worker-cc".to_string(),
            runtime: "claude-code".to_string(),
            agent: "claude-code".to_string(),
            model: "sonnet".to_string(),
            provider: Some("anthropic".to_string()),
            worktree_path: Some(std::path::PathBuf::from("/tmp/project")),
            native_session_id: Some("claude-native-session".to_string()),
            last_session_id: Some(
                uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000333").unwrap(),
            ),
            created_at: now - chrono::Duration::minutes(10),
            updated_at: now - chrono::Duration::minutes(10),
        };
        let threads = vec![current_without_native, previous_with_native];

        let selected = find_worker_thread(&threads, "worker-cc").expect("selected thread");
        let rendered = worker_thread_list(&cfg, &threads);

        assert_eq!(
            selected.native_session_id.as_deref(),
            Some("claude-native-session")
        );
        assert!(rendered.contains("native claude-nativ..."));
        assert!(rendered.contains("thread 00000000"));
    }

    #[test]
    fn worker_thread_cli_handoff_quotes_paths_and_targets_native_tui() {
        let thread = WorkerThread {
            id: uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap(),
            scope: "cwd:/tmp/tiffany-worker".to_string(),
            role: "worker-cc".to_string(),
            runtime: "claude-code".to_string(),
            agent: "claude-code".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            provider: Some("anthropic".to_string()),
            worktree_path: Some(std::path::PathBuf::from("/tmp/tiffany worker")),
            native_session_id: Some("native session".to_string()),
            last_session_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        assert_eq!(
            native_thread_handoff_command(&Config::default(), &thread),
            "cd '/tmp/tiffany worker' && claude --resume 'native session'"
        );
    }

    #[test]
    fn tui_job_result_capture_prefers_final_worker_output() {
        let mut capture = TuiJobResultCapture::default();
        let task_id = uuid::Uuid::new_v4();

        capture.observe(
            &orchestrator::pipeline::orchestrator::RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".to_string(),
                role: "worker-cc".to_string(),
                event_kind: "assistant".to_string(),
                content: "claude-code assistant: thinking".to_string(),
            },
        );
        capture.observe(
            &orchestrator::pipeline::orchestrator::RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".to_string(),
                role: "worker-cc".to_string(),
                event_kind: "assistant".to_string(),
                content: "claude-code assistant: interim answer".to_string(),
            },
        );
        capture.observe(
            &orchestrator::pipeline::orchestrator::RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".to_string(),
                role: "worker-cc".to_string(),
                event_kind: "result".to_string(),
                content: "claude-code result: final answer for the job".to_string(),
            },
        );

        assert_eq!(
            capture.result().as_deref(),
            Some("final answer for the job")
        );
    }

    #[test]
    fn tui_job_result_capture_does_not_replace_final_answer_with_diff() {
        let mut capture = TuiJobResultCapture::default();
        let task_id = uuid::Uuid::new_v4();

        capture.observe(
            &orchestrator::pipeline::orchestrator::RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".to_string(),
                role: "worker-cc".to_string(),
                event_kind: "assistant".to_string(),
                content:
                    "claude-code assistant: Fake Claude worker completed the Tiffany e2e smoke run."
                        .to_string(),
            },
        );
        capture.observe(
            &orchestrator::pipeline::orchestrator::RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".to_string(),
                role: "worker-cc".to_string(),
                event_kind: "result".to_string(),
                content:
                    "claude-code result: Fake Claude worker completed the Tiffany e2e smoke run."
                        .to_string(),
            },
        );
        capture.observe(&orchestrator::pipeline::orchestrator::RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".to_string(),
            role: "worker-cc".to_string(),
            event_kind: "diff".to_string(),
            content: "claude-code diff: files changed:\n  - README.md\n\ndiff --git a/README.md b/README.md\n+very long diff output".to_string(),
        });

        assert_eq!(
            capture.result().as_deref(),
            Some("Fake Claude worker completed the Tiffany e2e smoke run.")
        );
    }

    #[test]
    fn jobs_cli_includes_result_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let store = orchestrator::core::session_store::SessionStore::open(
            &tmp.path().join("logs"),
            &tmp.path().join("db.sqlite"),
        )
        .unwrap();
        let job = store
            .create_tui_job(
                "write summary",
                "running",
                Some("single-worker"),
                Some("worker-cc"),
            )
            .unwrap();
        store
            .set_tui_job_status(job.id, "done", Some("final result\nwith two lines"), None)
            .unwrap();

        let rendered = format_tui_jobs_cli(&store, 5).unwrap();

        assert!(rendered.contains("✓"));
        assert!(rendered.contains("done"));
        assert!(rendered.contains("result final result with two lines"));
        assert!(!rendered.contains('{'));
    }

    #[test]
    fn jobs_cli_can_show_and_cancel_by_short_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = orchestrator::core::session_store::SessionStore::open(
            &tmp.path().join("logs"),
            &tmp.path().join("db.sqlite"),
        )
        .unwrap();
        let job = store
            .create_tui_job(
                "queued follow-up",
                "queued",
                Some("direct-answer"),
                Some("worker-cc"),
            )
            .unwrap();
        let short_id = job.id.to_string().chars().take(8).collect::<String>();

        let shown = run_jobs_command(
            &store,
            Some(crate::JobsCmd::Show {
                id: short_id.clone(),
            }),
            20,
        )
        .unwrap();
        assert!(shown.contains("Jobs"));
        assert!(shown.contains(&short_id));
        assert!(shown.contains("queued follow-up"));
        assert!(shown.contains("queued"));

        let cancelled = run_jobs_command(
            &store,
            Some(crate::JobsCmd::Cancel {
                id: short_id.clone(),
            }),
            20,
        )
        .unwrap();
        assert!(cancelled.contains("Job "));
        assert!(cancelled.contains("cancelled"));
        assert!(cancelled.contains("error cancelled by user from jobs"));
        let job = store.get_tui_job(job.id).unwrap().expect("job");
        assert_eq!(job.status, "cancelled");
    }

    #[test]
    fn jobs_cli_recovers_only_stale_running_jobs() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("db.sqlite");
        let store = orchestrator::core::session_store::SessionStore::open(
            &tmp.path().join("logs"),
            &db_path,
        )
        .unwrap();
        let stale = store
            .create_tui_job(
                "stale running",
                "running",
                Some("single-worker"),
                Some("worker-cc"),
            )
            .unwrap();
        let fresh = store
            .create_tui_job(
                "fresh running",
                "running",
                Some("single-worker"),
                Some("worker-codex"),
            )
            .unwrap();
        let queued = store
            .create_tui_job(
                "queued prompt",
                "queued",
                Some("direct-answer"),
                Some("worker-gemini"),
            )
            .unwrap();
        let old = (chrono::Utc::now() - chrono::Duration::minutes(45)).to_rfc3339();
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute(
            "UPDATE tui_jobs SET started_at = ?, updated_at = ? WHERE id = ?",
            rusqlite::params![old, old, stale.id.to_string()],
        )
        .unwrap();

        let rendered = recover_stale_tui_jobs(&store, 30, 10).unwrap();

        let stale = store.get_tui_job(stale.id).unwrap().expect("stale job");
        let fresh = store.get_tui_job(fresh.id).unwrap().expect("fresh job");
        let queued = store.get_tui_job(queued.id).unwrap().expect("queued job");
        assert_eq!(stale.status, "failed");
        assert!(stale
            .error
            .as_deref()
            .unwrap()
            .contains("recovered stale running job"));
        assert!(stale.ended_at.is_some());
        assert_eq!(fresh.status, "running");
        assert_eq!(queued.status, "queued");
        assert!(rendered.contains("Recovered stale jobs"));
        assert!(rendered.contains("failed: 1"));
        assert!(rendered.contains("stale running"));
        assert!(rendered.contains("fresh running"));
    }

    #[test]
    fn jobs_cli_retries_finished_job_as_new_queued_job() {
        let tmp = tempfile::tempdir().unwrap();
        let store = orchestrator::core::session_store::SessionStore::open(
            &tmp.path().join("logs"),
            &tmp.path().join("db.sqlite"),
        )
        .unwrap();
        let failed = store
            .create_tui_job(
                "retry this prompt",
                "running",
                Some("single-worker"),
                Some("worker-cc"),
            )
            .unwrap();
        store
            .set_tui_job_status(failed.id, "failed", None, Some("network timeout"))
            .unwrap();
        let short = short_uuid_for_cli(failed.id);

        let rendered = retry_tui_job(&store, &short, 10, false, false).unwrap();
        let jobs = store.list_tui_jobs(10).unwrap();
        let retry = jobs
            .iter()
            .find(|job| job.id != failed.id && job.prompt == "retry this prompt")
            .expect("retry job");
        let failed = store.get_tui_job(failed.id).unwrap().expect("failed job");

        assert_eq!(failed.status, "failed");
        assert_eq!(retry.status, "queued");
        assert_eq!(retry.route.as_deref(), Some("single-worker"));
        assert_eq!(retry.role.as_deref(), Some("worker-cc"));
        assert!(rendered.contains("queued for retry"));
        assert!(rendered.contains(&short_uuid_for_cli(retry.id)));
        assert!(rendered.contains("tiffany-loop /queue run"));
        assert!(!rendered.contains("retry prompt:"));
    }

    #[test]
    fn jobs_cli_can_emit_full_retry_prompt_for_tui_handoff() {
        let tmp = tempfile::tempdir().unwrap();
        let store = orchestrator::core::session_store::SessionStore::open(
            &tmp.path().join("logs"),
            &tmp.path().join("db.sqlite"),
        )
        .unwrap();
        let prompt = "first line\nsecond\tline \\ slash";
        let failed = store
            .create_tui_job(prompt, "running", Some("single-worker"), Some("worker-cc"))
            .unwrap();
        store
            .set_tui_job_status(failed.id, "failed", None, Some("network timeout"))
            .unwrap();

        let rendered =
            retry_tui_job(&store, &short_uuid_for_cli(failed.id), 10, true, false).unwrap();

        assert!(rendered.contains("retry prompt: first line\\nsecond\\tline \\\\ slash"));
    }

    #[test]
    fn jobs_cli_tui_handoff_does_not_leave_duplicate_persisted_retry_job() {
        let tmp = tempfile::tempdir().unwrap();
        let store = orchestrator::core::session_store::SessionStore::open(
            &tmp.path().join("logs"),
            &tmp.path().join("db.sqlite"),
        )
        .unwrap();
        let failed = store
            .create_tui_job(
                "retry in active TUI",
                "running",
                Some("single-worker"),
                Some("worker-cc"),
            )
            .unwrap();
        store
            .set_tui_job_status(failed.id, "failed", None, Some("network timeout"))
            .unwrap();

        let rendered = retry_tui_job(&store, &short_uuid_for_cli(failed.id), 10, false, true)
            .expect("tui handoff");
        let jobs = store.list_tui_jobs(10).unwrap();

        assert_eq!(
            jobs.len(),
            1,
            "handoff should not create a stale queued retry job"
        );
        assert_eq!(jobs[0].id, failed.id);
        assert_eq!(jobs[0].status, "failed");
        assert!(rendered.contains("prepared for TUI retry"));
        assert!(rendered.contains("queued in current TUI input queue"));
        assert!(rendered.contains("retry prompt: retry in active TUI"));
        assert!(rendered.contains("/queue run"));
    }

    #[test]
    fn jobs_cli_rejects_retry_for_active_job() {
        let tmp = tempfile::tempdir().unwrap();
        let store = orchestrator::core::session_store::SessionStore::open(
            &tmp.path().join("logs"),
            &tmp.path().join("db.sqlite"),
        )
        .unwrap();
        let running = store
            .create_tui_job(
                "active prompt",
                "running",
                Some("single-worker"),
                Some("worker-cc"),
            )
            .unwrap();

        let err = retry_tui_job(&store, &short_uuid_for_cli(running.id), 10, false, false)
            .expect_err("running job should not retry");

        assert!(format!("{err:#}").contains("is running"));
        assert_eq!(store.list_tui_jobs(10).unwrap().len(), 1);
    }

    #[test]
    fn tui_job_progress_attaches_worker_session_and_native_handle() {
        let tmp = tempfile::tempdir().unwrap();
        let store = orchestrator::core::session_store::SessionStore::open(
            &tmp.path().join("logs"),
            &tmp.path().join("db.sqlite"),
        )
        .unwrap();
        let job = store
            .create_tui_job("continue worker", "running", None, Some("worker-cc"))
            .unwrap();
        let task_id = uuid::Uuid::new_v4();
        let thread_id = uuid::Uuid::new_v4();

        attach_tui_job_progress(
            &store,
            job.id,
            &orchestrator::pipeline::orchestrator::RunProgress::WorkerStarted {
                task_id,
                agent: "claude-code".to_string(),
                role: "worker-cc".to_string(),
                runtime: "claude-code".to_string(),
                cc_agent: None,
                model: "claude-sonnet-4-6".to_string(),
                provider: Some("anthropic".to_string()),
                prompt: "continue worker".to_string(),
            },
        );
        attach_tui_job_progress(
            &store,
            job.id,
            &orchestrator::pipeline::orchestrator::RunProgress::WorkerThreadReady {
                task_id,
                role: "worker-cc".to_string(),
                thread_id,
                native_session_id: Some("claude-native-session".to_string()),
                reused: true,
            },
        );

        let job = store.get_tui_job(job.id).unwrap().expect("job");
        assert_eq!(job.task_id, Some(task_id));
        assert_eq!(job.session_id, None);
        assert_eq!(job.worker_thread_id, Some(thread_id));
        assert_eq!(
            job.native_session_id.as_deref(),
            Some("claude-native-session")
        );
        let mut session = Session::new(task_id, "claude-code", Role::Worker);
        session.worker_thread_id = Some(thread_id);
        session.native_session_id = Some("claude-native-session".to_string());
        store.finalize(&session).unwrap();
        attach_tui_job_progress(
            &store,
            job.id,
            &orchestrator::pipeline::orchestrator::RunProgress::WorkerDone {
                task_id,
                agent: "claude-code".to_string(),
                role: "worker-cc".to_string(),
                duration_ms: 125,
                ok: true,
            },
        );
        let job = store.get_tui_job(job.id).unwrap().expect("job");
        assert_eq!(job.session_id, Some(session.id));
        let rendered = format_tui_jobs_cli(&store, 5).unwrap();
        assert!(rendered.contains("task "));
        assert!(rendered.contains("session "));
        assert!(rendered.contains("thread "));
        assert!(rendered.contains("native claude-native-session"));
    }

    #[test]
    fn worker_thread_export_writes_last_session_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let store = orchestrator::core::session_store::SessionStore::open(
            &tmp.path().join("logs"),
            &tmp.path().join("db.sqlite"),
        )
        .unwrap();
        let mut session = Session::new(uuid::Uuid::new_v4(), "claude-code", Role::Worker);
        session.ended_at = Some(chrono::Utc::now());
        store.finalize(&session).unwrap();
        store
            .append(&orchestrator::core::types::Event {
                session_id: session.id,
                task_id: session.task_id,
                ts: chrono::Utc::now(),
                kind: "assistant".to_string(),
                payload: serde_json::json!({"text": "handoff ready"}),
            })
            .unwrap();
        let thread = store
            .get_or_create_worker_thread(
                "worker-cc",
                "claude-code",
                "claude-code",
                "claude-sonnet-4-6",
                Some("anthropic"),
            )
            .unwrap();
        store
            .update_worker_thread_after_session(
                thread.id,
                Some("claude-native-session"),
                session.id,
                None,
            )
            .unwrap();
        let thread = store.worker_thread_by_role("worker-cc").unwrap().unwrap();
        let out = tmp.path().join("exports").join("worker-cc.md");

        let rendered = export_worker_thread_session(
            &store,
            &thread,
            crate::SessionExportFormatArg::Markdown,
            Some(&out),
            false,
        )
        .unwrap();

        assert!(rendered.contains("role: worker-cc"));
        assert!(rendered.contains("target:"));
        assert!(rendered.contains("worker-cc.md"));
        let body = std::fs::read_to_string(out).unwrap();
        assert!(body.contains("# Tiffany session"));
        assert!(body.contains("handoff ready"));
    }

    #[test]
    fn import_native_sessions_file_writes_typed_events_to_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = orchestrator::core::session_store::SessionStore::open(
            &tmp.path().join("logs"),
            &tmp.path().join("db.sqlite"),
        )
        .unwrap();
        let path = tmp.path().join("native-sessions.json");
        std::fs::write(
            &path,
            r#"{
              "version": 1,
              "conversations": [
                {
                  "id": "tiffany-native-test",
                  "cwd": "/tmp/project",
                  "created_at_unix": 10,
                  "updated_at_unix": 20,
                  "turns": [
                    {
                      "user_prompt": "改 README",
                      "result": "已修改",
                      "captured_at_unix": 20,
                      "events": [
                        {
                          "role": "worker",
                          "status": "output",
                          "title": "worker diff · worker-cc · claude-code",
                          "kind": "diff",
                          "content": "diff --git a/README.md b/README.md",
                          "agent": "claude-code",
                          "worker_role": "worker-cc",
                          "model": "claude-sonnet-4-6",
                          "provider": "anthropic",
                          "task_id": "task-1",
                          "worker_thread_id": "thread-1",
                          "native_session_id": "native-1"
                        },
                        {
                          "role": "worker",
                          "status": "output",
                          "title": "worker answer · worker-cc · claude-code",
                          "kind": "answer",
                          "content": "已读取规则，准备实现。",
                          "agent": "claude-code",
                          "worker_role": "worker-cc",
                          "model": "claude-sonnet-4-6",
                          "provider": "anthropic",
                          "task_id": "task-answer",
                          "worker_thread_id": "thread-1",
                          "native_session_id": "native-1"
                        },
                        {
                          "role": "worker",
                          "status": "output",
                          "title": "worker tool call · worker-cc · claude-code",
                          "kind": "tool_call",
                          "content": "tool Bash: cargo test -q",
                          "agent": "claude-code",
                          "worker_role": "worker-cc",
                          "model": "claude-sonnet-4-6",
                          "provider": "anthropic",
                          "task_id": "task-tool",
                          "worker_thread_id": "thread-1",
                          "native_session_id": "native-1"
                        },
                        {
                          "role": "worker",
                          "status": "output",
                          "title": "worker tool result · worker-cc · claude-code",
                          "kind": "tool_result",
                          "content": "tool shell result: exit 0\nok",
                          "agent": "claude-code",
                          "worker_role": "worker-cc",
                          "model": "claude-sonnet-4-6",
                          "provider": "anthropic",
                          "task_id": "task-tool-result",
                          "worker_thread_id": "thread-1",
                          "native_session_id": "native-1"
                        },
                        {
                          "role": "worker",
                          "status": "output",
                          "title": "worker approval · worker-cc · claude-code",
                          "kind": "approval",
                          "content": "waiting for command approval: rm -rf target",
                          "agent": "claude-code",
                          "worker_role": "worker-cc",
                          "model": "claude-sonnet-4-6",
                          "provider": "anthropic",
                          "task_id": "task-approval",
                          "worker_thread_id": "thread-1",
                          "native_session_id": "native-1"
                        },
                        {
                          "role": "worker",
                          "status": "output",
                          "title": "worker stderr · worker-codex · codex",
                          "kind": "stderr",
                          "content": "API Error: 400 [1211][模型不存在]",
                          "agent": "codex",
                          "worker_role": "worker-codex",
                          "model": "MiniMax-M3",
                          "provider": "minimax",
                          "task_id": "task-stderr",
                          "worker_thread_id": "thread-codex",
                          "native_session_id": "codex-native"
                        }
                      ]
                    }
                  ]
                }
              ]
            }"#,
        )
        .unwrap();

        let report = import_native_sessions_file(&store, &path).unwrap();
        let conversation = store
            .native_conversation_by_cwd("/tmp/project")
            .unwrap()
            .expect("imported native conversation");

        assert_eq!(report.conversations, 1);
        assert_eq!(report.turns, 1);
        assert_eq!(report.events, 6);
        assert_eq!(conversation.id, "tiffany-native-test");
        assert_eq!(conversation.turns[0].user_prompt, "改 README");
        let events = &conversation.turns[0].events;
        let kinds = events
            .iter()
            .map(|event| event.kind.as_deref().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                "diff",
                "answer",
                "tool_call",
                "tool_result",
                "approval",
                "stderr"
            ]
        );
        assert_eq!(events[0].kind.as_deref(), Some("diff"));
        assert_eq!(
            events[0].content.as_deref(),
            Some("diff --git a/README.md b/README.md")
        );
        assert_eq!(events[0].worker_role.as_deref(), Some("worker-cc"));
        assert_eq!(
            events[2].content.as_deref(),
            Some("tool Bash: cargo test -q")
        );
        assert_eq!(
            events[3].content.as_deref(),
            Some("tool shell result: exit 0\nok")
        );
        assert_eq!(
            events[4].content.as_deref(),
            Some("waiting for command approval: rm -rf target")
        );
        assert_eq!(events[5].worker_role.as_deref(), Some("worker-codex"));
        assert_eq!(
            events[5].content.as_deref(),
            Some("API Error: 400 [1211][模型不存在]")
        );
    }

    #[test]
    fn native_history_cli_filters_and_formats_readable_text() {
        let conversation = NativeConversation {
            id: "conv-1".to_string(),
            cwd: "/tmp/project".to_string(),
            created_at_unix: 1,
            updated_at_unix: 2,
            turns: vec![NativeTurn {
                turn_index: 0,
                user_prompt: "改 README".to_string(),
                result: "done".to_string(),
                captured_at_unix: 2,
                events: vec![
                    NativeEvent {
                        event_index: 0,
                        role: "worker".to_string(),
                        status: "output".to_string(),
                        title: "worker diff · worker-cc".to_string(),
                        kind: Some("diff".to_string()),
                        content: Some(
                            "{\"message\":\"diff --git a/README.md b/README.md\"}".to_string(),
                        ),
                        agent: Some("claude-code".to_string()),
                        worker_role: Some("worker-cc".to_string()),
                        model: Some("claude".to_string()),
                        provider: Some("anthropic".to_string()),
                        task_id: Some("task-1".to_string()),
                        worker_thread_id: Some("abcdef12-0000".to_string()),
                        native_session_id: Some("claude-native-session".to_string()),
                    },
                    NativeEvent {
                        event_index: 1,
                        role: "worker".to_string(),
                        status: "output".to_string(),
                        title: "worker stderr · worker-codex".to_string(),
                        kind: Some("stderr".to_string()),
                        content: Some("codex error".to_string()),
                        agent: Some("codex".to_string()),
                        worker_role: Some("worker-codex".to_string()),
                        model: Some("gpt".to_string()),
                        provider: Some("openai".to_string()),
                        task_id: Some("task-2".to_string()),
                        worker_thread_id: Some("99999999-0000".to_string()),
                        native_session_id: Some("codex-native-session".to_string()),
                    },
                ],
            }],
        };
        let filter = NativeHistoryCliFilter::new(
            Some("worker-cc".to_string()),
            Some("abcdef12".to_string()),
            None,
            Some("diff".to_string()),
        );

        let filtered = filter_native_conversation_for_cli(conversation, &filter);
        let text = format_native_history_cli(Some(&filtered), "/tmp/project", &filter);

        assert_eq!(filtered.turns.len(), 1);
        assert_eq!(filtered.turns[0].events.len(), 1);
        assert!(text.contains("Native history"));
        assert!(text.contains("filter: role=worker-cc thread=abcdef12 kind=diff"));
        assert!(text.contains("events: 1"));
        assert!(text.contains("diff · worker-cc"));
        assert!(text.contains("thread: abcdef12-0000"));
        assert!(text.contains("native: claude-native-session"));
        assert!(text.contains("diff --git a/README.md b/README.md"));
        assert!(text.contains("tiffany-loop /continue open worker-cc"));
        assert!(!text.contains("worker-codex"));
        assert!(!text.contains("{\"message\""));
    }

    #[test]
    fn ab_worker_routes_choose_two_configured_workers() {
        let mut cfg = config_with_models();
        cfg.roles.insert(
            "worker-codex".to_string(),
            RoleConfig {
                model: "gpt4o".to_string(),
                runtime: "codex".to_string(),
                agent_teams: false,
            },
        );
        cfg.roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "sonnet".to_string(),
                runtime: "claude-code".to_string(),
                agent_teams: true,
            },
        );

        assert_eq!(
            ab_worker_routes(&cfg, None).unwrap(),
            ["worker-cc".to_string(), "worker-codex".to_string()]
        );
        assert_eq!(
            ab_worker_routes(&cfg, Some("codex")).unwrap(),
            ["worker-codex".to_string(), "worker-cc".to_string()]
        );
    }

    #[test]
    fn ab_worker_routes_can_include_gemini_worker() {
        let mut cfg = config_with_models();
        cfg.runtimes.insert(
            "gemini".to_string(),
            orchestrator::config::RuntimeConfig {
                kind: "subprocess".to_string(),
                binary: Some("gemini-test".to_string()),
                supports_mcp: false,
                supports_agent_teams: false,
            },
        );
        cfg.roles.insert(
            "worker-codex".to_string(),
            RoleConfig {
                model: "gpt4o".to_string(),
                runtime: "codex".to_string(),
                agent_teams: false,
            },
        );
        cfg.roles.insert(
            "worker-gemini".to_string(),
            RoleConfig {
                model: "gemini-pro".to_string(),
                runtime: "gemini".to_string(),
                agent_teams: false,
            },
        );

        assert_eq!(
            ab_worker_routes(&cfg, Some("gemini")).unwrap(),
            ["worker-gemini".to_string(), "worker-codex".to_string()]
        );
    }

    #[test]
    fn ab_worker_routes_require_two_distinct_workers() {
        let mut cfg = config_with_models();
        cfg.roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "sonnet".to_string(),
                runtime: "claude-code".to_string(),
                agent_teams: true,
            },
        );

        let err = ab_worker_routes(&cfg, None).unwrap_err();

        assert!(format!("{err:#}").contains("two distinct configured worker roles"));
    }

    #[test]
    fn ab_winner_prefers_success_then_smaller_score() {
        let summaries = vec![
            AbRunSummary {
                index: 0,
                route: "worker-cc".to_string(),
                completed_count: 1,
                score_bytes: 400,
                ok: true,
                error: None,
            },
            AbRunSummary {
                index: 1,
                route: "worker-codex".to_string(),
                completed_count: 0,
                score_bytes: 1,
                ok: false,
                error: Some("failed".to_string()),
            },
        ];
        assert_eq!(pick_ab_winner(&summaries).unwrap(), 0);

        let summaries = vec![
            AbRunSummary {
                index: 0,
                route: "worker-cc".to_string(),
                completed_count: 1,
                score_bytes: 400,
                ok: true,
                error: None,
            },
            AbRunSummary {
                index: 1,
                route: "worker-codex".to_string(),
                completed_count: 1,
                score_bytes: 40,
                ok: true,
                error: None,
            },
        ];
        assert_eq!(pick_ab_winner(&summaries).unwrap(), 1);
    }

    #[test]
    fn setup_default_config_keeps_runtimes_without_preloading_providers() {
        let cfg = default_setup_config();

        assert!(cfg.providers.is_empty());
        assert!(cfg.models.is_empty());
        assert!(cfg.roles.is_empty());
        assert!(cfg.runtimes.contains_key("claude-code"));
        assert!(cfg.runtimes.contains_key("codex"));
        assert!(cfg.runtimes.contains_key("gemini"));
    }

    #[test]
    fn setup_models_follow_configured_providers_only() {
        let mut cfg = default_setup_config();
        cfg.providers
            .insert("anthropic".to_string(), provider("anthropic"));
        cfg.providers
            .insert("openai".to_string(), provider("openai"));
        cfg.models.push(ModelConfig {
            id: "stale".to_string(),
            provider: "missing".to_string(),
            name: "stale-model".to_string(),
        });

        register_default_models_for_configured_providers(&mut cfg);

        assert!(cfg.models.iter().any(|model| model.id == "sonnet"));
        assert!(cfg.models.iter().any(|model| model.id == "gpt4o"));
        assert!(!cfg.models.iter().any(|model| model.id == "gemini-pro"));
        assert!(!cfg.models.iter().any(|model| model.id == "stale"));
        assert!(cfg
            .models
            .iter()
            .all(|model| cfg.providers.contains_key(&model.provider)));
    }

    #[test]
    fn setup_role_defaults_do_not_create_codex_worker_for_anthropic_only() {
        let mut cfg = default_setup_config();
        cfg.providers
            .insert("anthropic".to_string(), provider("anthropic"));
        register_default_models_for_configured_providers(&mut cfg);

        let assignments = default_role_assignments(&cfg).unwrap();

        assert!(assignments.iter().any(|item| item.role == "worker-cc"));
        assert!(!assignments.iter().any(|item| item.role == "worker-codex"));
        assert!(assignments.iter().all(|item| item.runtime == "claude-code"));
    }

    #[test]
    fn setup_role_defaults_use_codex_worker_for_openai_only() {
        let mut cfg = default_setup_config();
        cfg.providers
            .insert("openai".to_string(), provider("openai"));
        register_default_models_for_configured_providers(&mut cfg);

        let assignments = default_role_assignments(&cfg).unwrap();

        assert!(assignments.iter().any(|item| item.role == "worker-codex"));
        assert!(!assignments.iter().any(|item| item.role == "worker-cc"));
        assert!(assignments.iter().all(|item| item.runtime == "codex"));
        assert_eq!(
            default_tag_overrides(&Config {
                roles: assignments
                    .iter()
                    .map(|item| {
                        (
                            item.role.to_string(),
                            RoleConfig {
                                model: item.model.clone(),
                                runtime: item.runtime.to_string(),
                                agent_teams: item.agent_teams,
                            },
                        )
                    })
                    .collect(),
                ..Config::default()
            }),
            vec![
                ("refactor", "worker-codex".to_string()),
                ("boilerplate", "worker-codex".to_string()),
                ("test", "worker-codex".to_string())
            ]
        );
    }

    #[test]
    fn setup_role_defaults_use_gemini_worker_for_google_only() {
        let mut cfg = default_setup_config();
        cfg.providers
            .insert("google".to_string(), provider("google"));
        register_default_models_for_configured_providers(&mut cfg);

        let assignments = default_role_assignments(&cfg).unwrap();

        assert!(assignments.iter().any(|item| item.role == "worker-gemini"));
        assert!(!assignments.iter().any(|item| item.role == "worker-cc"));
        assert!(!assignments.iter().any(|item| item.role == "worker-codex"));
        assert!(assignments.iter().all(|item| item.runtime == "gemini"));
    }

    #[test]
    fn provider_setup_uses_minimax_preset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");

        setup_provider(&path, "minimax", None, None, None, None, false, false).unwrap();

        let body = std::fs::read_to_string(path).unwrap();
        assert!(body.contains("minimax:"));
        assert!(body.contains("type: openai"));
        assert!(body.contains("api_key: ${MINIMAX_API_KEY}"));
        assert!(body.contains("base_url: https://api.minimaxi.com/v1"));
    }

    #[test]
    fn provider_setup_rejects_key_and_env_together() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");

        let err = setup_provider(
            &path,
            "openai",
            None,
            Some("sk-test"),
            Some("OPENAI_API_KEY"),
            None,
            false,
            false,
        )
        .unwrap_err();

        assert!(format!("{:#}", err).contains("use either --key or --env"));
    }

    #[test]
    fn provider_setup_dry_run_does_not_write_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");

        setup_provider(&path, "deepseek", None, None, None, None, true, false).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn provider_delete_removes_configured_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");

        setup_provider(&path, "minimax", None, None, None, None, false, false).unwrap();
        delete_provider(&path, "minimax", false).unwrap();

        let body = std::fs::read_to_string(path).unwrap();
        assert!(!body.contains("minimax:"));
    }

    #[test]
    fn provider_setup_selector_preview_redacts_literal_key() {
        let preview = provider_setup_command_preview(
            "custom",
            "openai",
            None,
            Some("sk-test-secret"),
            Some("https://llm.example.com/v1"),
        );

        assert_eq!(
            preview,
            "config provider setup custom --type openai --key <redacted> --endpoint https://llm.example.com/v1"
        );
        assert!(!preview.contains("sk-test-secret"));
    }

    #[test]
    fn write_session_export_creates_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("session.md");

        write_session_export(&path, "body").unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), "body");
    }

    #[test]
    fn detached_run_record_roundtrips_and_resolves_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let record = DetachedRunRecord {
            id: "run-12345-99".into(),
            pid: 42,
            prompt: "finish work".into(),
            created_at: "2026-06-20T00:00:00Z".into(),
            log_path: dir.path().join("run-12345-99.log"),
            exit_path: dir.path().join("run-12345-99.exit"),
        };
        let path = dir.path().join("run-12345-99.json");

        write_detached_run_record(&path, &record).unwrap();
        let resolved = find_detached_run_record(dir.path(), "run-123").unwrap();
        let body = std::fs::read_to_string(resolved).unwrap();
        let parsed: DetachedRunRecord = serde_json::from_str(&body).unwrap();

        assert_eq!(parsed.id, record.id);
        assert_eq!(parsed.prompt, "finish work");
    }

    #[test]
    fn detached_run_state_reads_exit_code_file() {
        let dir = tempfile::tempdir().unwrap();
        let exit_path = dir.path().join("run.exit");
        let record = DetachedRunRecord {
            id: "run-1".into(),
            pid: 999_999,
            prompt: "done".into(),
            created_at: "2026-06-20T00:00:00Z".into(),
            log_path: dir.path().join("run.log"),
            exit_path: exit_path.clone(),
        };

        std::fs::write(&exit_path, "0\n").unwrap();
        assert_eq!(detached_run_state(&record), "completed");
        std::fs::write(&exit_path, "2\n").unwrap();
        assert_eq!(detached_run_state(&record), "exited 2");
    }

    #[test]
    fn detached_shell_script_quotes_arguments_and_writes_exit_code() {
        let script = detached_run_shell_script(
            Path::new("/tmp/orch bin"),
            &["events".into(), "can't fail".into()],
            Path::new("/tmp/run exit"),
        );

        assert!(script.contains("'/tmp/orch bin'"));
        assert!(script.contains("'can'\\''t fail'"));
        assert!(script.contains("'/tmp/run exit'"));
        assert!(script.contains("printf '%s\\n' \"$status\""));
    }

    #[test]
    fn status_actions_send_missing_config_to_setup() {
        assert_eq!(
            status_actions(false, &[], true, false),
            vec![
                StatusAction {
                    label: "next",
                    command: "orchestrator setup".into(),
                },
                StatusAction {
                    label: "check",
                    command: "orchestrator doctor".into(),
                },
            ]
        );
    }

    #[test]
    fn status_actions_launch_tiffany_when_ready() {
        assert_eq!(
            status_actions(true, &[], true, false),
            vec![
                StatusAction {
                    label: "next",
                    command: "tiffany-loop".into(),
                },
                StatusAction {
                    label: "check",
                    command: "orchestrator doctor".into(),
                },
            ]
        );
    }

    #[test]
    fn status_actions_explain_legacy_tui_override() {
        let actions = status_actions(true, &[], true, true);

        assert_eq!(actions[0].label, "next");
        assert!(actions[0].command.contains("ORCHESTRATOR_LEGACY_TUI"));
        assert_eq!(actions[1].command, "orchestrator doctor".to_string());
    }

    #[test]
    fn status_actions_explain_missing_tiffany_binary() {
        let actions = status_actions(true, &[], false, false);

        assert_eq!(actions[0].label, "next");
        assert!(actions[0].command.contains("install tiffany-loop"));
        assert!(actions[0].command.contains("./scripts/tiffany-dev"));
        assert_eq!(actions[1].command, "orchestrator doctor".to_string());
    }

    #[test]
    fn status_actions_repair_unhealthy_config_before_launch() {
        let issues = vec!["planner model missing-model missing".to_string()];
        let actions = status_actions(true, &issues, true, false);

        assert_eq!(actions[0].label, "fix role");
        assert!(actions[0].command.contains("/role"));
        assert!(actions[0]
            .command
            .contains("orchestrator roles register planner"));
        assert!(actions[0]
            .command
            .contains("--provider <provider> --model-name <api-model>"));
        assert!(actions[0].command.contains("--model missing-model"));
        assert_eq!(actions[1].command, "orchestrator doctor".to_string());
    }

    #[test]
    fn status_actions_focus_provider_only_gaps_on_provider_setup() {
        let issues = vec![
            "anthropic api key missing".to_string(),
            "openai api key missing".to_string(),
        ];
        let actions = status_actions(true, &issues, true, false);

        assert_eq!(actions[0].label, "fix auth");
        assert!(actions[0].command.contains("/provider env anthropic"));
        assert!(actions[0]
            .command
            .contains("config provider setup anthropic --env <ENV_NAME>"));
        assert!(!actions[0].command.contains("orchestrator setup"));
        assert_eq!(actions[1].command, "orchestrator doctor".to_string());
    }

    #[test]
    fn status_actions_call_out_missing_openai_compatible_endpoint() {
        let issues = vec!["minimax base_url missing".to_string()];
        let actions = status_actions(true, &issues, true, false);

        assert_eq!(actions[0].label, "fix endpoint");
        assert!(actions[0]
            .command
            .contains("/provider endpoint minimax <url>"));
        assert!(actions[0]
            .command
            .contains("config provider setup minimax --endpoint <url>"));
        assert_eq!(actions[1].command, "orchestrator doctor".to_string());
    }

    #[test]
    fn status_actions_split_mixed_provider_and_role_repairs() {
        let issues = vec![
            "google api key missing".to_string(),
            "gpt4o provider openai missing".to_string(),
            "worker-cc runtime claude-code missing".to_string(),
        ];
        let actions = status_actions(true, &issues, true, false);

        assert_eq!(actions[0].label, "fix auth");
        assert!(actions[0].command.contains("/provider env google"));
        assert_eq!(actions[1].label, "fix provider");
        assert!(actions[1].command.contains("provider `openai`"));
        assert_eq!(actions[2].label, "fix runtime");
        assert!(actions[2].command.contains("/role worker-cc"));
        assert!(actions[2]
            .command
            .contains("--provider <provider> --model-name <api-model>"));
        assert_eq!(actions[3].command, "orchestrator doctor".to_string());
    }

    #[test]
    fn status_actions_offer_worker_registration_when_default_worker_missing() {
        let issues = vec!["no default worker".to_string()];
        let actions = status_actions(true, &issues, true, false);

        assert_eq!(actions[0].label, "add worker");
        assert!(actions[0]
            .command
            .contains("orchestrator roles register worker-cc"));
        assert!(actions[0].command.contains("--agent-teams"));
        assert_eq!(actions[1].command, "orchestrator doctor".to_string());
    }

    #[test]
    fn status_name_summary_sorts_deduplicates_and_truncates() {
        assert_eq!(status_name_summary(Vec::new(), 4), "0");
        assert_eq!(
            status_name_summary(
                vec![
                    "worker".to_string(),
                    "planner".to_string(),
                    "worker".to_string(),
                    "critic".to_string(),
                    "reviewer".to_string(),
                ],
                3,
            ),
            "4 (critic, planner, reviewer, +1)"
        );
    }

    #[test]
    fn status_config_health_reports_ok_for_wired_config() {
        let mut cfg = config_with_models();
        cfg.providers
            .insert("anthropic".to_string(), provider("anthropic"));
        cfg.providers
            .insert("openai".to_string(), provider("openai"));
        cfg.roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "sonnet".to_string(),
                runtime: "claude-code".to_string(),
                agent_teams: true,
            },
        );

        assert_eq!(status_config_health(&cfg), "ok");
    }

    #[test]
    fn status_config_health_ignores_unused_provider_and_model_gaps() {
        let mut cfg = config_with_models();
        cfg.providers
            .insert("anthropic".to_string(), provider("anthropic"));
        cfg.providers.insert(
            "google".to_string(),
            ProviderConfig {
                kind: "google".to_string(),
                api_key: None,
                base_url: None,
            },
        );
        cfg.models.push(ModelConfig {
            id: "unused-openai".to_string(),
            provider: "openai".to_string(),
            name: "gpt-4o".to_string(),
        });
        cfg.roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "sonnet".to_string(),
                runtime: "claude-code".to_string(),
                agent_teams: true,
            },
        );

        let issues = status_config_issues(&cfg);

        assert_eq!(issues, Vec::<String>::new());
        assert_eq!(status_config_health(&cfg), "ok");
    }

    #[test]
    fn status_config_health_reports_role_linked_missing_provider() {
        let mut cfg = config_with_models();
        cfg.roles.insert(
            "worker-codex".to_string(),
            RoleConfig {
                model: "gpt4o".to_string(),
                runtime: "codex".to_string(),
                agent_teams: false,
            },
        );

        let issues = status_config_issues(&cfg);

        assert!(issues.contains(&"gpt4o provider openai missing".to_string()));
        assert!(status_config_health(&cfg).contains("model provider links"));
    }

    #[test]
    fn status_config_health_reports_internal_config_gaps() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                kind: "openai".to_string(),
                api_key: Some(String::new()),
                base_url: None,
            },
        );
        cfg.models.push(ModelConfig {
            id: "gpt".to_string(),
            provider: "openai".to_string(),
            name: "gpt-test".to_string(),
        });
        cfg.roles.insert(
            "planner".to_string(),
            RoleConfig {
                model: "gpt".to_string(),
                runtime: "missing-runtime".to_string(),
                agent_teams: false,
            },
        );

        let issues = status_config_issues(&cfg);

        assert!(issues.contains(&"openai api key missing".to_string()));
        assert!(issues.contains(&"planner runtime missing-runtime missing".to_string()));
        assert!(issues.contains(&"no default worker".to_string()));
        let health = status_config_health(&cfg);
        assert!(health.contains("issue(s):"));
        assert!(health.contains("provider auth missing for 1: openai"));
        assert!(health.contains("role/model wiring:"));
    }

    #[test]
    fn status_config_health_reports_missing_openai_compatible_endpoint() {
        let mut cfg = config_with_models();
        cfg.providers.insert(
            "minimax".to_string(),
            ProviderConfig {
                kind: "openai".to_string(),
                api_key: Some("set".to_string()),
                base_url: None,
            },
        );
        cfg.models.push(ModelConfig {
            id: "minimax-m3".to_string(),
            provider: "minimax".to_string(),
            name: "MiniMax-M3".to_string(),
        });
        cfg.roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "minimax-m3".to_string(),
                runtime: "claude-code".to_string(),
                agent_teams: true,
            },
        );

        let issues = status_config_issues(&cfg);

        assert!(issues.contains(&"minimax base_url missing".to_string()));
        assert!(status_config_health(&cfg).contains("minimax base_url missing"));
    }

    #[test]
    fn status_config_health_accepts_runtime_aliases() {
        let mut cfg = config_with_models();
        cfg.providers
            .insert("anthropic".to_string(), provider("anthropic"));
        cfg.roles.insert(
            "worker-cc".to_string(),
            RoleConfig {
                model: "sonnet".to_string(),
                runtime: "claude".to_string(),
                agent_teams: true,
            },
        );

        let issues = status_config_issues(&cfg);

        assert!(!issues.contains(&"worker-cc runtime claude missing".to_string()));
    }

    #[test]
    fn status_issue_summary_groups_provider_auth_noise() {
        let issues = vec![
            "anthropic api key missing".to_string(),
            "google api key missing".to_string(),
            "minimax api key missing".to_string(),
            "openai api key missing".to_string(),
            "gpt4o provider openai missing".to_string(),
            "worker-cc runtime claude-code missing".to_string(),
        ];

        let summary = status_issue_summary(&issues, 3);

        assert!(summary.contains("provider auth missing for 4: anthropic, google, minimax, openai"));
        assert!(summary.contains("model provider links: gpt4o provider openai missing"));
        assert!(summary.contains("role/model wiring: worker-cc runtime claude-code missing"));
    }
}
