//! Links to chat messages, read back into the messages they name. `#543 in-app-message-links`.
//!
//! # Why
//!
//! Agents cite earlier messages by their permalinks, which is the right habit — connect things by
//! reference rather than repeat them — and on a phone it is a bad trip: every tap leaves the app
//! for the chat service's own, and comes back to the top. The owner, 2026-10-26: "It would be
//! great if we could recognize chat links and translate them into vibe talk links to jump there in
//! the client directly … I would prefer to keep these message links in vibe talk by default
//! because you could always follow the link in vibe talk and then if you really want to copy the
//! slack link from there."
//!
//! So [`crate::render`] asks, of every link it draws, whether its address is a message's (or a
//! thread's, or a conversation's) in one of the services this server reads, and marks the link
//! with what it names ([`ATTRIBUTE`]). The address is untouched: the page decides, as it draws the
//! body, whether the channel is one it serves and whether the reader wants links kept in the app,
//! and only then makes the tap a jump. A link the page does not take keeps going where it went.
//!
//! # The shapes
//!
//! Each the inverse of what [`crate::upstream_url`] forms, plus the other addresses each service
//! hands out for the same thing, and nothing more:
//!
//! | Service | Address | Names |
//! |---|---|---|
//! | Slack | `https://WS.slack.com/archives/C/pSECONDSMICROS` | the message, in Main |
//! | Slack | the same, `?thread_ts=ROOT&cid=C` | a reply, in its thread |
//! | Slack | `https://WS.slack.com/archives/C` | the conversation |
//! | Slack | `https://app.slack.com/client/T/C[/thread/C-ROOT]` | the conversation, or a thread |
//! | Slack | `slack://channel?team=T&id=C[&message=TS[&thread_ts=ROOT]]` | the same, in Slack's app |
//! | Discord | `https://discord.com/channels/G/C[/M]`, and `discordapp.com`, `ptb.`, `canary.` | the channel, or a message in it |
//! | Google Chat | `https://chat.google.com/[u/N/]room/S[/T[/M]]`, and `dm/` | the space, a thread, or a message in it |
//! | Google Chat | `https://mail.google.com/(chat|mail)/u/N/#chat/space/S[/T[/M]]`, and `dm/` | the same, in Gmail |
//!
//! The ids are this server's own spelling of the upstream's ([`crate::slack::ids`] for Slack, whose
//! `ts` is a snowflake-shaped message id here and whose thread is its root's `ts`; Discord's
//! snowflakes verbatim; a Google Chat space, thread and message as the bridge's resource names).
//! A Google Chat message part is read as the system-assigned name `spaces/S/messages/T.M`; a
//! bridge that presents ids of its own will not find it, and the page then opens the thread the
//! link also names.
//!
//! # Checked, never decoded
//!
//! Every part is checked against the shape its service mints — a conversation id, a canonical
//! `ts`, a snowflake, a Google id of letters, digits, `-` and `_` — and an address with any part
//! of another shape names nothing. A part is never percent-decoded: an escape is not in any of
//! those shapes, so it simply fails the check.

use url::Url;

/// The attribute [`crate::render`] writes on a link this recognizes, holding [`Target::attribute`].
pub const ATTRIBUTE: &str = "data-msg";

/// The longest address read, in bytes; a longer one names nothing. Far past any real permalink.
const MAX_ADDRESS: usize = 1024;

/// The longest Google Chat id read, in bytes.
const MAX_GOOGLE_ID: usize = 128;

/// The service a recognized address belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// Slack, on the web or in its app.
    Slack,
    /// Discord.
    Discord,
    /// Google Chat, on its own site or in Gmail.
    GoogleChat,
}

impl Service {
    /// The name the page is told, lowercase and stable.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Slack => "slack",
            Self::Discord => "discord",
            Self::GoogleChat => "gchat",
        }
    }
}

/// What a recognized address names, in this server's ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The service it belongs to.
    pub service: Service,
    /// The channel, as this server names it.
    pub channel: String,
    /// The message, when the address names one.
    pub message: Option<String>,
    /// The thread the message is a reply in, or the thread the address opens; `None` for a
    /// message on the channel's own timeline and for the channel itself.
    pub thread: Option<String>,
}

impl Target {
    /// The value of [`ATTRIBUTE`]: `service:channel:message:thread`, each part percent-encoded so
    /// a `:` or `/` inside an id is data, an absent part empty. Only unreserved characters, `%`
    /// and `:` remain, so the value needs no escaping in an attribute.
    #[must_use]
    pub fn attribute(&self) -> String {
        [
            self.service.key(),
            &self.channel,
            self.message.as_deref().unwrap_or(""),
            self.thread.as_deref().unwrap_or(""),
        ]
        .iter()
        .map(|part| encode(part))
        .collect::<Vec<_>>()
        .join(":")
    }
}

/// What `address` names, or `None` when it is not one of the shapes in the module note.
#[must_use]
pub fn recognize(address: &str) -> Option<Target> {
    if address.len() > MAX_ADDRESS
        || address
            .bytes()
            .any(|byte| byte == b'\\' || byte == b' ' || byte.is_ascii_control())
    {
        return None;
    }
    let url = Url::parse(address).ok()?;
    match url.scheme() {
        "slack" => slack_app(&url),
        "http" | "https" => {
            if url.username() != "" || url.password().is_some() || url.port().is_some() {
                return None;
            }
            let host = url.host_str()?.to_ascii_lowercase();
            let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
            if host == "app.slack.com" {
                slack_client(&segments)
            } else if host == "slack.com" || host.ends_with(".slack.com") {
                slack_archive(&url, &segments)
            } else if DISCORD_HOSTS.contains(&host.as_str()) {
                discord(&segments)
            } else if host == "chat.google.com" {
                google_chat(&segments)
            } else if host == "mail.google.com" {
                gmail_chat(&url, &segments)
            } else {
                None
            }
        }
        _ => None,
    }
}

// --- Slack -----------------------------------------------------------------------------------

/// A Slack target from its conversation, the message's `ts`, and the root `ts` of the thread it
/// is a reply in (a root's own `ts` is no thread: the root is on the channel's timeline).
fn slack(conversation: &str, ts: Option<&str>, root: Option<&str>) -> Option<Target> {
    use crate::slack::ids;
    if !ids::is_conversation_id(conversation) {
        return None;
    }
    // A part that is present must be in shape; an absent one is simply absent.
    let checked = |part: Option<&str>| match part {
        Some(part) => permalink_shaped_ts(part).map(Some),
        None => Some(None),
    };
    let ts = checked(ts)?;
    let root = checked(root)?;
    let message = match &ts {
        Some(ts) => Some(ids::message_id_from_ts(ts)?.as_str().to_owned()),
        None => None,
    };
    let thread = match (&ts, root) {
        (Some(ts), Some(root)) if *ts == root => None,
        (Some(_), root) => root,
        // A thread named without a message: its root is the message.
        (None, Some(_)) => return None,
        (None, None) => None,
    };
    Some(Target {
        service: Service::Slack,
        channel: conversation.to_owned(),
        message,
        thread,
    })
}

/// A `ts` as Slack mints one, canonical: ten second digits, a dot, six microsecond digits.
fn permalink_shaped_ts(ts: &str) -> Option<String> {
    let canonical = crate::slack::ids::canonical_ts(ts)?;
    (canonical == ts && canonical.len() == 17).then_some(canonical)
}

/// `p1700000000000100`, the permalink's message segment, as a `ts`.
fn permalink_segment_ts(segment: &str) -> Option<String> {
    let digits = segment.strip_prefix('p')?;
    if digits.len() != 16 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(format!("{}.{}", &digits[..10], &digits[10..]))
}

/// The value of query parameter `key`, as written, when it appears once; `Err` when it appears
/// more than once, which no address Slack mints does.
fn query_value<'u>(url: &'u Url, key: &str) -> Result<Option<&'u str>, ()> {
    let mut found = None;
    for pair in url.query().unwrap_or("").split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            if found.is_some() {
                return Err(());
            }
            found = Some(value);
        }
    }
    Ok(found)
}

/// `https://WS.slack.com/archives/C[/pTS][?thread_ts=ROOT&cid=C]`.
fn slack_archive(url: &Url, segments: &[&str]) -> Option<Target> {
    let ["archives", conversation, rest @ ..] = segments else {
        return None;
    };
    if let Some(cid) = query_value(url, "cid").ok()? {
        if cid != *conversation {
            return None;
        }
    }
    let root = query_value(url, "thread_ts").ok()?;
    match rest {
        [] => root.is_none().then(|| slack(conversation, None, None))?,
        [segment] => slack(conversation, Some(&permalink_segment_ts(segment)?), root),
        _ => None,
    }
}

/// `https://app.slack.com/client/T/C[/thread/C-ROOT]`.
fn slack_client(segments: &[&str]) -> Option<Target> {
    let ["client", team, conversation, rest @ ..] = segments else {
        return None;
    };
    if !crate::slack::ids::is_team_id(team) {
        return None;
    }
    match rest {
        [] => slack(conversation, None, None),
        ["thread", scoped] => {
            let (owner, root) = scoped.split_once('-')?;
            if owner != *conversation {
                return None;
            }
            // The thread itself, opened at its root: no message is named over it.
            let mut target = slack(conversation, Some(root), None)?;
            target.message = None;
            target.thread = Some(root.to_owned());
            Some(target)
        }
        _ => None,
    }
}

/// `slack://channel?team=T&id=C[&message=TS[&thread_ts=ROOT]]`, as [`crate::upstream_url`] forms it.
fn slack_app(url: &Url) -> Option<Target> {
    if url.host_str() != Some("channel") || !matches!(url.path(), "" | "/") {
        return None;
    }
    let team = query_value(url, "team").ok()??;
    if !crate::slack::ids::is_team_id(team) {
        return None;
    }
    let conversation = query_value(url, "id").ok()??;
    let message = query_value(url, "message").ok()?;
    let root = query_value(url, "thread_ts").ok()?;
    slack(conversation, message, root)
}

// --- Discord ---------------------------------------------------------------------------------

/// The hosts Discord's clients hand links out on.
const DISCORD_HOSTS: [&str; 7] = [
    "discord.com",
    "www.discord.com",
    "discordapp.com",
    "ptb.discord.com",
    "canary.discord.com",
    "ptb.discordapp.com",
    "canary.discordapp.com",
];

/// `/channels/G/C[/M]`, a guild a snowflake or `@me` for a direct message.
fn discord(segments: &[&str]) -> Option<Target> {
    use crate::upstream_url::is_snowflake;
    let ["channels", guild, channel, rest @ ..] = segments else {
        return None;
    };
    if !(*guild == "@me" || is_snowflake(guild)) || !is_snowflake(channel) {
        return None;
    }
    let message = match rest {
        [] => None,
        [message] if is_snowflake(message) => Some((*message).to_owned()),
        _ => return None,
    };
    Some(Target {
        service: Service::Discord,
        channel: (*channel).to_owned(),
        message,
        thread: None,
    })
}

// --- Google Chat -----------------------------------------------------------------------------

/// `[u/N/](room|dm)/S[/T[/M]]` on chat.google.com.
fn google_chat(segments: &[&str]) -> Option<Target> {
    let segments = match segments {
        ["u", account, rest @ ..] if account.bytes().all(|b| b.is_ascii_digit()) => rest,
        _ => segments,
    };
    let [kind, rest @ ..] = segments else {
        return None;
    };
    if !matches!(*kind, "room" | "dm") {
        return None;
    }
    google_parts(rest)
}

/// `/chat/u/N/#chat/(space|dm)/S[/T[/M]]` on mail.google.com, and the same under Gmail's own
/// `/mail/u/N/`: the conversation is in the fragment.
fn gmail_chat(url: &Url, segments: &[&str]) -> Option<Target> {
    match segments {
        ["chat" | "mail"] => {}
        ["chat" | "mail", "u", account] if account.bytes().all(|b| b.is_ascii_digit()) => {}
        _ => return None,
    }
    let fragment = url.fragment()?;
    let parts: Vec<&str> = fragment.split('/').collect();
    let ["chat", kind, rest @ ..] = parts.as_slice() else {
        return None;
    };
    if !matches!(*kind, "space" | "dm") {
        return None;
    }
    google_parts(rest)
}

/// A space, and a thread and a message in it, as resource names.
fn google_parts(parts: &[&str]) -> Option<Target> {
    if parts.is_empty() || parts.len() > 3 || !parts.iter().all(|part| google_id(part)) {
        return None;
    }
    let space = parts[0];
    let channel = format!("spaces/{space}");
    let thread = parts.get(1);
    let message = parts.get(2);
    Some(Target {
        service: Service::GoogleChat,
        message: match (thread, message) {
            (Some(thread), Some(message)) => Some(format!("{channel}/messages/{thread}.{message}")),
            _ => None,
        },
        // A thread's first message is its root, on the space's own timeline.
        thread: match (thread, message) {
            (Some(thread), Some(message)) if thread == message => None,
            (Some(thread), _) => Some(format!("{channel}/threads/{thread}")),
            (None, _) => None,
        },
        channel,
    })
}

/// A Google Chat id as a link carries one: letters, digits, `-` and `_`.
fn google_id(part: &str) -> bool {
    (1..=MAX_GOOGLE_ID).contains(&part.len())
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// `text` percent-encoded but for the unreserved characters.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{recognize, Service, Target};

    const C: &str = "C0123ABCDE";
    /// `1700000000.000100` as this server's message id.
    fn slack_id(ts: &str) -> String {
        crate::slack::ids::message_id_from_ts(ts)
            .expect("a ts")
            .as_str()
            .to_owned()
    }

    fn target(
        service: Service,
        channel: &str,
        message: Option<&str>,
        thread: Option<&str>,
    ) -> Target {
        Target {
            service,
            channel: channel.to_owned(),
            message: message.map(str::to_owned),
            thread: thread.map(str::to_owned),
        }
    }

    #[track_caller]
    fn none(addresses: &[&str]) {
        for address in addresses {
            assert_eq!(recognize(address), None, "{address} was recognized");
        }
    }

    #[test]
    fn a_slack_permalink_names_its_message_and_a_reply_its_thread() {
        let ts = "1700000000.000100";
        let root = "1699999999.000200";
        assert_eq!(
            recognize(&format!(
                "https://acme.slack.com/archives/{C}/p1700000000000100"
            )),
            Some(target(Service::Slack, C, Some(&slack_id(ts)), None))
        );
        assert_eq!(
            recognize(&format!(
                "https://acme.slack.com/archives/{C}/p1700000000000100?thread_ts={root}&cid={C}"
            )),
            Some(target(Service::Slack, C, Some(&slack_id(ts)), Some(root)))
        );
        // The root's own link, as Slack writes it from inside the thread, is the root, in Main.
        assert_eq!(
            recognize(&format!(
                "https://acme.slack.com/archives/{C}/p1700000000000100?thread_ts={ts}&cid={C}"
            )),
            Some(target(Service::Slack, C, Some(&slack_id(ts)), None))
        );
        // The conversation alone, a private group and a direct message.
        assert_eq!(
            recognize(&format!("https://acme.slack.com/archives/{C}")),
            Some(target(Service::Slack, C, None, None))
        );
        assert!(
            recognize("https://acme.slack.com/archives/G0123ABCDE/p1700000000000100").is_some()
        );
        assert!(
            recognize("https://acme.slack.com/archives/D0123ABCDE/p1700000000000100").is_some()
        );
        // Either scheme, any case of the host, a trailing slash, an unrelated parameter.
        assert!(recognize(&format!(
            "http://ACME.Slack.com/archives/{C}/p1700000000000100/"
        ))
        .is_some());
        assert!(recognize(&format!(
            "https://acme.slack.com/archives/{C}/p1700000000000100?utm_source=x"
        ))
        .is_some());
    }

    #[test]
    fn slacks_web_client_and_app_links_name_the_same_things() {
        let ts = "1700000000.000100";
        assert_eq!(
            recognize(&format!("https://app.slack.com/client/T0123ABCD/{C}")),
            Some(target(Service::Slack, C, None, None))
        );
        assert_eq!(
            recognize(&format!(
                "https://app.slack.com/client/T0123ABCD/{C}/thread/{C}-{ts}"
            )),
            Some(target(Service::Slack, C, None, Some(ts)))
        );
        assert_eq!(
            recognize(&format!(
                "slack://channel?team=T0123ABCD&id={C}&message={ts}"
            )),
            Some(target(Service::Slack, C, Some(&slack_id(ts)), None))
        );
        assert_eq!(
            recognize(&format!(
                "slack://channel?team=T0123ABCD&id={C}&message={ts}&thread_ts=1699999999.000200"
            )),
            Some(target(
                Service::Slack,
                C,
                Some(&slack_id(ts)),
                Some("1699999999.000200")
            ))
        );
        assert_eq!(
            recognize(&format!("slack://channel?team=T0123ABCD&id={C}")),
            Some(target(Service::Slack, C, None, None))
        );
        // Each address upstream_url forms is read back to the message it was formed for.
        let formed = crate::upstream_url::slack_permalink(
            "https://acme.slack.com",
            C,
            ts,
            Some("1699999999.000200"),
        )
        .expect("formed");
        assert_eq!(
            recognize(&formed),
            Some(target(
                Service::Slack,
                C,
                Some(&slack_id(ts)),
                Some("1699999999.000200")
            ))
        );
        let app = crate::upstream_url::slack_app_url("T0123ABCD", C, ts, None).expect("formed");
        assert_eq!(
            recognize(&app),
            Some(target(Service::Slack, C, Some(&slack_id(ts)), None))
        );
    }

    #[test]
    fn a_slack_address_with_any_part_out_of_shape_names_nothing() {
        none(&[
            "https://acme.slack.com/archives/c0123abcde/p1700000000000100",
            "https://acme.slack.com/archives/X0123ABCDE/p1700000000000100",
            &format!("https://acme.slack.com/archives/{C}/p170000000000010"),
            &format!("https://acme.slack.com/archives/{C}/p17000000000001000"),
            &format!("https://acme.slack.com/archives/{C}/1700000000000100"),
            &format!("https://acme.slack.com/archives/{C}/p1700000000000100/extra"),
            &format!("https://acme.slack.com/archives/{C}/p1700000000000100?thread_ts=17.1&cid={C}"),
            &format!("https://acme.slack.com/archives/{C}/p1700000000000100?thread_ts=1699999999.000200&cid=C9999ZZZZZ"),
            &format!("https://acme.slack.com/archives/{C}/p1700000000000100?thread_ts=1&thread_ts=2"),
            &format!("https://acme.slack.com/archives/{C}?thread_ts=1699999999.000200"),
            &format!("https://acme.slack.com/archives/%43{}/p1700000000000100", &C[1..]),
            &format!("https://acme.slack.com/files/{C}/p1700000000000100"),
            &format!("https://slack.com.example.com/archives/{C}/p1700000000000100"),
            &format!("https://notslack.com/archives/{C}/p1700000000000100"),
            &format!("https://acme.slack.com:8443/archives/{C}/p1700000000000100"),
            &format!("https://user@acme.slack.com/archives/{C}/p1700000000000100"),
            &format!("https://app.slack.com/client/X0123/{C}"),
            &format!("https://app.slack.com/client/T0123ABCD/{C}/thread/C9999ZZZZZ-1700000000.000100"),
            &format!("slack://channel?team=T0123ABCD&id={C}&thread_ts=1700000000.000100"),
            &format!("slack://channel?id={C}&message=1700000000.000100"),
            &format!("slack://user?team=T0123ABCD&id={C}"),
            &format!("slack://channel?team=T0123ABCD&id={C}&message=1700000000.1"),
        ]);
    }

    #[test]
    fn a_discord_link_names_its_channel_and_message() {
        let (g, c, m) = (
            "100000000000000001",
            "100000000000000002",
            "100000000000000003",
        );
        assert_eq!(
            recognize(&format!("https://discord.com/channels/{g}/{c}/{m}")),
            Some(target(Service::Discord, c, Some(m), None))
        );
        assert_eq!(
            recognize(&format!("https://discord.com/channels/{g}/{c}")),
            Some(target(Service::Discord, c, None, None))
        );
        for host in [
            "canary.discordapp.com",
            "discordapp.com",
            "ptb.discord.com",
            "canary.discord.com",
            "www.discord.com",
        ] {
            assert!(
                recognize(&format!("https://{host}/channels/{g}/{c}/{m}")).is_some(),
                "{host}"
            );
        }
        assert!(recognize(&format!("https://discord.com/channels/@me/{c}/{m}")).is_some());
        let formed = crate::upstream_url::discord_message_url(g, c, m).expect("formed");
        assert_eq!(
            recognize(&formed),
            Some(target(Service::Discord, c, Some(m), None))
        );
        none(&[
            &format!("https://discord.com/channels/{g}"),
            &format!("https://discord.com/channels/{g}/{c}/{m}/x"),
            &format!("https://discord.com/channels/{g}/12345/{m}"),
            &format!("https://discord.com/channels/guild/{c}/{m}"),
            &format!("https://discord.gg/{c}"),
            &format!("https://evil-discord.com/channels/{g}/{c}/{m}"),
        ]);
    }

    #[test]
    fn a_google_chat_link_names_its_space_thread_and_message() {
        let space = "spaces/AAAa1";
        assert_eq!(
            recognize("https://chat.google.com/room/AAAa1/BBBb2/CCCc3"),
            Some(target(
                Service::GoogleChat,
                space,
                Some("spaces/AAAa1/messages/BBBb2.CCCc3"),
                Some("spaces/AAAa1/threads/BBBb2")
            ))
        );
        // A thread's first message is its root, on the space's timeline.
        assert_eq!(
            recognize("https://chat.google.com/room/AAAa1/BBBb2/BBBb2"),
            Some(target(
                Service::GoogleChat,
                space,
                Some("spaces/AAAa1/messages/BBBb2.BBBb2"),
                None
            ))
        );
        assert_eq!(
            recognize("https://chat.google.com/u/0/room/AAAa1/BBBb2"),
            Some(target(
                Service::GoogleChat,
                space,
                None,
                Some("spaces/AAAa1/threads/BBBb2")
            ))
        );
        assert_eq!(
            recognize("https://chat.google.com/dm/AAAa1"),
            Some(target(Service::GoogleChat, space, None, None))
        );
        assert_eq!(
            recognize("https://mail.google.com/chat/u/0/#chat/space/AAAa1/BBBb2"),
            Some(target(
                Service::GoogleChat,
                space,
                None,
                Some("spaces/AAAa1/threads/BBBb2")
            ))
        );
        assert_eq!(
            recognize("https://mail.google.com/mail/u/1/#chat/space/AAAa1"),
            Some(target(Service::GoogleChat, space, None, None))
        );
        // What upstream_url forms is read back.
        let thread = crate::threads::MessageThread {
            id: "spaces/AAAa1/threads/BBBb2".to_owned(),
            root_message_id: None,
            is_root: false,
            reply_count: None,
            reply_count_exact: false,
            hue: None,
            display_name: None,
            summary: None,
        };
        let formed = crate::upstream_url::google_chat_url(
            space,
            "spaces/AAAa1/messages/BBBb2.CCCc3",
            Some(&thread),
        )
        .expect("formed");
        assert_eq!(
            recognize(&formed).and_then(|t| t.message).as_deref(),
            Some("spaces/AAAa1/messages/BBBb2.CCCc3")
        );
        none(&[
            "https://chat.google.com/",
            "https://chat.google.com/room",
            "https://chat.google.com/room/AA%41/BBBb2",
            "https://chat.google.com/room/AAAa1/BBBb2/CCCc3/DDD",
            "https://chat.google.com/room/AA.a1",
            "https://chat.google.com/settings/AAAa1",
            "https://mail.google.com/mail/u/0/#inbox/AAAa1",
            "https://mail.google.com/chat/u/0/#chat/home",
            "https://chat.google.com.example.com/room/AAAa1",
        ]);
    }

    #[test]
    fn other_addresses_and_absurd_ones_name_nothing() {
        none(&[
            "https://github.com/octo/widgets/pull/41",
            "https://example.com/archives/C0123ABCDE/p1700000000000100",
            "ftp://acme.slack.com/archives/C0123ABCDE/p1700000000000100",
            "acme.slack.com/archives/C0123ABCDE/p1700000000000100",
            "https://acme.slack.com/archives/C0123ABCDE/p1700000000000100\\",
            "",
            &format!(
                "https://acme.slack.com/archives/C0123ABCDE/p1700000000000100?x={}",
                "y".repeat(1100)
            ),
        ]);
    }

    #[test]
    fn the_attribute_escapes_every_part_and_keeps_empty_ones() {
        let gchat = target(
            Service::GoogleChat,
            "spaces/A",
            Some("spaces/A/messages/T.M"),
            Some("spaces/A/threads/T"),
        );
        assert_eq!(
            gchat.attribute(),
            "gchat:spaces%2FA:spaces%2FA%2Fmessages%2FT.M:spaces%2FA%2Fthreads%2FT"
        );
        assert_eq!(
            target(Service::Slack, C, None, None).attribute(),
            format!("slack:{C}::")
        );
    }
}
