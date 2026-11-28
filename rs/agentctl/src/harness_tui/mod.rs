//! Modular screen classification and answer-key policy, independent of terminal transport.
use serde::Serialize;

mod claude;
mod codex;
pub(crate) mod muse;

/// What input the visible screen currently accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Ordinary idle editor.
    IdleComposer,
    /// Active turn with a follow-up editor.
    Working,
    /// A question, possibly collapsed.
    BlockingQuestion,
    /// A picker, approval, or unsupported modal.
    MenuOrPicker,
    /// No supported layout.
    Unknown,
}

/// One numbered option exactly as displayed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OptionItem {
    /// One-based visible number.
    pub number: usize,
    /// Label and description.
    pub text: String,
}

/// Facts extracted only from the visible form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Question {
    /// Question text.
    pub text: String,
    /// Visible options.
    pub options: Vec<OptionItem>,
    /// Selected option number, when visible.
    pub selected: Option<usize>,
    /// Question count; multiple-question forms are not answered automatically.
    pub total: usize,
    /// Text/options hidden behind the queue summary.
    pub collapsed: bool,
    /// Free-text notes editor is focused.
    pub notes: bool,
    /// Existing answer draft.
    pub draft: String,
}

/// Classification plus any question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct View {
    /// Visible input state.
    pub state: State,
    /// Recognized question.
    pub question: Option<Question>,
}
impl View {
    pub(crate) fn state(state: State) -> Self {
        Self {
            state,
            question: None,
        }
    }
    pub(crate) fn question(question: Question) -> Self {
        Self {
            state: State::BlockingQuestion,
            question: Some(question),
        }
    }
}

/// Effects before the single submission key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnswerPlan {
    /// Navigation keys, revalidated after each redraw.
    pub keys: Vec<&'static str>,
    /// Final submission key; sent once, never guessed by the transport.
    pub submit_key: &'static str,
    /// Optional literal text to paste.
    pub text: Option<String>,
    /// Expected selected option before submission.
    pub selected: Option<usize>,
}

/// Every harness owns its own strings, layouts, and answer-key policy.
pub trait HarnessTui {
    /// Whether a working editor can receive ordinary follow-ups.
    fn working_followups(&self) -> bool;
    /// Submission key for an ordinary follow-up during a working turn.
    fn working_submit_key(&self) -> &'static str {
        "Enter"
    }
    /// Keys to expand a collapsed question.
    fn open_question_keys(&self) -> &'static [&'static str];
    /// Inspect visible rows without effects.
    fn classify(&self, screen: &str) -> View;
    /// Refuse unknown forms instead of guessing where keys will go.
    fn answer_plan(
        &self,
        question: &Question,
        option: Option<usize>,
        text: Option<&str>,
    ) -> Result<AnswerPlan, String>;
}

/// A permission or approval prompt recognised well enough to refuse
/// (#2421 agentctl-asks-never-block): the harness waits for a person to
/// allow or refuse a call or a message, and agentctl may only ever choose
/// the refusal. Built by a harness's own strict recogniser from the
/// visible rows; anything less certain stays unrecognised and is
/// reported, never answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Denial {
    /// Which prompt: `hook-confirmation`, `permission` (Claude Code) or
    /// `command-approval` (Muse).
    pub kind: &'static str,
    /// The prompt as drawn: its heading and first reason line, bounded.
    pub prompt: String,
    /// Every visible option, numbered as drawn.
    pub options: Vec<OptionItem>,
    /// The option the cursor is on, when a cursor is drawn.
    pub selected: Option<usize>,
    /// The label of the row the cursor is drawn on, read from that row
    /// itself: the submit key is sent only when it is a refusal.
    pub selected_text: Option<String>,
    /// The one option that refuses; its label passes [`deny_label`].
    pub deny: usize,
    /// The key that submits the refusal once the cursor is verified on it:
    /// `Escape` where Esc is itself the refusal (every Claude permission
    /// dialog, and Muse's command approval, whose refusal reads `(esc)`), so
    /// a dialog that replaced this one in the instant before the key is
    /// refused too, never approved; `Enter` only where Esc means nothing
    /// known (Muse's peer dialog). If no dialog is up any more, Esc
    /// interrupts a running turn: a cost accepted over any approval.
    pub submit_key: &'static str,
}

/// Longest prompt text a [`Denial`] keeps, in characters.
pub const DENIAL_PROMPT_CHARS: usize = 300;

/// The refusal labels agentctl knows, lowercased, exactly as the harnesses
/// draw them (#2421).
pub const REFUSAL_LABELS: [&str; 7] = [
    "no",
    "no (esc)",
    "no, and tell claude what to do differently",
    "no, and tell claude what to do differently (esc)",
    "reject once",
    "abort the entire command",
    "abort the entire command (esc)",
];

/// Whether an option label is one of the known refusals
/// ([`REFUSAL_LABELS`]), compared whole, case aside. Anything else, an
/// approving label (`Yes`, `Allow ...`, `Trust ...`) or an unknown one that
/// merely starts with `No` (`No thanks, run it`), never passes; nor does
/// `Block`, which refuses a peer for good rather than once.
#[must_use]
pub fn deny_label(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    REFUSAL_LABELS.contains(&lower.as_str())
}

/// The option of `options` that refuses, when exactly one does and some
/// other option is there to refuse instead of.
pub(crate) fn sole_deny_option(options: &[OptionItem]) -> Option<usize> {
    let mut refusing = options.iter().filter(|option| deny_label(&option.text));
    let deny = refusing.next()?;
    let refusing_more = refusing.next().is_some();
    // "No" and "No, and tell Claude what to do differently" both refuse;
    // the plain one is chosen. Any other pair is ambiguous.
    if refusing_more {
        let plain: Vec<_> = options
            .iter()
            .filter(|option| option.text.trim().eq_ignore_ascii_case("no"))
            .collect();
        return (plain.len() == 1).then(|| plain[0].number);
    }
    (options.len() >= 2).then_some(deny.number)
}

/// The permission or approval prompt `screen` shows for `harness`, when
/// one is recognised well enough to refuse (#2421): Claude Code's hook,
/// safety-check and permission-rule confirmations, and Muse's command
/// approval. Muse's peer-approval dialog, Codex (approvals off, no recorded
/// approval screen) and an unknown harness have none: reported only.
#[must_use]
pub fn permission_denial(harness: Option<&str>, screen: &str) -> Option<Denial> {
    let denial = match harness {
        Some("claude") => claude::permission_denial(screen),
        Some("muse") => muse::permission_denial(screen),
        _ => None,
    }?;
    // Defence in depth: whatever a recogniser built, the options must be
    // numbered exactly 1..n, the option chosen must be the only refusal and
    // read as one, and a drawn cursor must carry the label of its own row.
    let numbered = denial
        .options
        .iter()
        .enumerate()
        .all(|(position, option)| option.number == position + 1);
    let chosen = denial
        .options
        .iter()
        .find(|option| option.number == denial.deny)?;
    let cursor_consistent = match (denial.selected, denial.selected_text.as_deref()) {
        (Some(selected), Some(text)) => denial
            .options
            .get(selected.wrapping_sub(1))
            .is_some_and(|option| option.text == text),
        _ => false,
    };
    (numbered
        && cursor_consistent
        && matches!(denial.submit_key, "Enter" | "Escape")
        && deny_label(&chosen.text)
        && sole_deny_option(&denial.options) == Some(denial.deny))
    .then_some(denial)
}

/// Supported adapters; unknown harnesses remain unsupported.
pub fn harness_tui(harness: Option<&str>) -> Option<&'static dyn HarnessTui> {
    match harness {
        Some("codex") => Some(&codex::CodexTui),
        Some("claude") => Some(&claude::ClaudeTui),
        _ => None,
    }
}

/// Adapters a queue drain may use to type an ordinary follow-up into a working pane: those of
/// [`harness_tui`], plus Muse, whose forms agentctl does not answer.
pub fn followup_tui(harness: Option<&str>) -> Option<&'static dyn HarnessTui> {
    match harness {
        Some("muse") => Some(&muse::MuseTui),
        _ => harness_tui(harness),
    }
}

pub(crate) fn plain(screen: &str) -> String {
    crate::submission::render_screen(screen)
        .iter()
        .map(|(row, _)| row.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn option_row(row: &str, cursor: char) -> Option<(OptionItem, bool)> {
    let trimmed = row.trim();
    let selected = trimmed.starts_with(cursor);
    let body = trimmed.trim_start_matches(cursor).trim_start();
    let (n, label) = body.split_once(". ")?;
    if !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((
        OptionItem {
            number: n.parse().ok()?,
            text: label.trim().to_owned(),
        },
        selected,
    ))
}

pub(crate) fn navigation(q: &Question, number: usize) -> Result<Vec<&'static str>, String> {
    if q.total != 1 || q.collapsed || q.notes || !q.draft.is_empty() {
        return Err(
            "only one fully visible, unanswered question with no existing draft is supported"
                .to_owned(),
        );
    }
    let numbers: Vec<_> = q.options.iter().map(|o| o.number).collect();
    let selected = q.selected.filter(|s| numbers.contains(s));
    if numbers != (1..=numbers.len()).collect::<Vec<_>>()
        || !numbers.contains(&number)
        || selected.is_none()
    {
        return Err(
            "option is out of range, hidden, or the current selection is not visible".to_owned(),
        );
    }
    let selected = selected.expect("checked selection");
    Ok(vec![
        if number > selected { "Down" } else { "Up" };
        number.abs_diff(selected)
    ])
}

/// A pre-injection refusal with the actual visible question and recovery command.
pub fn blocked_hint(view: &View) -> String {
    let text = view
        .question
        .as_ref()
        .map_or("Question text is not recognizable on this screen.", |q| {
            q.text.as_str()
        });
    format!("{text} Use agentctl prompts NAME to inspect; agentctl answer NAME --option N or --text TEXT to answer a supported form.")
}
