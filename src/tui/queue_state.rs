//! Shared queue state helpers for terminal chat surfaces.

use super::state::InputState;
use super::util::truncate_chars;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct QueueItemPreview {
    pub(super) index: usize,
    pub(super) text: String,
    pub(super) job_id: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct QueueSnapshot {
    pub(super) count: usize,
    pub(super) paused: bool,
    pub(super) hidden_count: usize,
    pub(super) preview: Vec<QueueItemPreview>,
}

impl QueueSnapshot {
    pub(super) fn from_input(input: &InputState, preview_limit: usize) -> Self {
        Self::new_with_jobs(
            &input.queued_prompts,
            &input.queued_job_ids,
            input.queue_paused,
            preview_limit,
        )
    }

    #[cfg(test)]
    pub(super) fn new(prompts: &[String], paused: bool, preview_limit: usize) -> Self {
        Self::new_with_jobs(prompts, &[], paused, preview_limit)
    }

    pub(super) fn new_with_jobs(
        prompts: &[String],
        job_ids: &[Uuid],
        paused: bool,
        preview_limit: usize,
    ) -> Self {
        let count = prompts.len();
        let preview = prompts
            .iter()
            .take(preview_limit)
            .enumerate()
            .map(|(idx, prompt)| QueueItemPreview {
                index: idx + 1,
                text: prompt.clone(),
                job_id: job_ids.get(idx).copied(),
            })
            .collect::<Vec<_>>();
        Self {
            count,
            paused,
            hidden_count: count.saturating_sub(preview.len()),
            preview,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub(super) fn state_label(&self) -> &'static str {
        if self.paused {
            "paused"
        } else {
            "ready"
        }
    }

    pub(super) fn status_label(&self) -> &'static str {
        if self.paused {
            "paused"
        } else {
            "next"
        }
    }

    pub(super) fn batch_label(&self) -> &'static str {
        if self.paused {
            "paused"
        } else {
            "next batch"
        }
    }

    pub(super) fn pending_label(&self) -> String {
        if self.is_empty() {
            "empty".into()
        } else {
            format!("{} pending ({})", self.count, self.state_label())
        }
    }
}

fn short_job_id(job_id: Uuid) -> String {
    job_id.to_string().chars().take(8).collect()
}

pub(super) fn can_start_queued_batch(input: &InputState) -> bool {
    !run_is_active(input) && !input.queue_paused && !input.queued_prompts.is_empty()
}

pub(super) fn run_is_active(input: &InputState) -> bool {
    input.run_rx.is_some() || input.run_handle.is_some()
}

pub(super) fn queue_followup(input: &mut InputState, prompt: String) {
    input.queued_prompts.push(prompt);
}

pub(super) fn drain_queued_prompts(input: &mut InputState) -> String {
    let prompts = std::mem::take(&mut input.queued_prompts);
    merge_queued_prompts(prompts)
}

pub(super) fn merge_queued_prompts(prompts: Vec<String>) -> String {
    let mut prompts = prompts
        .into_iter()
        .map(|prompt| prompt.trim().to_string())
        .filter(|prompt| !prompt.is_empty())
        .collect::<Vec<_>>();

    match prompts.len() {
        0 => String::new(),
        1 => prompts.remove(0),
        _ => prompts
            .into_iter()
            .enumerate()
            .map(|(idx, prompt)| format!("{}. {}", idx + 1, prompt))
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

pub(super) fn format_queue_show(input: &InputState) -> String {
    let snapshot = QueueSnapshot::from_input(input, usize::MAX);
    if snapshot.is_empty() {
        return "Queue is empty.\n\nWhile a run is active, type a normal message to queue it."
            .into();
    }

    let mut out = format!(
        "Queued batch ({}) - {}:",
        snapshot.count,
        snapshot.state_label()
    );
    for item in &snapshot.preview {
        out.push('\n');
        let job = item
            .job_id
            .map(|job_id| format!("job {} · ", short_job_id(job_id)))
            .unwrap_or_default();
        out.push_str(&format!(
            "  ↳ {}. {}{}",
            item.index,
            job,
            truncate_chars(&item.text, 180)
        ));
    }
    if snapshot.paused {
        out.push_str("\n\nPaused: the batch will stay here until /queue resume or /queue run.");
    } else {
        out.push_str(
            "\n\nExecution: all queued messages are merged into one follow-up prompt and run together after the current task finishes.",
        );
    }
    out.push_str(
        "\nManage: /queue edit <n> <text>, /queue promote <n>, /queue remove <n>, /queue pause, /queue clear.",
    );
    out.push_str("\nJobs: /jobs shows persisted status, session, native id, and result.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_labels_empty_ready_and_paused_states() {
        let empty = QueueSnapshot::new(&[], false, 4);
        assert!(empty.is_empty());
        assert_eq!(empty.pending_label(), "empty");

        let prompts = vec!["one".to_string(), "two".to_string()];
        let ready = QueueSnapshot::new(&prompts, false, 1);
        assert_eq!(ready.count, 2);
        assert_eq!(ready.state_label(), "ready");
        assert_eq!(ready.status_label(), "next");
        assert_eq!(ready.batch_label(), "next batch");
        assert_eq!(ready.hidden_count, 1);
        assert_eq!(ready.preview[0].index, 1);

        let paused = QueueSnapshot::new(&prompts, true, 4);
        assert_eq!(paused.state_label(), "paused");
        assert_eq!(paused.status_label(), "paused");
        assert_eq!(paused.batch_label(), "paused");
        assert_eq!(paused.pending_label(), "2 pending (paused)");
    }

    #[test]
    fn snapshot_carries_job_ids_for_queue_preview() {
        let prompts = vec!["first".to_string(), "second".to_string()];
        let jobs = vec![
            Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap(),
        ];

        let snapshot = QueueSnapshot::new_with_jobs(&prompts, &jobs, false, 2);

        assert_eq!(snapshot.preview[0].job_id, Some(jobs[0]));
        assert_eq!(snapshot.preview[1].job_id, Some(jobs[1]));
    }

    #[test]
    fn queue_show_links_items_to_persisted_jobs() {
        let input = InputState {
            queued_prompts: vec!["write tests".to_string(), "update docs".to_string()],
            queued_job_ids: vec![
                Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
                Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap(),
            ],
            ..InputState::default()
        };

        let rendered = format_queue_show(&input);

        assert!(rendered.contains("1. job 11111111 · write tests"));
        assert!(rendered.contains("2. job 22222222 · update docs"));
        assert!(rendered.contains("/jobs shows persisted status"));
    }

    #[test]
    fn merge_queued_prompts_trims_empty_and_numbers_multi_item_batches() {
        assert_eq!(merge_queued_prompts(vec![]), "");
        assert_eq!(
            merge_queued_prompts(vec!["  single follow-up  ".to_string()]),
            "single follow-up"
        );
        assert_eq!(
            merge_queued_prompts(vec![
                " first ".to_string(),
                "".to_string(),
                "second".to_string()
            ]),
            "1. first\n\n2. second"
        );
    }
}
