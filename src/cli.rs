//! CLI dispatch.

use anyhow::{Context, Result};
use orchestrator::config::Config;
use orchestrator::core::types::{Role, Session, Task, TaskStatus};
use orchestrator::pipeline::orchestrator::Orchestrator;
use orchestrator::roles::ab_judge::AbJudge;
use orchestrator::session_export::SessionExportFormat;
use orchestrator::tiffany_events::{TiffanyProgressEvent, TiffanyTextProgressFormatter};
use orchestrator::tiffany_install;
use orchestrator::{adapters, cc_config, mux, roles, runtime, storage};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

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

            while let Some(event) = rx.recv().await {
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

            run.await??;
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
            }
            Ok(())
        }

        crate::Cmd::Roles { action } => handle_roles(config_path, action),

        crate::Cmd::Status => {
            print_status(config_path)?;
            Ok(())
        }

        crate::Cmd::Doctor => {
            println!("{}", orchestrator::doctor::run(config_path).render_text());
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
            provs.sort_by(|a, b| b.1.tokens_in.cmp(&a.1.tokens_in));
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
        _ => (2, name.to_string()),
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

    for (provider_name, provider) in &cfg.providers {
        if provider.kind != "ollama"
            && provider
                .api_key
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
        {
            issues.push(format!("{provider_name} api key missing"));
        }
        if provider_needs_base_url(provider_name, provider) {
            issues.push(format!("{provider_name} base_url missing"));
        }
    }

    let model_ids = cfg
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<HashSet<_>>();
    for model in &cfg.models {
        if !cfg.providers.contains_key(&model.provider) {
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
    }
}

fn print_roles(config_path: &Path, selected_role: Option<&str>) -> Result<()> {
    let cfg = Config::load(config_path)?;
    if let Some(role) = selected_role {
        let Some(role_cfg) = cfg.roles.get(role) else {
            anyhow::bail!(
                "unknown role '{}'. Available: {}",
                role,
                available_roles_for_cli(&cfg)
            );
        };
        println!("{}", role_detail_for_cli(&cfg, role, role_cfg));
        return Ok(());
    }

    println!("Registered roles:");
    let mut roles = cfg.roles.iter().collect::<Vec<_>>();
    roles.sort_by(|a, b| a.0.cmp(b.0));
    if roles.is_empty() {
        println!("  (none)");
    }
    for (role, role_cfg) in roles {
        println!("  {}", role_detail_for_cli(&cfg, role, role_cfg));
    }
    println!(
        "\nRegister: orchestrator roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime-id>"
    );
    Ok(())
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
    let Some(runtime_cfg) = cfg.runtimes.get(runtime) else {
        anyhow::bail!(
            "unknown runtime '{}'. Available: {}",
            runtime,
            available_runtimes_for_cli(&cfg)
        );
    };

    let resolved_model_id = resolve_role_model_id(&cfg, model, provider, model_name)?;
    let existing_model = cfg.models.iter().find(|m| m.id == resolved_model_id);
    let model_write = match existing_model {
        Some(existing) if provider.is_some() || model_name.is_some() => {
            let provider = provider.unwrap_or(existing.provider.as_str());
            if !cfg.providers.contains_key(provider) {
                anyhow::bail!(
                    "unknown provider '{}'. Available: {}",
                    provider,
                    available_providers_for_cli(&cfg)
                );
            }
            Some(orchestrator::config::ModelConfig {
                id: resolved_model_id.clone(),
                provider: provider.to_string(),
                name: model_name.unwrap_or(existing.name.as_str()).to_string(),
            })
        }
        Some(_) => None,
        None => {
            let Some(provider) = provider else {
                anyhow::bail!(
                    "unknown model '{}'. Available: {}\nTo register it inline, add --provider <provider> --model-name <provider-model-name>.",
                    resolved_model_id,
                    available_models_for_cli(&cfg)
                );
            };
            if !cfg.providers.contains_key(provider) {
                anyhow::bail!(
                    "unknown provider '{}'. Available: {}",
                    provider,
                    available_providers_for_cli(&cfg)
                );
            }
            Some(orchestrator::config::ModelConfig {
                id: resolved_model_id.clone(),
                provider: provider.to_string(),
                name: model_name.unwrap_or(resolved_model_id.as_str()).to_string(),
            })
        }
    };

    if let Some(model_cfg) = &model_write {
        Config::write_model_to_config_file(config_path, model_cfg)?;
        println!(
            "✓ model {} registered: provider={} name={}",
            model_cfg.id, model_cfg.provider, model_cfg.name
        );
    }

    let teams = if no_agent_teams {
        false
    } else if agent_teams {
        true
    } else {
        default_agent_teams(role, runtime, runtime_cfg)
    };
    let role_cfg = orchestrator::config::RoleConfig {
        model: resolved_model_id.clone(),
        runtime: runtime.to_string(),
        agent_teams: teams,
    };
    Config::write_role_to_config_file(config_path, role, &role_cfg)?;
    println!("✓ role {} registered", role);
    println!("  model: {}", resolved_model_id);
    println!("  runtime: {}", runtime);
    println!("  agent teams: {}", teams);
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
) -> String {
    let model_entry = cfg.models.iter().find(|m| m.id == role_cfg.model);
    let model = model_entry
        .map(|m| m.id.as_str())
        .unwrap_or(role_cfg.model.as_str());
    let provider = model_entry.map(|m| m.provider.as_str()).unwrap_or("-");
    let api_model = model_entry.map(|m| m.name.as_str()).unwrap_or("-");
    let health = role_health_for_cli(cfg, role_cfg, model_entry);
    format!(
        "{:<14} model={:<18} provider={:<12} api_model={:<28} runtime={:<12} teams={} health={}",
        role, model, provider, api_model, role_cfg.runtime, role_cfg.agent_teams, health
    )
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
    for role in ["planner", "critic", "reviewer", "worker-cc", "worker-codex"] {
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

    let Some(planner) = codex_primary.clone().or_else(|| claude_primary.clone()) else {
        anyhow::bail!(
            "no configured model can drive Claude Code or Codex runtimes; add anthropic/minimax/openai-compatible/ollama provider first"
        );
    };
    let planner_runtime = if codex_primary.as_deref() == Some(planner.as_str()) {
        "codex"
    } else {
        "claude-code"
    };

    let critic = claude_smart
        .clone()
        .or_else(|| codex_primary.clone())
        .expect("planner availability checked above");
    let critic_runtime = if claude_smart.as_deref() == Some(critic.as_str()) {
        "claude-code"
    } else {
        "codex"
    };

    let reviewer = codex_cheap
        .clone()
        .or_else(|| claude_cheap.clone())
        .expect("planner availability checked above");
    let reviewer_runtime = if codex_cheap.as_deref() == Some(reviewer.as_str()) {
        "codex"
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
    Ok(assignments)
}

#[derive(Clone, Copy)]
enum RuntimeTarget {
    Claude,
    Codex,
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
    }
}

fn default_tag_overrides(cfg: &Config) -> Vec<(&'static str, String)> {
    let refactor_role = if cfg.roles.contains_key("worker-cc") {
        "worker-cc"
    } else {
        "worker-codex"
    };
    let fast_role = if cfg.roles.contains_key("worker-codex") {
        "worker-codex"
    } else {
        refactor_role
    };

    [
        ("refactor", refactor_role),
        ("boilerplate", fast_role),
        ("test", fast_role),
    ]
    .into_iter()
    .filter(|(_, role)| cfg.roles.contains_key(*role))
    .map(|(tag, role)| (tag, role.to_string()))
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
    fn role_detail_for_cli_surfaces_provider_api_model_and_health() {
        let mut cfg = config_with_models();
        cfg.providers
            .insert("anthropic".to_string(), provider("anthropic"));

        let planner = cfg.roles.get("planner").unwrap();
        let detail = role_detail_for_cli(&cfg, "planner", planner);
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
        let detail = role_detail_for_cli(&cfg, "broken", &broken);
        assert!(detail.contains("health=model-missing:missing-model"));
        assert!(detail.contains("runtime-missing:missing-runtime"));
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
            provider: "missing-provider".to_string(),
            name: "gpt-test".to_string(),
        });
        cfg.roles.insert(
            "planner".to_string(),
            RoleConfig {
                model: "missing-model".to_string(),
                runtime: "missing-runtime".to_string(),
                agent_teams: false,
            },
        );

        let issues = status_config_issues(&cfg);

        assert!(issues.contains(&"openai api key missing".to_string()));
        assert!(issues.contains(&"gpt provider missing-provider missing".to_string()));
        assert!(issues.contains(&"planner model missing-model missing".to_string()));
        assert!(issues.contains(&"planner runtime missing-runtime missing".to_string()));
        assert!(issues.contains(&"no default worker".to_string()));
        let health = status_config_health(&cfg);
        assert!(health.contains("issue(s):"));
        assert!(health.contains("provider auth missing for 1: openai"));
        assert!(health.contains("model provider links:"));
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
