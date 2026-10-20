//! Claude single-question selection forms, distinct from permission menus.
use super::{navigation, option_row, plain, AnswerPlan, HarnessTui, Question, State, View};
pub struct ClaudeTui;
impl HarnessTui for ClaudeTui {
    fn working_followups(&self) -> bool {
        true
    }
    fn open_question_keys(&self) -> &'static [&'static str] {
        &[]
    }
    fn classify(&self, screen: &str) -> View {
        let clean = plain(screen);
        let all_rows: Vec<_> = clean.lines().collect();
        let border = all_rows.iter().rposition(|r| {
            r.trim().chars().count() >= 3 && r.trim().chars().all(|c| matches!(c, '─' | '━' | '═'))
        });
        let rows = &all_rows[border.map_or(0, |i| i + 1)..];
        let tail = rows[rows.len().saturating_sub(16)..]
            .join("\n")
            .to_lowercase();
        let footer = rows
            .iter()
            .rev()
            .find(|r| !r.trim().is_empty())
            .map_or(String::new(), |r| r.trim().to_lowercase());
        if footer.contains("enter to select") && footer.contains("esc to cancel") {
            if [
                "do you want to proceed",
                "trust",
                "select model",
                "permission",
                "bash command",
            ]
            .iter()
            .any(|s| rows.join("\n").to_lowercase().contains(s))
            {
                return View::state(State::MenuOrPicker);
            }
            let mut q = Question {
                text: String::new(),
                options: vec![],
                selected: None,
                total: 1,
                collapsed: false,
                notes: false,
                draft: String::new(),
            };
            for row in rows {
                if let Some((option, selected)) = option_row(row, '❯') {
                    if selected {
                        q.selected = Some(option.number);
                    }
                    q.options.push(option);
                } else if row.trim().ends_with('?') {
                    q.text = row.trim().to_owned();
                }
            }
            if !q.text.is_empty() && !q.options.is_empty() && q.selected.is_some() {
                q.total = rows
                    .join("\n")
                    .chars()
                    .filter(|c| matches!(c, '☐' | '☑'))
                    .count()
                    .max(1);
                return View::question(q);
            }
            return View::state(State::MenuOrPicker);
        }
        if tail.contains("esc to cancel") || tail.contains("enter to confirm") {
            return View::state(State::MenuOrPicker);
        }
        if crate::submission::composer_view("claude", screen).is_some() {
            return View::state(if tail.contains("esc to interrupt") {
                State::Working
            } else {
                State::IdleComposer
            });
        }
        View::state(State::Unknown)
    }
    fn answer_plan(
        &self,
        q: &Question,
        option: Option<usize>,
        text: Option<&str>,
    ) -> Result<AnswerPlan, String> {
        if text.is_some() {
            return Err("this Claude form supports --option only; free-text forms are not recognized safely".to_owned());
        }
        let number = option.ok_or("choose --option")?;
        let keys = navigation(q, number)?;
        if q.options.iter().any(|o| {
            o.number == number
                && ["other", "type something"]
                    .iter()
                    .any(|s| o.text.to_lowercase().starts_with(s))
        }) {
            return Err("this option opens another form; answer it interactively".to_owned());
        }
        Ok(AnswerPlan {
            submit_key: "Enter",
            keys,
            text: None,
            selected: Some(number),
        })
    }
}
