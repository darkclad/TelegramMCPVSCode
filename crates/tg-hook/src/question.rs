//! `PreToolUse` / `AskUserQuestion` hook payload.
//!
//! When Claude is about to call the `AskUserQuestion` tool, a `PreToolUse`
//! hook fires with the questions in `tool_input`. This module parses them,
//! renders them for Telegram (every option gets a globally-unique number so
//! the user can answer with bare digits), and turns a Telegram reply back
//! into an answer summary Claude can act on.

use anyhow::{Result, anyhow};
use serde_json::Value;

/// One selectable option within a [`Question`].
#[derive(Debug, Clone)]
pub struct QOption {
    /// Short option label — what Claude matches the answer against.
    pub label: String,
    /// Optional longer explanation of the option.
    pub description: Option<String>,
}

/// A single question from an `AskUserQuestion` tool call.
#[derive(Debug, Clone)]
pub struct Question {
    /// Short header tag (e.g. `Security scope`), if Claude supplied one.
    pub header: Option<String>,
    /// The full question text.
    pub text: String,
    /// `true` when the user may pick more than one option.
    pub multi_select: bool,
    /// The 2–4 options offered.
    pub options: Vec<QOption>,
}

/// The full set of questions in one `AskUserQuestion` call (1–4 questions).
#[derive(Debug, Clone)]
pub struct QuestionSet {
    /// The questions, in ask order.
    pub questions: Vec<Question>,
}

impl QuestionSet {
    /// Parse from a `PreToolUse` payload's `tool_input` object.
    ///
    /// # Errors
    ///
    /// Returns an error if `questions` is missing, not an array, or empty, or
    /// if a question is missing its text or options.
    pub fn from_tool_input(tool_input: &Value) -> Result<Self> {
        let arr = tool_input
            .get("questions")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("AskUserQuestion tool_input has no `questions` array"))?;
        let questions = arr.iter().map(parse_question).collect::<Result<Vec<_>>>()?;
        if questions.is_empty() {
            return Err(anyhow!("AskUserQuestion call has no questions"));
        }
        Ok(Self { questions })
    }

    /// Flat `(question_index, option_index)` list in global-numbering order.
    /// The number shown to the user is the position in this list, plus one.
    fn flat_options(&self) -> Vec<(usize, usize)> {
        let mut flat = Vec::new();
        for (qi, q) in self.questions.iter().enumerate() {
            for oi in 0..q.options.len() {
                flat.push((qi, oi));
            }
        }
        flat
    }

    /// Render the questions as a Telegram message. Every option is given a
    /// globally-unique number so the user can answer with bare digits.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out =
            String::from("🔵 Claude needs your input — reply with the option number(s).\n");
        let mut n = 0;
        for q in &self.questions {
            out.push('\n');
            if let Some(h) = &q.header {
                let _ = writeln!(out, "▸ {h}");
            }
            let _ = writeln!(out, "{}", q.text);
            for opt in &q.options {
                n += 1;
                match &opt.description {
                    Some(d) => {
                        let _ = writeln!(out, "   {n}. {} — {d}", opt.label);
                    }
                    None => {
                        let _ = writeln!(out, "   {n}. {}", opt.label);
                    }
                }
            }
            if q.multi_select {
                out.push_str("   (multi-select — list every number that applies)\n");
            }
        }
        out.push_str(
            "\nReply with one number per question (e.g. \"1, 4\"); for a multi-select \
             question list each number you want.",
        );
        out
    }

    /// Turn a Telegram reply into an answer summary for Claude.
    ///
    /// Bare numbers in `reply` are matched against the global option numbers
    /// from [`Self::render`]. If none match, the raw reply is passed through
    /// verbatim so Claude can still interpret a free-text answer.
    #[must_use]
    pub fn answer_from_reply(&self, reply: &str) -> String {
        use std::fmt::Write as _;
        let flat = self.flat_options();
        let picks: Vec<usize> = reply
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|s| s.parse::<usize>().ok())
            .filter(|&n| n >= 1 && n <= flat.len())
            .collect();

        let trimmed = reply.trim();
        if picks.is_empty() {
            return format!(
                "The user answered the AskUserQuestion prompt via Telegram with a \
                 free-text reply: \"{trimmed}\". Interpret it as their answer to the \
                 question(s) and continue — do not call AskUserQuestion again.",
            );
        }

        // Group the picked options by their question.
        let mut per_question: Vec<Vec<String>> = vec![Vec::new(); self.questions.len()];
        for n in picks {
            let (qi, oi) = flat[n - 1];
            per_question[qi].push(self.questions[qi].options[oi].label.clone());
        }

        let mut out = String::from(
            "The user answered the AskUserQuestion prompt via Telegram (not the in-app \
             dialog):\n",
        );
        for (qi, q) in self.questions.iter().enumerate() {
            let label = q.header.as_deref().unwrap_or(q.text.as_str());
            let chosen = &per_question[qi];
            if chosen.is_empty() {
                let _ = writeln!(out, "• {label}: (no option picked)");
            } else {
                let _ = writeln!(out, "• {label}: {}", chosen.join("; "));
            }
        }
        out.push_str(
            "Use these answers and continue — do not call AskUserQuestion again for them.",
        );
        out
    }
}

/// Parse one question object from the `questions` array.
fn parse_question(v: &Value) -> Result<Question> {
    let text = v
        .get("question")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("question object missing `question` text"))?
        .to_string();
    let header = v.get("header").and_then(Value::as_str).map(str::to_string);
    let multi_select = v
        .get("multiSelect")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let options = v
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("question `{text}` has no `options` array"))?
        .iter()
        .map(|o| {
            let label = o
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("option missing `label`"))?
                .to_string();
            let description = o
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok(QOption { label, description })
        })
        .collect::<Result<Vec<_>>>()?;
    if options.is_empty() {
        return Err(anyhow!("question `{text}` has no options"));
    }
    Ok(Question {
        header,
        text,
        multi_select,
        options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "questions": [
                {
                    "question": "Pick a color",
                    "header": "Color",
                    "multiSelect": false,
                    "options": [
                        { "label": "Red", "description": "warm" },
                        { "label": "Blue", "description": "cool" }
                    ]
                },
                {
                    "question": "Pick toppings",
                    "header": "Toppings",
                    "multiSelect": true,
                    "options": [
                        { "label": "Cheese" },
                        { "label": "Olives" },
                        { "label": "Ham" }
                    ]
                }
            ]
        })
    }

    #[test]
    fn parses_tool_input() {
        let qs = QuestionSet::from_tool_input(&sample()).expect("parses");
        assert_eq!(qs.questions.len(), 2);
        assert_eq!(qs.questions[0].options.len(), 2);
        assert!(qs.questions[1].multi_select);
        assert!(!qs.questions[0].multi_select);
        assert_eq!(qs.questions[0].options[0].label, "Red");
    }

    #[test]
    fn render_numbers_options_globally() {
        let qs = QuestionSet::from_tool_input(&sample()).expect("parses");
        let text = qs.render();
        // 5 options across the 2 questions → global numbers 1..=5.
        assert!(text.contains("1. Red — warm"));
        assert!(text.contains("5. Ham"));
        assert!(text.contains("multi-select"));
    }

    #[test]
    fn answer_maps_numbers_to_labels() {
        let qs = QuestionSet::from_tool_input(&sample()).expect("parses");
        // 2 = Blue (Q1); 3 = Cheese and 5 = Ham (Q2, multi-select).
        let ans = qs.answer_from_reply("2, 3 and 5");
        assert!(ans.contains("Blue"));
        assert!(ans.contains("Cheese"));
        assert!(ans.contains("Ham"));
        assert!(!ans.contains("free-text"));
    }

    #[test]
    fn answer_falls_back_to_raw_text() {
        let qs = QuestionSet::from_tool_input(&sample()).expect("parses");
        let ans = qs.answer_from_reply("the blue one please");
        assert!(ans.contains("the blue one please"));
        assert!(ans.contains("free-text"));
    }

    #[test]
    fn out_of_range_numbers_are_ignored() {
        let qs = QuestionSet::from_tool_input(&sample()).expect("parses");
        // 99 is out of range; only 1 (Red) is a valid pick.
        let ans = qs.answer_from_reply("1, 99");
        assert!(ans.contains("Red"));
        assert!(!ans.contains("free-text"));
    }

    #[test]
    fn empty_or_missing_questions_errors() {
        assert!(QuestionSet::from_tool_input(&json!({ "questions": [] })).is_err());
        assert!(QuestionSet::from_tool_input(&json!({})).is_err());
    }
}
