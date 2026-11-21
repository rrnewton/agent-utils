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
        // A Claude hook confirmation (`Hook PreToolUse:Bash requires
        // confirmation … Do you want to proceed? 1. Yes 2. No`) is an
        // owner-only decision (#1784): recognize it as a blocking question
        // naming the hook and the first line of its reason, so the watcher
        // can page a person instead of reporting unrecognizable text.
        // `answer_plan` refuses it: nothing answers it automatically.
        {
            let rows: Vec<_> = clean.lines().collect();
            let hook = rows.iter().position(|row| {
                let lower = row.to_lowercase();
                lower.contains("hook") && lower.contains("requires confirmation")
            });
            if let Some(hook) = hook {
                let mut options = vec![];
                let mut selected = None;
                for row in &rows {
                    if let Some((option, is_selected)) = option_row(row, '❯') {
                        if is_selected {
                            selected = Some(option.number);
                        }
                        options.push(option);
                    }
                }
                let proceeds = rows
                    .iter()
                    .any(|row| row.to_lowercase().contains("do you want to proceed"));
                if proceeds && options.len() >= 2 && selected.is_some() {
                    let reason = rows[hook + 1..]
                        .iter()
                        .map(|row| row.trim())
                        .find(|row| {
                            !row.is_empty()
                                && !row.to_lowercase().contains("do you want to proceed")
                                && option_row(row, '❯').is_none()
                        })
                        .unwrap_or_default();
                    let heading = rows[hook].trim();
                    let text = if reason.is_empty() {
                        heading.to_owned()
                    } else {
                        format!("{heading}: {reason}")
                    };
                    return View::question(Question {
                        text,
                        options,
                        selected,
                        total: 1,
                        collapsed: false,
                        notes: false,
                        draft: String::new(),
                    });
                }
            }
        }
        // A permission prompt (`Do you want to proceed? ❯ 1. Yes  2. No`,
        // under a `Bash command` form or a dangerous-operation warning) is
        // an answerable question, not a menu: until this recogniser it
        // classified as a menu, so `agentctl answer` refused it while
        // `agentctl prompts` showed it (#2011
        // agentctl-claude-permission-answer). Only the exact form counts:
        // the proceed line, then numbered Yes/No options with one selected.
        {
            let rows: Vec<_> = clean.lines().collect();
            if let Some(proceed) = rows
                .iter()
                .position(|row| row.to_lowercase().contains("do you want to proceed"))
            {
                let mut options = vec![];
                let mut selected = None;
                for row in &rows[proceed + 1..] {
                    if let Some((option, is_selected)) = option_row(row, '❯') {
                        if is_selected {
                            selected = Some(option.number);
                        }
                        options.push(option);
                    }
                }
                let yes_no = options
                    .iter()
                    .any(|option| option.text.to_lowercase().starts_with("yes"))
                    && options
                        .iter()
                        .any(|option| option.text.to_lowercase().starts_with("no"));
                // A permission context must head the form: a bare proceed
                // selection (the shared corpus's menu row) stays a menu.
                let context = rows[..proceed].iter().rev().find(|row| {
                    let lower = row.to_lowercase();
                    lower.contains("dangerous")
                        || lower.contains("requires confirmation")
                        || lower.contains("bash command")
                });
                if options.len() >= 2 && selected.is_some() && yes_no && context.is_some() {
                    let text = context
                        .expect("permission context checked")
                        .trim()
                        .to_owned();
                    return View::question(Question {
                        text,
                        options,
                        selected,
                        total: 1,
                        collapsed: false,
                        notes: false,
                        draft: String::new(),
                    });
                }
            }
        }
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
        // A hook confirmation is owner-only (#1784): never answered through
        // agentctl, automatically or otherwise; a person answers at the pane.
        if q.text.to_lowercase().contains("hook")
            && q.text.to_lowercase().contains("requires confirmation")
        {
            return Err(
                "this is an owner-only hook confirmation; answer it at the pane in person — agentctl never answers it"
                    .to_owned(),
            );
        }
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
