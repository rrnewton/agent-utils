//! Read-only views of a bridge's own records for `chat health` and `chat latency` (#677
//! chat-bridge-repoint). They take the shared state lock, write nothing, and contact nothing.

use super::*;

/// The times one retained request reached each stage, from the bridge's records. Chat providers
/// such as Slack keep no time on a reaction, so these records are the only source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RequestTimes {
    /// The request key.
    pub key: String,
    /// The provider thread the request was posted in.
    pub thread_id: String,
    /// When the provider says the message was posted, when its time could be read.
    pub posted_at_millis: Option<u64>,
    /// When the bridge stored the message as a request.
    pub admitted_at_millis: u64,
    /// When the acknowledgement reaction (the first) was confirmed added.
    pub acknowledged_at_millis: Option<u64>,
    /// When the prompt was verified typed into the agent, or a headless model started it: the
    /// moment the receipt reaction (the second) is queued.
    pub delivered_at_millis: Option<u64>,
    /// When the provider accepted the request's first reply.
    pub first_reply_sent_at_millis: Option<u64>,
    /// When the provider accepted the request's latest sent reply.
    pub last_reply_sent_at_millis: Option<u64>,
    /// `pending`, `submitting`, `delivered` or `delivery_uncertain`.
    pub phase: &'static str,
    /// Why the prompt is not typed, or why its delivery is uncertain.
    pub delivery_error: Option<String>,
    /// Replies captured for the request.
    pub replies_captured: u32,
    /// Whether the request's replies are closed.
    pub replies_closed: bool,
}

/// One reply that is not sent, as `chat health` reports it. Retry fields are read when the
/// record carries them, and are `None` otherwise.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UnsentReply {
    /// The request key.
    pub key: String,
    /// The reply's number within its request.
    pub ordinal: u32,
    /// `pending` or `sending`.
    pub phase: String,
    /// When the reply was captured.
    pub captured_at_millis: Option<u64>,
    /// Send attempts the outbound helper has made for it.
    pub send_attempts: Option<u64>,
    /// The helper's code for the last failed attempt, such as `operation_conflict`.
    pub last_send_failure_code: Option<String>,
    /// When the last attempt failed.
    pub last_send_failure_at_millis: Option<u64>,
    /// Why the reply will never be sent, when that is settled.
    pub send_permanent_failure: Option<String>,
}

/// The marker file whose modification time is the provider worker's last item.
const PROVIDER_ALIVE_FILE: &str = "provider-alive";

fn modified_age(path: &Path, now_millis: u64) -> Option<u64> {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()?;
    let written = u64::try_from(modified.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()?;
    Some(now_millis.saturating_sub(written))
}

/// Unix milliseconds of an RFC 3339 time such as `2026-10-22T13:57:48.649079Z` or
/// `2026-10-22T09:57:48-04:00`; `None` for anything else.
#[must_use]
pub fn parse_rfc3339_millis(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || !matches!(bytes[10], b'T' | b't')
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse().ok())
            .flatten()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 60 {
        return None;
    }
    let mut rest = &text[19..];
    let mut millis = 0_i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let padded = format!("{:0<3}", &fraction[..digits.min(3)]);
        millis = padded.parse().ok()?;
        rest = &fraction[digits..];
    }
    let offset_minutes = match rest {
        "Z" | "z" => 0,
        _ if rest.len() == 6 && rest.as_bytes()[3] == b':' => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hours: i64 = rest.get(1..3)?.parse().ok()?;
            let minutes: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (hours * 60 + minutes)
        }
        _ => return None,
    };
    // Days from the civil date, after Howard Hinnant's algorithm.
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    u64::try_from(seconds * 1_000 + millis).ok()
}

impl BridgeState {
    /// Every retained request's stage times, oldest admission first.
    pub fn request_times(&self) -> Result<Vec<RequestTimes>> {
        let _snapshot = self.lock_state_snapshot()?;
        let mut records = self.request_records()?;
        records.sort_by(|(left, _), (right, _)| {
            (left.admitted_at_millis, &left.key).cmp(&(right.admitted_at_millis, &right.key))
        });
        let mut times = Vec::with_capacity(records.len());
        for (record, _) in records {
            let sent_at = |ordinal: u32| {
                self.read_reply(&record.key, ordinal)
                    .ok()
                    .filter(|reply| reply.phase == ReplyPhase::Sent)
                    .and_then(|reply| reply.sent_at_millis)
            };
            let first_reply_sent_at_millis = (record.reply_count > 0).then(|| sent_at(1)).flatten();
            let last_sent = record.next_send_ordinal.saturating_sub(1);
            let last_reply_sent_at_millis = if last_sent > 1 {
                sent_at(last_sent).or(first_reply_sent_at_millis)
            } else {
                first_reply_sent_at_millis
            };
            times.push(RequestTimes {
                posted_at_millis: parse_rfc3339_millis(&record.message.created_at),
                thread_id: record.message.thread_id.clone(),
                admitted_at_millis: record.admitted_at_millis,
                acknowledged_at_millis: record.ack_completed_at_millis,
                delivered_at_millis: record.delivered_at_millis,
                first_reply_sent_at_millis,
                last_reply_sent_at_millis,
                phase: request_phase_name(&record.phase),
                delivery_error: record.delivery_error.clone(),
                replies_captured: record.reply_count,
                replies_closed: record.reply_closed,
                key: record.key,
            });
        }
        Ok(times)
    }

    /// Every captured reply of a retained request that is not sent, read as plain JSON so that
    /// retry fields recorded by any build are reported.
    pub fn unsent_replies(&self) -> Result<Vec<UnsentReply>> {
        let _snapshot = self.lock_state_snapshot()?;
        let mut unsent = Vec::new();
        for (record, _) in self.request_records()? {
            for ordinal in record.next_send_ordinal.max(1)..record.next_reply_ordinal {
                let Ok(reply) = read_document::<Value>(
                    &self.reply_path(&record.key, ordinal),
                    MAX_REPLY_RECORD_BYTES,
                ) else {
                    continue;
                };
                let phase = reply["phase"].as_str().unwrap_or("unknown");
                if phase == "sent" {
                    continue;
                }
                let text = |name: &str| reply[name].as_str().map(str::to_owned);
                unsent.push(UnsentReply {
                    key: record.key.clone(),
                    ordinal,
                    phase: phase.to_owned(),
                    captured_at_millis: reply["captured_at_millis"].as_u64(),
                    send_attempts: reply["send_attempts"].as_u64(),
                    last_send_failure_code: text("last_send_failure_code"),
                    last_send_failure_at_millis: reply["last_send_failure_at_millis"].as_u64(),
                    send_permanent_failure: text("send_permanent_failure"),
                });
            }
        }
        Ok(unsent)
    }

    /// Touch `provider-alive` in the state directory: the provider worker does at each item it
    /// takes from an open subscription, heartbeats included, at most every few seconds, so its
    /// modification time says the event stream is alive even on a quiet channel. Only the time
    /// changes and nothing is synced; a failure is ignored.
    pub(crate) fn note_provider_alive(&self) {
        let _ = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.root.join(PROVIDER_ALIVE_FILE))
            .and_then(|file| file.set_modified(SystemTime::now()));
    }

    /// How long ago the provider worker last took an item from an open subscription, in
    /// milliseconds; `None` when no service built with this record has run on the state.
    #[must_use]
    pub fn provider_alive_age_millis(&self, now_millis: u64) -> Option<u64> {
        modified_age(&self.root.join(PROVIDER_ALIVE_FILE), now_millis)
    }

    /// The process holding this state's runner lease, that is the running `chat run`, if any.
    #[must_use]
    pub fn runner_pid(&self) -> Option<u64> {
        crate::lock_wait::holder_of(&self.root.join(".run.lock"))
    }

    /// How long ago the checkpoint, which the provider worker rewrites with each batch it commits,
    /// was last written, in milliseconds.
    #[must_use]
    pub fn checkpoint_age_millis(&self, now_millis: u64) -> Option<u64> {
        modified_age(&self.root.join("checkpoint.json"), now_millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_times_parse_with_fractions_and_offsets() {
        assert_eq!(parse_rfc3339_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_millis("2026-10-22T13:57:48.649079Z"),
            Some(1_792_677_468_649)
        );
        assert_eq!(
            parse_rfc3339_millis("2026-10-22T09:57:48.6-04:00"),
            Some(1_792_677_468_600)
        );
        assert_eq!(
            parse_rfc3339_millis("2024-02-29T12:00:00+00:00"),
            Some(1_709_208_000_000)
        );
        for bad in [
            "",
            "2026-10-22",
            "2026-13-01T00:00:00Z",
            "2026-10-22T13:57:48",
            "x",
        ] {
            assert_eq!(parse_rfc3339_millis(bad), None, "{bad}");
        }
    }
}
