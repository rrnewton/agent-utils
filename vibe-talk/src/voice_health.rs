//! "The voice service is not responding", as one content-free log line.
//!
//! `voice-unresponsive-signal`. A live call once stayed green for its whole length while every
//! turn produced no speech and no transcript: the socket was open, turns completed, and each one
//! carried PCM that nobody could hear. Nothing on the page and nothing in this log said so. The
//! page now judges each turn from the frames `vibe-talk-v1` already carries and reports what it
//! concluded here, so a log reader can see the episode without anyone having been on the call.
//!
//! **Content-free by construction.** [`HealthReport`] is a closed set of fields: two closed enums,
//! a flag, and integers. An unknown field, an unknown [`Cause`], or an unknown protocol is refused
//! rather than echoed, so the one line it writes has no free-text field at all — not the words
//! said, not an error message, not an id, not a provider.
//!
//! **Bounded.** The page reports each cause at most once per call, and a recovery only after an
//! unresponsive report it attempted, so a well-behaved call writes at most eight lines: five
//! causes and three recoveries. The server does not trust that: [`LogBudget`] caps the lines this
//! route writes per minute across every caller, and [`MAX_BODY_BYTES`] caps what it will read.
//! The first refusal in a window writes one line saying records are being dropped, so throttling
//! is visible without becoming a flood of its own.
//!
//! **How a call ended** (`#268 voice-call-drops`) is the second record here, [`EndReport`]: one
//! line per call that opened, naming who or what ended it, on the same terms — closed, bounded by
//! its own [`LogBudget`], and free of anything said in the call. `#330 voice-long-turn-drop` gave it
//! one field of words: the voice SERVICE's own close reason or error, capped at
//! [`MAX_CLOSE_REASON_CHARS`] with control characters refused, and quoted in the line — never the
//! owner's speech or the agent's.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The largest request body either route reads. A health report is under 200 bytes; a call-end
/// record with every field at its largest and its longest close reason, four bytes a character, is
/// under 800.
pub const MAX_BODY_BYTES: usize = 1_024;

/// The longest close reason a call-end record carries, in characters: a WebSocket close frame's
/// reason is at most 123 bytes, and an `error` frame's words are cut to the same order.
pub const MAX_CLOSE_REASON_CHARS: usize = 123;

/// The most lines this route writes per [`BUDGET_WINDOW`], across every caller. Well above what
/// honest pages produce — eight lines per call at most — and far below a flood.
pub const MAX_LINES_PER_WINDOW: u32 = 30;

/// The window [`MAX_LINES_PER_WINDOW`] is counted over.
pub const BUDGET_WINDOW: Duration = Duration::from_secs(60);

/// The longest call offset accepted: a day. Past that the number is not a measurement.
pub const MAX_SINCE_OPEN_MS: u32 = 86_400_000;

/// The most audio one turn is believed to carry: an hour.
pub const MAX_AUDIO_MS: u32 = 3_600_000;

/// The largest magnitude a 16-bit sample can have (`|-32768|`).
pub const MAX_PEAK: u32 = 32_768;

/// The most reconnects a call-end record may say came before it in one call. The page carries a call
/// on a new session at most three times (`#352 voice-reconnect-on-bridge-error`); past a thousand the
/// number is not a count of anything a call did.
pub const MAX_RECONNECTS: u32 = 1_000;
/// The most page messages one call-end record may say were held for the owner's speech. `#512
/// voice-recap-private-stack`.
pub const MAX_SPEECH_HOLDS: u32 = 10_000;

/// Wire protocols this signal is defined for. Only the provider-neutral one: a hosted vendor's
/// protocol has error frames of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Protocol {
    /// The deployment-managed WebSocket protocol.
    #[serde(rename = "vibe-talk-v1")]
    VibeTalkV1,
}

impl Protocol {
    const fn as_str(self) -> &'static str {
        match self {
            Self::VibeTalkV1 => "vibe-talk-v1",
        }
    }
}

/// What the page concluded. See the README, "When the voice service stops answering".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    /// The greeting turn completed with no audible audio and no assistant text.
    SilentGreeting,
    /// Consecutive turns completed with no audible audio and no assistant text.
    SilentTurns,
    /// A turn the page started got nothing at all within its bound.
    NoReply,
    /// The service sent an `error` frame.
    ErrorFrame,
    /// Audible audio or assistant text arrived after an unresponsive report.
    Recovered,
    /// A turn completed having carried audible audio and no assistant text: the service answered,
    /// and its words never arrived. Not an unresponsive state. `#330 voice-long-turn-drop`.
    AudioWithoutText,
}

impl Cause {
    const fn as_str(self) -> &'static str {
        match self {
            Self::SilentGreeting => "silent_greeting",
            Self::SilentTurns => "silent_turns",
            Self::NoReply => "no_reply",
            Self::ErrorFrame => "error_frame",
            Self::Recovered => "recovered",
            Self::AudioWithoutText => "audio_without_text",
        }
    }
}

/// One page's report. The field set IS the allowlist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HealthReport {
    /// Wire protocol of the call.
    pub protocol: Protocol,
    /// Whether the call was typed, in which case only text counts as a reply.
    pub chat: bool,
    /// What the page concluded.
    pub cause: Cause,
    /// Milliseconds since the socket opened.
    pub since_open_ms: u32,
    /// Completed turns in the call so far: an ordinal, not an id.
    pub turns: u32,
    /// Consecutive silent turns ending at the most recent one.
    pub silent_turns: u32,
    /// Milliseconds of PCM the last silent turn carried; 0 when it carried none.
    pub audio_ms: u32,
    /// The largest sample magnitude in the last silent turn's PCM.
    pub peak: u32,
}

impl HealthReport {
    /// Refuse anything that is not a plausible report.
    ///
    /// # Errors
    ///
    /// Returns a sentence naming the offending field.
    pub fn validate(&self) -> Result<(), String> {
        if self.since_open_ms > MAX_SINCE_OPEN_MS {
            return Err(format!(
                "since_open_ms is longer than {MAX_SINCE_OPEN_MS} ms"
            ));
        }
        if self.audio_ms > MAX_AUDIO_MS {
            return Err(format!("audio_ms is longer than {MAX_AUDIO_MS} ms"));
        }
        if self.peak > MAX_PEAK {
            return Err(format!("peak is larger than {MAX_PEAK}"));
        }
        if self.silent_turns > self.turns {
            return Err("silent_turns is more than turns".to_owned());
        }
        Ok(())
    }

    /// Whether this report ends an episode rather than starting one, and is logged at info.
    #[must_use]
    pub fn is_recovery(&self) -> bool {
        self.cause == Cause::Recovered
    }

    /// Whether this report is about the transcript rather than the service answering at all: a
    /// turn that was heard, only its words did not arrive. `#330 voice-long-turn-drop`.
    #[must_use]
    pub fn is_missing_text(&self) -> bool {
        self.cause == Cause::AudioWithoutText
    }

    /// The single log line: enum names and integers, and nothing anybody said.
    #[must_use]
    pub fn log_fields(&self) -> String {
        format!(
            "protocol={} chat={} cause={} since_open_ms={} turns={} silent_turns={} audio_ms={} peak={}",
            self.protocol.as_str(),
            self.chat,
            self.cause.as_str(),
            self.since_open_ms,
            self.turns,
            self.silent_turns,
            self.audio_ms,
            self.peak,
        )
    }
}

/// Wire protocols a call-end record is defined for: every one the page speaks. `#330
/// voice-long-turn-drop`: a call on the hosted vendor's protocol that ended mid-answer left no line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum CallProtocol {
    /// The deployment-managed WebSocket protocol.
    #[serde(rename = "vibe-talk-v1")]
    VibeTalkV1,
    /// The hosted conversational vendor's own protocol.
    #[serde(rename = "elevenlabs")]
    Hosted,
}

impl CallProtocol {
    const fn as_str(self) -> &'static str {
        match self {
            Self::VibeTalkV1 => "vibe-talk-v1",
            Self::Hosted => "elevenlabs",
        }
    }
}

/// Whether the page was in sight when its call ended. `#330 voice-long-turn-drop`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// On screen.
    Visible,
    /// In the background: another app in front, the phone locked.
    Hidden,
}

impl Visibility {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Hidden => "hidden",
        }
    }
}

/// Who or what ended a call that had opened, as the page saw it. `#268 voice-call-drops`. See the
/// README, "How a call ended".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndedBy {
    /// The reader pressed Hang up.
    HangUp,
    /// The voice service sent an `error` frame, which ends its session, and the page then said
    /// goodbye (`audio_end`, `quit`) and closed.
    ErrorFrame,
    /// The voice service's side closed the socket with a closing handshake while the reader was in
    /// the call and the page was in sight.
    ServerClosed,
    /// The connection was lost without a closing handshake (the browser's 1006, or an error on the
    /// socket before its close): the network, the phone, or the far side cut it.
    ConnectionLost,
    /// The socket closed while the page was in the background, or just after it came back.
    Suspended,
    /// `#352 voice-reconnect-on-bridge-error`. This socket's session was lost: the voice service's
    /// `error` asked the client to reconnect, and the page said goodbye to it and carried the call on
    /// a new session. One per socket replaced; the call itself goes on, and its own end is recorded
    /// later.
    Reconnected,
    /// `#352`. The voice service asked to reconnect again within a minute of the page's last
    /// reconnect, or after the most the page makes in one call, so the page ended the call instead.
    ReconnectLimit,
    /// `#352`. A reconnect could not get a working session — the server refused one, its socket did
    /// not open in time, or it closed or erred before its session started — so the page ended the
    /// call.
    ReconnectFailed,
}

impl EndedBy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::HangUp => "hang_up",
            Self::ErrorFrame => "error_frame",
            Self::ServerClosed => "server_closed",
            Self::ConnectionLost => "connection_lost",
            Self::Suspended => "suspended",
            Self::Reconnected => "reconnected",
            Self::ReconnectLimit => "reconnect_limit",
            Self::ReconnectFailed => "reconnect_failed",
        }
    }
}

/// The kind of audio device a call's capture or playback used, as the page classified it from
/// the labels the browser reported. `#619 voice-bluetooth-output`. Kinds only, never a label:
/// a device name is the owner's, and this record is content-free by construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioRoute {
    /// A Bluetooth headset (HFP/SCO, BLE or A2DP as the browser labelled it).
    Bluetooth,
    /// A wired headset or headphones.
    WiredHeadset,
    /// The phone's loudspeaker.
    Speakerphone,
    /// The phone's earpiece.
    Earpiece,
    /// A USB audio device.
    Usb,
    /// A device the page could not classify further.
    Other,
}

impl AudioRoute {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Bluetooth => "bluetooth",
            Self::WiredHeadset => "wired_headset",
            Self::Speakerphone => "speakerphone",
            Self::Earpiece => "earpiece",
            Self::Usb => "usb",
            Self::Other => "other",
        }
    }
}

/// One page's record of how a call that opened ended: `POST /api/v1/voice-end`. The field set IS
/// the allowlist, as for [`HealthReport`]: closed enums, a flag and integers, and one capped line of
/// the voice service's own words — nothing said in the call.
///
/// `#268 voice-call-drops`. Calls ended on their own, and the only record of why was the voice
/// bridge's log, which saw the page say goodbye and could not see what made it. One line per call
/// here tells a hang-up from a drop from the service ending the call.
///
/// `#330 voice-long-turn-drop`. A long answer was cut and the call ended, and this line could not say
/// why: it now carries the service's reason, and what decides between the candidates — whether the
/// page had just sent a text turn, an `audio_end` or an `interrupt` (a bridge may take the last two
/// for a cut), whether it was in sight, and how much of the answer was still waiting to be played.
/// The new fields are optional, so a page that predates them still records its calls.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EndReport {
    /// Wire protocol of the call.
    pub protocol: CallProtocol,
    /// Whether the call was typed.
    pub chat: bool,
    /// Who or what ended it.
    pub ended_by: EndedBy,
    /// The close code the browser reported, `1000`–`4999`; absent when the page closed the socket
    /// itself.
    #[serde(default)]
    pub close_code: Option<u16>,
    /// Milliseconds from the socket opening to the end.
    pub since_open_ms: u32,
    /// Completed turns in the call: an ordinal, not an id.
    pub turns: u32,
    /// Whether the page was in sight at the end.
    #[serde(default)]
    pub visibility: Option<Visibility>,
    /// Milliseconds of the assistant's audio still queued to play at the end.
    #[serde(default)]
    pub queued_audio_ms: Option<u32>,
    /// Milliseconds before the end that the page last sent a text turn; absent when it sent none.
    #[serde(default)]
    pub last_text_ms: Option<u32>,
    /// Milliseconds before the end that the page last sent `audio_end`; absent when it sent none.
    #[serde(default)]
    pub last_audio_end_ms: Option<u32>,
    /// Milliseconds before the end that the page last sent `interrupt`; absent when it sent none.
    #[serde(default)]
    pub last_interrupt_ms: Option<u32>,
    /// `#352 voice-reconnect-on-bridge-error`. How many times the page carried this call on a new
    /// session before this record; absent when it never did.
    #[serde(default)]
    pub reconnects: Option<u32>,
    /// `#512 voice-recap-private-stack`. How many of the page's own messages — a relayed chat
    /// message, its word on a send card — waited for the owner's speech, judged by the microphone's
    /// level or his transcript growing, before closing the segment to be said; absent when none did.
    #[serde(default)]
    pub speech_holds: Option<u32>,
    /// ...the longest of those waits, in milliseconds...
    #[serde(default)]
    pub speech_hold_max_ms: Option<u32>,
    /// ...and how many of them went at the cap, with the microphone still loud.
    #[serde(default)]
    pub speech_hold_capped: Option<u32>,
    /// `#619 voice-bluetooth-output`. The kind of device the call captured from, as the page
    /// classified the browser's reported label; absent on a page that predates it, and on a
    /// typed call, which captured nothing.
    #[serde(default)]
    pub audio_input: Option<AudioRoute>,
    /// `#619 voice-bluetooth-output`. The kind of device the call played to, on the same terms.
    #[serde(default)]
    pub audio_output: Option<AudioRoute>,
    /// The voice service's own words: its close reason, or its `error`. At most
    /// [`MAX_CLOSE_REASON_CHARS`], with no control characters.
    #[serde(default)]
    pub close_reason: Option<String>,
}

impl EndReport {
    /// Refuse anything that is not a plausible record.
    ///
    /// # Errors
    ///
    /// Returns a sentence naming the offending field.
    pub fn validate(&self) -> Result<(), String> {
        if self.since_open_ms > MAX_SINCE_OPEN_MS {
            return Err(format!(
                "since_open_ms is longer than {MAX_SINCE_OPEN_MS} ms"
            ));
        }
        if self
            .close_code
            .is_some_and(|code| !(1000..=4999).contains(&code))
        {
            return Err("close_code is not a WebSocket close code (1000-4999)".to_owned());
        }
        if self.queued_audio_ms.is_some_and(|ms| ms > MAX_AUDIO_MS) {
            return Err(format!("queued_audio_ms is longer than {MAX_AUDIO_MS} ms"));
        }
        for (name, ago) in [
            ("last_text_ms", self.last_text_ms),
            ("last_audio_end_ms", self.last_audio_end_ms),
            ("last_interrupt_ms", self.last_interrupt_ms),
        ] {
            if ago.is_some_and(|ms| ms > MAX_SINCE_OPEN_MS) {
                return Err(format!("{name} is longer than {MAX_SINCE_OPEN_MS} ms"));
            }
        }
        if self.reconnects.is_some_and(|count| count > MAX_RECONNECTS) {
            return Err(format!("reconnects is more than {MAX_RECONNECTS}"));
        }
        for (name, count) in [
            ("speech_holds", self.speech_holds),
            ("speech_hold_capped", self.speech_hold_capped),
        ] {
            if count.is_some_and(|count| count > MAX_SPEECH_HOLDS) {
                return Err(format!("{name} is more than {MAX_SPEECH_HOLDS}"));
            }
        }
        if self
            .speech_hold_max_ms
            .is_some_and(|ms| ms > MAX_SINCE_OPEN_MS)
        {
            return Err(format!(
                "speech_hold_max_ms is longer than {MAX_SINCE_OPEN_MS} ms"
            ));
        }
        if let Some(reason) = &self.close_reason {
            if reason.chars().count() > MAX_CLOSE_REASON_CHARS {
                return Err(format!(
                    "close_reason is longer than {MAX_CLOSE_REASON_CHARS} characters"
                ));
            }
            if reason.chars().any(char::is_control) {
                return Err("close_reason carries a control character".to_owned());
            }
        }
        Ok(())
    }

    /// Whether the reader ended the call, rather than anything ending it for them.
    #[must_use]
    pub fn by_the_reader(&self) -> bool {
        self.ended_by == EndedBy::HangUp
    }

    /// The single log line: enum names and integers, `-` for what was not sent, and the service's
    /// reason last and quoted, its quotes and backslashes escaped, so it cannot pass for a field.
    #[must_use]
    pub fn log_fields(&self) -> String {
        let number = |value: Option<u32>| value.map_or_else(|| "-".to_owned(), |n| n.to_string());
        // `#512 voice-recap-private-stack`. Only on a call that held anything for the owner's
        // speech, so every other call's line reads as it did.
        let holds = self.speech_holds.map_or_else(String::new, |count| {
            format!(
                " speech_holds={count} speech_hold_max_ms={} speech_hold_capped={}",
                number(self.speech_hold_max_ms),
                number(self.speech_hold_capped)
            )
        });
        // `#619 voice-bluetooth-output`. Only on a call that captured, so every other call's
        // line reads as it did.
        let route = match (self.audio_input, self.audio_output) {
            (None, None) => String::new(),
            (input, output) => format!(
                " audio_input={} audio_output={}",
                input.map_or("-", AudioRoute::as_str),
                output.map_or("-", AudioRoute::as_str)
            ),
        };
        format!(
            "protocol={} chat={} ended_by={} close_code={} since_open_ms={} turns={} visibility={} \
             queued_audio_ms={} last_text_ms={} last_audio_end_ms={} last_interrupt_ms={} \
             reconnects={}{holds}{route} close_reason={}",
            self.protocol.as_str(),
            self.chat,
            self.ended_by.as_str(),
            self.close_code
                .map_or_else(|| "-".to_owned(), |code| code.to_string()),
            self.since_open_ms,
            self.turns,
            self.visibility.map_or("-", Visibility::as_str),
            number(self.queued_audio_ms),
            number(self.last_text_ms),
            number(self.last_audio_end_ms),
            number(self.last_interrupt_ms),
            number(self.reconnects),
            self.close_reason
                .as_deref()
                .map_or_else(|| "-".to_owned(), |reason| format!("{reason:?}")),
        )
    }
}

/// What [`LogBudget::spend`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spend {
    /// Write the record.
    Allowed,
    /// Refuse it, and say once that this window is dropping records.
    FirstRefusal,
    /// Refuse it quietly: this window has already said so.
    Refused,
}

impl Spend {
    /// Whether the record may be logged.
    #[must_use]
    pub const fn allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// A fixed-window count of the lines this route has written.
#[derive(Debug)]
pub struct LogBudget {
    window: Mutex<(Instant, u32)>,
}

impl Default for LogBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl LogBudget {
    /// A budget whose first window starts now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            window: Mutex::new((Instant::now(), 0)),
        }
    }

    /// Spend one line if the current window has one left.
    pub fn spend(&self) -> Spend {
        self.spend_at(Instant::now())
    }

    fn spend_at(&self, now: Instant) -> Spend {
        let mut window = self
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if now.duration_since(window.0) >= BUDGET_WINDOW {
            *window = (now, 0);
        }
        // Counting on past the cap (saturating) is how the first refusal is told from the rest.
        window.1 = window.1.saturating_add(1);
        if window.1 <= MAX_LINES_PER_WINDOW {
            Spend::Allowed
        } else if window.1 == MAX_LINES_PER_WINDOW + 1 {
            Spend::FirstRefusal
        } else {
            Spend::Refused
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(cause: Cause) -> HealthReport {
        HealthReport {
            protocol: Protocol::VibeTalkV1,
            chat: false,
            cause,
            since_open_ms: 21_400,
            turns: 3,
            silent_turns: 2,
            audio_ms: 1_000,
            peak: 0,
        }
    }

    #[test]
    fn the_log_line_is_enum_names_and_integers_only() {
        let line = report(Cause::SilentTurns).log_fields();
        assert_eq!(
            line,
            "protocol=vibe-talk-v1 chat=false cause=silent_turns since_open_ms=21400 turns=3 \
             silent_turns=2 audio_ms=1000 peak=0"
        );
        assert!(
            !line.contains('"'),
            "a quoted value is a free-text field: {line}"
        );
    }

    #[test]
    fn every_cause_has_the_wire_name_it_deserializes_from() {
        for cause in [
            Cause::SilentGreeting,
            Cause::SilentTurns,
            Cause::NoReply,
            Cause::ErrorFrame,
            Cause::Recovered,
            Cause::AudioWithoutText,
        ] {
            let wire = serde_json::to_value(cause).expect("serializes");
            assert_eq!(wire, cause.as_str());
        }
    }

    #[test]
    fn a_report_refuses_unknown_fields_values_and_absurd_numbers() {
        let mut smuggled = serde_json::to_value(report(Cause::NoReply)).expect("serializes");
        smuggled["message"] = "the upstream failed".into();
        assert!(serde_json::from_value::<HealthReport>(smuggled).is_err());
        let mut unknown = serde_json::to_value(report(Cause::NoReply)).expect("serializes");
        unknown["cause"] = "made_up".into();
        assert!(serde_json::from_value::<HealthReport>(unknown).is_err());
        for absurd in [
            HealthReport {
                since_open_ms: MAX_SINCE_OPEN_MS + 1,
                ..report(Cause::NoReply)
            },
            HealthReport {
                audio_ms: MAX_AUDIO_MS + 1,
                ..report(Cause::NoReply)
            },
            HealthReport {
                peak: MAX_PEAK + 1,
                ..report(Cause::NoReply)
            },
            HealthReport {
                silent_turns: 4,
                ..report(Cause::NoReply)
            },
        ] {
            assert!(absurd.validate().is_err(), "{absurd:?}");
        }
        report(Cause::NoReply).validate().expect("valid");
    }

    fn end() -> EndReport {
        EndReport {
            protocol: CallProtocol::VibeTalkV1,
            chat: false,
            ended_by: EndedBy::ServerClosed,
            close_code: Some(1011),
            since_open_ms: 95_000,
            turns: 4,
            visibility: Some(Visibility::Visible),
            queued_audio_ms: Some(41_000),
            last_text_ms: Some(2_000),
            last_audio_end_ms: Some(1_900),
            last_interrupt_ms: None,
            reconnects: None,
            speech_holds: None,
            speech_hold_max_ms: None,
            speech_hold_capped: None,
            audio_input: None,
            audio_output: None,
            close_reason: Some("interrupt boundary timed out; reconnect".to_owned()),
        }
    }

    #[test]
    fn a_call_that_held_page_messages_for_the_owners_speech_says_how_many_and_how_long() {
        // `#512 voice-recap-private-stack`. Numbers only, before the service's reason, and nothing
        // at all on a call that held none.
        let held = EndReport {
            speech_holds: Some(3),
            speech_hold_max_ms: Some(4_250),
            speech_hold_capped: Some(1),
            ..end()
        };
        held.validate().expect("valid");
        assert!(
            held.log_fields().ends_with(
                " reconnects=- speech_holds=3 speech_hold_max_ms=4250 speech_hold_capped=1 \
                 close_reason=\"interrupt boundary timed out; reconnect\""
            ),
            "{}",
            held.log_fields()
        );
        assert!(!end().log_fields().contains("speech_hold"));
        let absurd = EndReport {
            speech_holds: Some(MAX_SPEECH_HOLDS + 1),
            ..end()
        };
        assert!(absurd.validate().is_err());
    }

    #[test]
    fn a_call_that_captured_says_the_kinds_of_its_audio_devices_and_no_more() {
        // `#619 voice-bluetooth-output`. Kinds only, before the service's reason, and nothing
        // at all on a call that captured nothing.
        let routed = EndReport {
            audio_input: Some(AudioRoute::Bluetooth),
            audio_output: Some(AudioRoute::Bluetooth),
            ..end()
        };
        routed.validate().expect("valid");
        assert!(
            routed
                .log_fields()
                .contains(" audio_input=bluetooth audio_output=bluetooth close_reason="),
            "{}",
            routed.log_fields()
        );
        assert!(!end().log_fields().contains("audio_input"));
        let parsed: EndReport = serde_json::from_value(serde_json::json!({
            "protocol": "vibe-talk-v1", "chat": false, "ended_by": "hang_up",
            "since_open_ms": 1000, "turns": 1, "audio_input": "bluetooth",
        }))
        .expect("a route kind reads");
        assert_eq!(parsed.audio_input, Some(AudioRoute::Bluetooth));
        assert!(
            serde_json::from_value::<EndReport>(serde_json::json!({
                "protocol": "vibe-talk-v1", "chat": false, "ended_by": "hang_up",
                "since_open_ms": 1000, "turns": 1, "audio_input": "AirPods Pro",
            }))
            .is_err(),
            "a device LABEL must never be accepted into this record"
        );
    }

    #[test]
    fn a_call_end_line_says_what_decides_who_ended_it_and_quotes_only_the_services_reason() {
        // `#330 voice-long-turn-drop`.
        assert_eq!(
            end().log_fields(),
            "protocol=vibe-talk-v1 chat=false ended_by=server_closed close_code=1011 \
             since_open_ms=95000 turns=4 visibility=visible queued_audio_ms=41000 \
             last_text_ms=2000 last_audio_end_ms=1900 last_interrupt_ms=- reconnects=- \
             close_reason=\"interrupt boundary timed out; reconnect\""
        );
        // A reason that tries to look like more fields stays inside its quotes.
        let forged = EndReport {
            close_reason: Some("x\" ended_by=hang_up".to_owned()),
            ..end()
        };
        assert!(
            forged
                .log_fields()
                .ends_with(r#"close_reason="x\" ended_by=hang_up""#),
            "{}",
            forged.log_fields()
        );
        // The hosted protocol is recorded too, by its wire name.
        let hosted: EndReport = serde_json::from_value(serde_json::json!({
            "protocol": "elevenlabs", "chat": false, "ended_by": "hang_up",
            "since_open_ms": 1000, "turns": 1,
        }))
        .expect("an older page's record, on the hosted protocol, still reads");
        assert!(hosted.log_fields().starts_with("protocol=elevenlabs "));
        assert!(hosted.log_fields().ends_with("close_reason=-"));
    }

    #[test]
    fn a_reconnect_is_recorded_per_socket_and_the_calls_end_says_how_many_came_before() {
        // `#352 voice-reconnect-on-bridge-error`.
        for (ended_by, word) in [
            (EndedBy::Reconnected, "reconnected"),
            (EndedBy::ReconnectLimit, "reconnect_limit"),
            (EndedBy::ReconnectFailed, "reconnect_failed"),
        ] {
            let parsed: EndedBy =
                serde_json::from_value(serde_json::json!(word)).expect("a documented cause");
            assert_eq!(parsed, ended_by);
            let report = EndReport { ended_by, ..end() };
            report.validate().expect("valid");
            assert!(
                report.log_fields().contains(&format!(" ended_by={word} ")),
                "{}",
                report.log_fields()
            );
            assert!(
                !report.by_the_reader(),
                "{word} was taken for the reader's own hang-up"
            );
        }
        let after = EndReport {
            ended_by: EndedBy::HangUp,
            close_code: None,
            reconnects: Some(1),
            close_reason: None,
            ..end()
        };
        after.validate().expect("valid");
        assert!(
            after.log_fields().ends_with(" reconnects=1 close_reason=-"),
            "{}",
            after.log_fields()
        );
        assert!(after.by_the_reader());
        // A page that predates the count still records its calls.
        let older: EndReport = serde_json::from_value(serde_json::json!({
            "protocol": "vibe-talk-v1", "chat": false, "ended_by": "hang_up",
            "since_open_ms": 1000, "turns": 1,
        }))
        .expect("a record without a count reads");
        assert_eq!(older.reconnects, None);
    }

    #[test]
    fn a_call_end_record_refuses_a_long_or_control_laden_reason_and_absurd_offsets() {
        end().validate().expect("valid");
        for absurd in [
            EndReport {
                close_reason: Some("x".repeat(MAX_CLOSE_REASON_CHARS + 1)),
                ..end()
            },
            EndReport {
                close_reason: Some("line one\nline two".to_owned()),
                ..end()
            },
            EndReport {
                queued_audio_ms: Some(MAX_AUDIO_MS + 1),
                ..end()
            },
            EndReport {
                last_text_ms: Some(MAX_SINCE_OPEN_MS + 1),
                ..end()
            },
            EndReport {
                reconnects: Some(MAX_RECONNECTS + 1),
                ..end()
            },
        ] {
            assert!(absurd.validate().is_err(), "{absurd:?}");
        }
        // Counted in characters, not bytes: a reason in another script is not cut shorter.
        EndReport {
            close_reason: Some("é".repeat(MAX_CLOSE_REASON_CHARS)),
            ..end()
        }
        .validate()
        .expect("123 characters of two bytes each is still 123 characters");
    }

    #[test]
    fn the_budget_refuses_past_its_cap_and_refills_with_the_next_window() {
        let budget = LogBudget::new();
        let start = Instant::now();
        for _ in 0..MAX_LINES_PER_WINDOW {
            assert_eq!(budget.spend_at(start), Spend::Allowed);
        }
        assert_eq!(budget.spend_at(start), Spend::FirstRefusal);
        assert_eq!(budget.spend_at(start + BUDGET_WINDOW / 2), Spend::Refused);
        assert_eq!(budget.spend_at(start + BUDGET_WINDOW), Spend::Allowed);
    }
}
