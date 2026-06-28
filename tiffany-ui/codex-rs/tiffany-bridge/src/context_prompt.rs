#[derive(Clone, Copy, Debug)]
pub struct ContextPromptTurn<'a> {
    pub user_prompt: &'a str,
    pub result: &'a str,
}

pub fn contextual_prompt(turns: &[ContextPromptTurn<'_>], current_prompt: &str) -> String {
    let current_prompt = current_prompt.trim();
    if turns.is_empty() {
        return current_prompt.to_string();
    }

    let recent = turns
        .iter()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|turn| {
            format!(
                "user:\n{}\n\nassistant result:\n{}",
                truncate_context_text(turn.user_prompt, 1_200),
                truncate_context_text(turn.result, 3_000)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    format!(
        "You are continuing a multi-turn tiffany-loop orchestrator conversation.\n\
         Use the previous turns to resolve follow-ups, pronouns, and references.\n\
         The current user request below is the highest priority.\n\n\
         Previous turns:\n{recent}\n\n\
         ---\n\
         Current user request:\n{current_prompt}",
    )
}

fn truncate_context_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = trimmed.chars().take(max_chars).collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_trimmed_current_prompt_without_history() {
        assert_eq!(contextual_prompt(&[], "  你好  "), "你好");
    }

    #[test]
    fn includes_recent_turns_and_current_request() {
        let turns = [
            ContextPromptTurn {
                user_prompt: "你好",
                result: "你好！有什么我可以帮你的吗？",
            },
            ContextPromptTurn {
                user_prompt: "你叫啥",
                result: "我是 tiffany-loop worker。",
            },
        ];

        let prompt = contextual_prompt(&turns, "你能干啥");

        assert!(prompt.contains("Previous turns:"));
        assert!(prompt.contains("user:\n你好"));
        assert!(prompt.contains("assistant result:\n我是 tiffany-loop worker。"));
        assert!(prompt.contains("Current user request:\n你能干啥"));
    }

    #[test]
    fn keeps_only_six_most_recent_turns() {
        let prompts = ["one", "two", "three", "four", "five", "six", "seven"];
        let turns = prompts
            .iter()
            .map(|prompt| ContextPromptTurn {
                user_prompt: prompt,
                result: "result",
            })
            .collect::<Vec<_>>();

        let prompt = contextual_prompt(&turns, "current");

        assert!(!prompt.contains("user:\none"));
        assert!(prompt.contains("user:\ntwo"));
        assert!(prompt.contains("user:\nseven"));
    }

    #[test]
    fn truncates_long_turn_text() {
        let user = "u".repeat(1_300);
        let result = "r".repeat(3_100);
        let turns = [ContextPromptTurn {
            user_prompt: &user,
            result: &result,
        }];

        let prompt = contextual_prompt(&turns, "current");

        assert!(prompt.contains(&format!("{}…", "u".repeat(1_200))));
        assert!(prompt.contains(&format!("{}…", "r".repeat(3_000))));
    }
}
