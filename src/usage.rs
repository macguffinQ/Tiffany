//! Token usage tracking: aggregate from session store, enforce budget.

use crate::config::TokenPlan;
use crate::core::session_store::SessionStore;
use crate::core::types::Session;
use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub total_cost_usd: f64,
    pub by_provider: HashMap<String, ProviderUsage>,
    pub by_day: Vec<DayUsage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub session_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayUsage {
    pub date: String, // YYYY-MM-DD
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetStatus {
    /// Today's usage, kept as `usage` for compatibility with older callers.
    pub usage: Usage,
    pub monthly_usage: Usage,
    pub daily_limit: Option<u64>,
    pub monthly_limit_usd: Option<f64>,
    pub warn_at_percent: u8,
    pub daily_percent: Option<f64>,
    pub monthly_percent: Option<f64>,
    pub per_provider_limits: HashMap<String, u64>,
    pub per_provider_percent: HashMap<String, f64>,
    pub warnings: Vec<String>,
}

impl Usage {
    pub fn from_sessions(sessions: &[Session]) -> Self {
        let mut u = Usage::default();
        for s in sessions {
            u.total_tokens_in += s.token_in;
            u.total_tokens_out += s.token_out;
            u.total_cost_usd += s.cost_usd;
            // By provider (using the model name as proxy if agent is unknown)
            let provider_key = if s.model.is_empty() {
                s.agent.clone()
            } else {
                // Strip version suffixes
                let m: String = s.model.chars().take_while(|c| *c != '-').collect();
                if m.is_empty() {
                    s.model.clone()
                } else {
                    m
                }
            };
            let entry = u.by_provider.entry(provider_key).or_default();
            entry.tokens_in += s.token_in;
            entry.tokens_out += s.token_out;
            entry.cost_usd += s.cost_usd;
            entry.session_count += 1;
        }
        // Aggregate by day (using started_at)
        let mut by_day: HashMap<String, DayUsage> = HashMap::new();
        for s in sessions {
            let day = s.started_at.format("%Y-%m-%d").to_string();
            let entry = by_day.entry(day).or_insert(DayUsage {
                date: s.started_at.format("%Y-%m-%d").to_string(),
                tokens_in: 0,
                tokens_out: 0,
                cost_usd: 0.0,
            });
            entry.tokens_in += s.token_in;
            entry.tokens_out += s.token_out;
            entry.cost_usd += s.cost_usd;
        }
        let mut by_day: Vec<DayUsage> = by_day.into_values().collect();
        by_day.sort_by(|a, b| a.date.cmp(&b.date));
        u.by_day = by_day;
        u
    }
}

/// Compute usage for sessions in the given time window (e.g., "today" or "this month").
pub fn compute_for_window(store: &SessionStore, window: UsageWindow) -> Result<Usage> {
    let all = store.list(10_000)?;
    let now = Utc::now();
    let cutoff = match window {
        UsageWindow::All => DateTime::<Utc>::MIN_UTC,
        UsageWindow::Today => {
            let today = now.date_naive();
            today.and_hms_opt(0, 0, 0).unwrap().and_utc()
        }
        UsageWindow::ThisMonth => {
            // First day of the current month at 00:00:00
            use chrono::TimeZone;
            let year = now.year();
            let month = now.month();
            Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
                .single()
                .unwrap_or(now)
        }
        UsageWindow::LastDays(n) => now - Duration::days(n),
    };
    let filtered: Vec<Session> = all.into_iter().filter(|s| s.started_at >= cutoff).collect();
    Ok(Usage::from_sessions(&filtered))
}

pub fn compute_budget_status(
    store: &SessionStore,
    token_plan: &TokenPlan,
) -> Result<Option<BudgetStatus>> {
    if !token_plan.enabled {
        return Ok(None);
    }
    let today = compute_for_window(store, UsageWindow::Today)?;
    let month = compute_for_window(store, UsageWindow::ThisMonth)?;
    Ok(budget_status_from_usage(token_plan, today, month))
}

pub fn budget_status_from_usage(
    token_plan: &TokenPlan,
    today: Usage,
    month: Usage,
) -> Option<BudgetStatus> {
    if !token_plan.enabled {
        return None;
    }

    let warn_at_percent = token_plan.warn_at_percent.min(100);
    let daily_used = today.total_tokens_in + today.total_tokens_out;
    let daily_percent = token_plan
        .daily_limit
        .map(|limit| percent_u64(daily_used, limit));
    let monthly_percent = token_plan
        .monthly_limit_usd
        .map(|limit| percent_f64(month.total_cost_usd, limit));

    let mut warnings = vec![];
    push_budget_warning(
        &mut warnings,
        "Daily token budget",
        daily_used.to_string(),
        token_plan.daily_limit.map(|limit| limit.to_string()),
        daily_percent,
        warn_at_percent,
    );
    push_budget_warning(
        &mut warnings,
        "Monthly cost budget",
        format!("${:.4}", month.total_cost_usd),
        token_plan
            .monthly_limit_usd
            .map(|limit| format!("${:.4}", limit)),
        monthly_percent,
        warn_at_percent,
    );

    let mut per_provider_percent = HashMap::new();
    let mut provider_limits: Vec<_> = token_plan.per_provider.iter().collect();
    provider_limits.sort_by(|a, b| a.0.cmp(b.0));
    for (provider, limit) in provider_limits {
        let used = today
            .by_provider
            .get(provider)
            .map(|usage| usage.tokens_in + usage.tokens_out)
            .unwrap_or(0);
        let pct = percent_u64(used, *limit);
        per_provider_percent.insert(provider.clone(), pct);
        push_budget_warning(
            &mut warnings,
            &format!("Provider `{provider}` daily token budget"),
            used.to_string(),
            Some(limit.to_string()),
            Some(pct),
            warn_at_percent,
        );
    }

    Some(BudgetStatus {
        usage: today,
        monthly_usage: month,
        daily_limit: token_plan.daily_limit,
        monthly_limit_usd: token_plan.monthly_limit_usd,
        warn_at_percent,
        daily_percent,
        monthly_percent,
        per_provider_limits: token_plan.per_provider.clone(),
        per_provider_percent,
        warnings,
    })
}

pub fn format_budget_status(status: &BudgetStatus) -> String {
    let mut out = String::from("Budget alerts");
    if status.daily_limit.is_none()
        && status.monthly_limit_usd.is_none()
        && status.per_provider_limits.is_empty()
    {
        out.push_str("\n  enabled, but no limits are configured");
        return out;
    }

    if let Some(limit) = status.daily_limit {
        let used = status.usage.total_tokens_in + status.usage.total_tokens_out;
        out.push_str(&format!(
            "\n  daily tokens: {}/{} ({})",
            used,
            limit,
            format_percent(status.daily_percent)
        ));
    }
    if let Some(limit) = status.monthly_limit_usd {
        out.push_str(&format!(
            "\n  monthly cost: ${:.4}/${:.4} ({})",
            status.monthly_usage.total_cost_usd,
            limit,
            format_percent(status.monthly_percent)
        ));
    }

    if !status.per_provider_limits.is_empty() {
        out.push_str("\n  provider daily tokens:");
        let mut providers: Vec<_> = status.per_provider_limits.iter().collect();
        providers.sort_by(|a, b| a.0.cmp(b.0));
        for (provider, limit) in providers {
            let used = status
                .usage
                .by_provider
                .get(provider)
                .map(|usage| usage.tokens_in + usage.tokens_out)
                .unwrap_or(0);
            out.push_str(&format!(
                "\n    {:<18} {}/{} ({})",
                truncate_label(provider, 18),
                used,
                limit,
                format_percent(status.per_provider_percent.get(provider).copied())
            ));
        }
    }

    if status.warnings.is_empty() {
        out.push_str("\n  ok: below warning threshold");
    } else {
        out.push_str("\n  warnings:");
        for warning in &status.warnings {
            out.push_str("\n    ⚠ ");
            out.push_str(warning);
        }
    }
    out
}

fn push_budget_warning(
    warnings: &mut Vec<String>,
    label: &str,
    used: String,
    limit: Option<String>,
    percent: Option<f64>,
    warn_at_percent: u8,
) {
    let Some(limit) = limit else {
        return;
    };
    let Some(percent) = percent else {
        return;
    };
    if percent >= 100.0 {
        warnings.push(format!(
            "{label} exceeded: {used}/{limit} ({}).",
            format_percent_value(percent)
        ));
    } else if percent >= warn_at_percent as f64 {
        warnings.push(format!(
            "{label} warning: {used}/{limit} ({}) reached warn threshold {}%.",
            format_percent_value(percent),
            warn_at_percent
        ));
    }
}

fn percent_u64(used: u64, limit: u64) -> f64 {
    if limit == 0 {
        if used == 0 {
            0.0
        } else {
            f64::INFINITY
        }
    } else {
        (used as f64 / limit as f64) * 100.0
    }
}

fn percent_f64(used: f64, limit: f64) -> f64 {
    if limit <= 0.0 {
        if used <= 0.0 {
            0.0
        } else {
            f64::INFINITY
        }
    } else {
        (used / limit) * 100.0
    }
}

fn format_percent(percent: Option<f64>) -> String {
    percent
        .map(format_percent_value)
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_percent_value(percent: f64) -> String {
    if percent.is_infinite() {
        "over limit".to_string()
    } else {
        format!("{percent:.1}%")
    }
}

fn truncate_label(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push('…');
    }
    out
}

#[derive(Debug, Clone, Copy)]
pub enum UsageWindow {
    All,
    Today,
    ThisMonth,
    LastDays(i64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TokenPlan;
    use crate::core::types::{Role, Session};
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn aggregates_tokens() {
        let s1 = Session {
            id: Uuid::new_v4(),
            task_id: Uuid::nil(),
            agent: "claude-code".into(),
            role: Role::Worker,
            model: "claude-sonnet-4-6".into(),
            started_at: Utc::now(),
            ended_at: None,
            parent_session_ids: vec![],
            token_in: 1000,
            token_out: 500,
            cost_usd: 0.05,
            files_touched: vec![],
        };
        let s2 = Session {
            token_in: 2000,
            token_out: 800,
            cost_usd: 0.10,
            model: "gpt-4o".into(),
            ..s1.clone()
        };
        let u = Usage::from_sessions(&[s1, s2]);
        assert_eq!(u.total_tokens_in, 3000);
        assert_eq!(u.total_tokens_out, 1300);
        assert!((u.total_cost_usd - 0.15).abs() < 0.001);
        assert_eq!(u.by_provider.len(), 2);
    }

    #[test]
    fn budget_status_warns_for_daily_monthly_and_provider_limits() {
        let session = Session {
            id: Uuid::new_v4(),
            task_id: Uuid::nil(),
            agent: "claude-code".into(),
            role: Role::Worker,
            model: "claude-sonnet-4-6".into(),
            started_at: Utc::now(),
            ended_at: None,
            parent_session_ids: vec![],
            token_in: 700,
            token_out: 200,
            cost_usd: 9.0,
            files_touched: vec![],
        };
        let usage = Usage::from_sessions(&[session]);
        let plan = TokenPlan {
            enabled: true,
            daily_limit: Some(1_000),
            monthly_limit_usd: Some(10.0),
            warn_at_percent: 80,
            per_provider: HashMap::from([("claude".to_string(), 800)]),
        };

        let status = budget_status_from_usage(&plan, usage.clone(), usage).unwrap();

        assert_eq!(status.daily_percent, Some(90.0));
        assert_eq!(status.monthly_percent, Some(90.0));
        assert_eq!(status.per_provider_percent.get("claude"), Some(&112.5));
        assert_eq!(status.warnings.len(), 3);
        assert!(status
            .warnings
            .iter()
            .any(|warning| warning.contains("Daily token budget warning")));
        assert!(status
            .warnings
            .iter()
            .any(|warning| warning.contains("Monthly cost budget warning")));
        assert!(status
            .warnings
            .iter()
            .any(|warning| warning.contains("Provider `claude` daily token budget exceeded")));
    }

    #[test]
    fn budget_status_formats_no_limits_as_configuration_hint() {
        let plan = TokenPlan {
            enabled: true,
            ..TokenPlan::default()
        };
        let status = budget_status_from_usage(&plan, Usage::default(), Usage::default()).unwrap();

        assert_eq!(
            format_budget_status(&status),
            "Budget alerts\n  enabled, but no limits are configured"
        );
    }
}
