//! Codex default-keymap question forms.
use super::{navigation, option_row, plain, AnswerPlan, HarnessTui, Question, State, View};

pub struct CodexTui;
impl HarnessTui for CodexTui {
    fn working_followups(&self) -> bool {
        true
    }
    fn open_question_keys(&self) -> &'static [&'static str] {
        &["shift+left"]
    }
    fn classify(&self, screen: &str) -> View {
        let clean = plain(screen);
        let rows: Vec<_> = clean.lines().collect();
        let tail = rows[rows.len().saturating_sub(16)..]
            .join("\n")
            .to_lowercase();
        let footer = rows
            .iter()
            .rev()
            .find(|r| !r.trim().is_empty())
            .map_or(String::new(), |r| r.trim().to_lowercase());
        if footer == "enter select · esc back" {
            if let Some(first) = rows
                .iter()
                .rposition(|row| row.trim() == "Approaching rate limits")
            {
                let last = rows.iter().rposition(|row| !row.trim().is_empty()).unwrap();
                let body: Vec<_> = rows[first + 1..last]
                    .iter()
                    .map(|row| row.trim())
                    .filter(|row| !row.is_empty())
                    .collect();
                if body.len() == 4 {
                    let model = body[0]
                        .strip_prefix("Switch to ")
                        .and_then(|row| row.strip_suffix(" for lower credit usage?"));
                    let parsed: Option<Vec<_>> =
                        body[1..].iter().map(|row| option_row(row, '›')).collect();
                    if let (Some(model), Some(parsed)) = (model, parsed) {
                        let valid_model = model
                            .as_bytes()
                            .first()
                            .is_some_and(u8::is_ascii_alphanumeric)
                            && model
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c));
                        let selected: Vec<_> = parsed
                            .iter()
                            .filter(|(_, selected)| *selected)
                            .map(|(item, _)| item.number)
                            .collect();
                        if valid_model
                            && parsed.iter().map(|(item, _)| item.number).eq([1, 2, 3])
                            && selected.len() == 1
                            && parsed[0].0.text.starts_with(&format!("Switch to {model} "))
                            && parsed[1].0.text == "Keep current model"
                            && parsed[2]
                                .0
                                .text
                                .starts_with("Keep current model (never show again) ")
                        {
                            return View::question(Question {
                                text: format!("Approaching rate limits\n{}", body[0]),
                                options: parsed.into_iter().map(|(item, _)| item).collect(),
                                selected: Some(selected[0]),
                                total: 1,
                                collapsed: false,
                                notes: false,
                                draft: String::new(),
                            });
                        }
                    }
                }
            }
            return View::state(State::MenuOrPicker);
        }
        if footer.starts_with("enter submit")
            && footer.contains("^] skip")
            && (footer.contains("shift+→ main prompt")
                || footer.contains("shift+right main prompt"))
        {
            // Async questions have a dedicated editor but no numbered Question header.
            if let Some(first) = rows.iter().rposition(|row| {
                matches!(
                    row.trim(),
                    "• Queued follow-up inputs" | "Queued follow-up inputs"
                )
            }) {
                let last = rows.iter().rposition(|row| !row.trim().is_empty()).unwrap();
                let mut parts: Vec<Vec<&str>> = Vec::new();
                let mut part = Vec::new();
                for row in &rows[first + 1..last] {
                    if row.trim().is_empty() {
                        if !part.is_empty() {
                            parts.push(std::mem::take(&mut part));
                        }
                    } else {
                        part.push(row.trim());
                    }
                }
                if !part.is_empty() {
                    parts.push(part);
                }
                if parts.len() >= 2 {
                    let editor = parts.pop().unwrap();
                    let title = parts
                        .iter()
                        .map(|part| part.join("\n"))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    if !title.is_empty()
                        && !title
                            .lines()
                            .any(|row| row.trim().starts_with("Question ") && row.contains('/'))
                    {
                        let parsed: Option<Vec<_>> =
                            editor.iter().map(|row| option_row(row, '›')).collect();
                        let (options, selected, draft) = if let Some(parsed) = parsed {
                            let selected: Vec<_> = parsed
                                .iter()
                                .filter(|(_, selected)| *selected)
                                .map(|(option, _)| option.number)
                                .collect();
                            if selected.len() != 1
                                || !parsed
                                    .iter()
                                    .enumerate()
                                    .all(|(index, (option, _))| option.number == index + 1)
                            {
                                return View::state(State::MenuOrPicker);
                            }
                            (
                                parsed.into_iter().map(|(option, _)| option).collect(),
                                Some(selected[0]),
                                String::new(),
                            )
                        } else if editor.iter().all(|row| option_row(row, '›').is_none()) {
                            let editor_text = editor.join(" ");
                            let editor = editor_text.trim_start_matches('›').trim();
                            (
                                vec![],
                                None,
                                if editor == "Type your answer" {
                                    String::new()
                                } else {
                                    editor.to_owned()
                                },
                            )
                        } else {
                            return View::state(State::MenuOrPicker);
                        };
                        return View::question(Question {
                            text: title,
                            options,
                            selected,
                            total: 1,
                            collapsed: false,
                            notes: false,
                            draft,
                        });
                    }
                }
            }
            return View::state(State::MenuOrPicker);
        }
        let header = rows.iter().enumerate().rev().find_map(|(i, row)| {
            let h = row.trim().strip_prefix("Question ")?;
            let (index, rest) = h.split_once('/')?;
            let (total, suffix) = rest.split_once(" (")?;
            if !suffix.ends_with(" unanswered)") || index.parse::<usize>().is_err() {
                return None;
            }
            Some((i, total.parse::<usize>().ok()?))
        });
        if let Some((i, total)) = header.filter(|_| {
            footer.contains("enter to submit answer") || footer.contains("enter to submit all")
        }) {
            let mut q = Question {
                text: String::new(),
                options: vec![],
                selected: None,
                total,
                collapsed: false,
                notes: tail.contains("tab or esc to clear notes"),
                draft: String::new(),
            };
            let mut title = vec![];
            let mut editor = false;
            for row in &rows[i + 1..] {
                let line = row.trim();
                if line.contains("enter to submit") {
                    break;
                }
                if let Some((option, selected)) = option_row(row, '›') {
                    if selected {
                        q.selected = Some(option.number);
                    }
                    q.options.push(option);
                } else if let Some(content) = line.strip_prefix('›') {
                    editor = true;
                    let content = content.trim();
                    if !matches!(content, "Type your answer (optional)" | "Add notes") {
                        q.draft = content.to_owned();
                    }
                } else if !line.is_empty() {
                    if let Some(last) = q.options.last_mut() {
                        last.text.push(' ');
                        last.text.push_str(line);
                    } else {
                        title.push(line);
                    }
                }
            }
            q.text = title.join("\n");
            if q.text.is_empty() || (q.options.is_empty() && !editor) {
                return View::state(State::MenuOrPicker);
            }
            return View::question(q);
        }
        if tail.contains("shift+← to answer") || tail.contains("shift+left to answer") {
            return View::question(Question {
                text: "Question collapsed; text and options are not visible.".to_owned(),
                options: vec![],
                selected: None,
                total: 1,
                collapsed: true,
                notes: false,
                draft: String::new(),
            });
        }
        let composer = crate::submission::composer_view("codex", screen);
        let modal_region = if composer.is_some() { &footer } else { &tail };
        if [
            "esc to cancel",
            "enter to confirm",
            "trust this folder",
            "allow command?",
            "enter to submit",
            "select model",
        ]
        .iter()
        .any(|s| modal_region.contains(s))
        {
            return View::state(State::MenuOrPicker);
        }
        if composer.is_some()
            && (tail.contains("esc to interrupt") || tail.contains("tab to queue message"))
        {
            return View::state(State::Working);
        }
        if crate::submission::codex_idle_composer(screen) {
            return View::state(State::IdleComposer);
        }
        View::state(State::Unknown)
    }
    fn answer_plan(
        &self,
        q: &Question,
        option: Option<usize>,
        text: Option<&str>,
    ) -> Result<AnswerPlan, String> {
        if let Some(number) = option {
            let keys = navigation(q, number)?;
            if q.options
                .iter()
                .any(|o| o.number == number && o.text.starts_with("None of the above"))
            {
                return Err("the free-text option needs --text, not --option".to_owned());
            }
            return Ok(AnswerPlan {
                submit_key: "Enter",
                keys,
                text: None,
                selected: Some(number),
            });
        }
        let text = text
            .filter(|t| !t.trim().is_empty() && !t.chars().any(char::is_control))
            .ok_or("--text must be nonempty single-line text without terminal controls")?;
        if q.options.is_empty() {
            if q.total != 1 || q.collapsed || q.notes || (!q.draft.is_empty() && q.draft != text) {
                return Err(
                    "free-text question is not empty or has an unsupported layout".to_owned(),
                );
            }
            return Ok(AnswerPlan {
                submit_key: "Enter",
                keys: vec![],
                text: q.draft.is_empty().then(|| text.to_owned()),
                selected: None,
            });
        }
        let other = q
            .options
            .iter()
            .find(|o| o.text.starts_with("None of the above"))
            .ok_or("this question has no visible free-text option; choose --option")?
            .number;
        let mut keys = navigation(q, other)?;
        keys.push("Tab");
        Ok(AnswerPlan {
            submit_key: "Enter",
            keys,
            text: Some(text.to_owned()),
            selected: Some(other),
        })
    }
}
