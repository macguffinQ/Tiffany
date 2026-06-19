//! Token usage tracking: aggregate from session store, enforce budget.

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
    pub usage: Usage,
    pub daily_limit: Option<u64>,
    pub monthly_limit_usd: Option<f64>,
    pub warn_at_percent: u8,
    pub daily_percent: Option<f64>,
    pub monthly_percent: Option<f64>,
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
}
