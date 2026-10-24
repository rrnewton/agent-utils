//! A message's own address in the service it came from. `#408 copy-upstream-link`.
//!
//! The page's "Open in \<service\>" link opens this, and its copy key copies it. It is formed
//! HERE, on the server, from the
//! parts a read already holds, because each service's shape is a fact about that service and the
//! page should not guess at any of them. A part that cannot be read gives NO address rather than
//! a plausible one: a copied link that opens the wrong place, or nowhere, is worse than no menu
//! item, and the page offers the item only where an address came back.
//!
//! * **Slack**: the message permalink, `{workspace}/archives/{conversation}/p{ts without its
//!   dot}`, a thread reply's carrying `?thread_ts={root}&cid={conversation}` so it opens the
//!   thread at the reply — the same shapes [`crate::slack::ids::parse_source`] accepts back. The
//!   workspace is one origin (`https://acme.slack.com`), configured or reported by the provider;
//!   see [`crate::slack::HttpSlackClient::upstream_url`]. Beside it, the address in Slack's own
//!   app (`#481 slack-app-deep-link`): the documented `slack://channel?team={team}&id={conversation}`
//!   (Slack's deep-linking docs), carrying the message's `ts` and a reply's `thread_ts` as the
//!   permalink names them, so an app that honors them opens at the message and one that does not
//!   still opens its channel — never a browser tab left behind by the handoff.
//! * **Discord**: `https://discord.com/channels/{guild}/{channel}/{message}`, only where the
//!   guild (server) is known.
//! * **Google Chat**, read through a Discord-protocol bridge, whose ids are Chat resource names:
//!   `https://chat.google.com/room/{space}/{thread}/{message}`. The shapes are the web client's
//!   own, from public sources: Google's sample code builds `room/{space}/{thread}` from a
//!   message's space and thread names, Google's event payloads carry `room/{space}` as a space's
//!   URI, and a system-assigned message name is `spaces/{space}/messages/{thread}.{message}`,
//!   whose two parts are the link's last two segments. Where the message part cannot be read —
//!   a bridge that presents its own message ids — the THREAD's link is given, and where no
//!   thread is known, the SPACE's: each is a confirmed shape that lands the reader in the
//!   conversation, and no unconfirmed segment is invented to look more precise.
//!
//! Every part is provider data, so every part is checked before it is put in an address: a Slack
//! conversation must be conversation-shaped and its `ts` canonical, Discord's parts snowflakes,
//! and a Google Chat segment is percent-encoded so an id can never add a path, a query or a
//! fragment of its own.

use crate::threads::MessageThread;

/// The longest address formed here, in bytes. Real ones are well under two hundred; the bound is
/// what an id padded to absurdity is refused at, so a copied link stays a link.
const MAX_URL_BYTES: usize = 512;

/// A Slack workspace's origin from `url` as configured or as `auth.test` reported it
/// (`https://acme.slack.com/`): scheme, host and port, no path. `None` for anything that is not
/// an `https` URL with a host — a link to hand a reader is never formed on any other scheme.
#[must_use]
pub fn workspace_origin(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url.trim()).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str().filter(|host| !host.is_empty())?;
    Some(match parsed.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    })
}

/// A Slack message's permalink on the workspace at `workspace` (an origin, see
/// [`workspace_origin`]): the message `ts` in `conversation`, in the thread rooted at
/// `thread_root_ts` when it is a reply there. `None` when any part cannot be read.
#[must_use]
pub fn slack_permalink(
    workspace: &str,
    conversation: &str,
    ts: &str,
    thread_root_ts: Option<&str>,
) -> Option<String> {
    let origin = workspace_origin(workspace)?;
    if !crate::slack::ids::is_conversation_id(conversation) {
        return None;
    }
    let ts = crate::slack::ids::canonical_ts(ts)?;
    let segment: String = ts.chars().filter(|ch| *ch != '.').collect();
    // A permalink segment is ten second digits and six microsecond digits — what Slack mints,
    // and what this server reads back (`slack::ids`' permalink parser). A `ts` of any other
    // length is provider data of no shape Slack would resolve, so it forms nothing.
    if segment.len() != 16 {
        return None;
    }
    let mut url = format!("{origin}/archives/{conversation}/p{segment}");
    if let Some(root) = thread_root_ts {
        let root = crate::slack::ids::canonical_ts(root)?;
        if root == ts || root.replace('.', "").len() != 16 {
            return None;
        }
        url.push_str(&format!("?thread_ts={root}&cid={conversation}"));
    }
    (url.len() <= MAX_URL_BYTES).then_some(url)
}

/// A Slack message's address in Slack's own app: the documented
/// `slack://channel?team={team}&id={conversation}`, with the message named by its `ts` (and a
/// reply's thread by its root's) as a permalink names them. Slack's docs name only the channel
/// form; the message parts ride along the same way and degrade to the channel where an app
/// ignores them. `None` when any part cannot be read — no team learned yet, no app address,
/// and the page falls back to the permalink. `#481 slack-app-deep-link`.
#[must_use]
pub fn slack_app_url(
    team: &str,
    conversation: &str,
    ts: &str,
    thread_root_ts: Option<&str>,
) -> Option<String> {
    if !crate::slack::ids::is_team_id(team) || !crate::slack::ids::is_conversation_id(conversation)
    {
        return None;
    }
    let ts = crate::slack::ids::canonical_ts(ts)?;
    if ts.replace('.', "").len() != 16 {
        return None;
    }
    let mut url = format!("slack://channel?team={team}&id={conversation}&message={ts}");
    if let Some(root) = thread_root_ts {
        let root = crate::slack::ids::canonical_ts(root)?;
        if root == ts || root.replace('.', "").len() != 16 {
            return None;
        }
        url.push_str(&format!("&thread_ts={root}"));
    }
    (url.len() <= MAX_URL_BYTES).then_some(url)
}

/// Whether `id` is a Discord snowflake: 17 to 20 digits, the lengths Discord mints. A shorter
/// run of digits is some other provider's numbering and forms no Discord address.
pub(crate) fn is_snowflake(id: &str) -> bool {
    (17..=20).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// A Discord message's address, `https://discord.com/channels/{guild}/{channel}/{message}`.
/// `None` unless every part is a snowflake: the caller holds no guild for most channels, and an
/// address with a guessed guild opens nothing.
#[must_use]
pub fn discord_message_url(guild: &str, channel: &str, message: &str) -> Option<String> {
    if !is_snowflake(guild) || !is_snowflake(channel) || !is_snowflake(message) {
        return None;
    }
    Some(format!(
        "https://discord.com/channels/{guild}/{channel}/{message}"
    ))
}

/// One path segment percent-encoded: the unreserved characters pass, everything else is escaped,
/// so a segment stays one segment whatever an id holds.
fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Whether `segment` can stand as one part of a Google Chat address at all: present, bounded,
/// and holding neither a separator nor anything that would have to be escaped to stay text.
fn chat_segment(segment: &str) -> Option<&str> {
    let segment = segment.trim();
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.len() > 128
        || segment.contains('/')
        || segment.chars().any(char::is_control)
    {
        return None;
    }
    Some(segment)
}

/// The remainder of a resource name after `kind/`: `spaces/AAA/threads/BBB` under `spaces`
/// gives `AAA/threads/BBB`, for the caller to read its own segment out of.
fn resource_id<'n>(name: &'n str, kind: &str) -> Option<&'n str> {
    name.strip_prefix(kind)?.strip_prefix('/')
}

/// The space a resource name or a channel id names: the one segment after `spaces/`, whether
/// the name runs on (`spaces/AAA/threads/BBB`, a channel narrowed to one thread) or not.
fn space_of(name: &str) -> Option<&str> {
    resource_id(name, "spaces").map(|rest| rest.split('/').next().unwrap_or(rest))
}

/// A Google Chat message's web-client address, from the resource names a bridge presents: its
/// channel is `spaces/{space}`, its thread `spaces/{space}/threads/{thread}`, and its own id
/// may be `spaces/{space}/messages/{thread}.{message}` (a system-assigned name) or the bridge's
/// own numbering. The module docs say which link each case gets. `None` where no space can be
/// read: the message did not come from Google Chat in any shape this knows.
#[must_use]
pub fn google_chat_url(
    channel_id: &str,
    message_id: &str,
    thread: Option<&MessageThread>,
) -> Option<String> {
    let from_message = resource_id(message_id, "spaces")
        .and_then(|rest| rest.split_once("/messages/"))
        .map(|(space, segment)| (space, Some(segment)));
    // The space the message was SERVED in and the one its own name claims must agree: parts
    // from two spaces form no link, rather than one into a space the reader is not looking at.
    let channel_space = space_of(channel_id).and_then(chat_segment);
    if let (Some(served), Some((claimed, _))) = (channel_space, from_message) {
        if chat_segment(claimed) != Some(served) {
            return None;
        }
    }
    let (space, segment) = match from_message {
        Some(found) => found,
        None => (channel_space?, None),
    };
    let space = chat_segment(space)?;
    let base = format!("https://chat.google.com/room/{}", encode_segment(space));
    // A channel narrowed to one thread (`spaces/{space}/threads/{thread}`) names the thread
    // its messages are in where a copy carries no thread record of its own.
    let scoped_thread = resource_id(channel_id, "spaces")
        .and_then(|rest| rest.strip_prefix(space)?.strip_prefix("/threads/"))
        .filter(|thread| !thread.contains('/'));
    let thread_id = thread
        .and_then(|thread| {
            resource_id(&thread.id, "spaces")
                .and_then(|rest| rest.strip_prefix(space)?.strip_prefix("/threads/"))
                .filter(|thread| !thread.contains('/'))
                .or_else(|| (!thread.id.contains('/')).then_some(thread.id.as_str()))
        })
        .or(scoped_thread);
    let thread_link = |thread: &str| format!("{base}/{}", encode_segment(thread));
    let url = match (
        segment.and_then(chat_segment),
        thread_id.and_then(chat_segment),
    ) {
        (Some(segment), Some(thread)) => match segment.strip_prefix(thread) {
            // The system-assigned shape, `{thread}.{message}`, root included (`{thread}.{thread}`).
            Some(rest) => match rest.strip_prefix('.').and_then(chat_segment) {
                Some(message) => format!("{}/{}", thread_link(thread), encode_segment(message)),
                None if rest.is_empty() => thread_link(thread),
                None => thread_link(thread),
            },
            // A message id of the bridge's own, in a known thread: the thread's link.
            None => thread_link(thread),
        },
        (Some(segment), None) => match segment.split_once('.') {
            Some((thread, message)) => match (chat_segment(thread), chat_segment(message)) {
                (Some(thread), Some(message)) => {
                    format!("{}/{}", thread_link(thread), encode_segment(message))
                }
                _ => base,
            },
            None => base,
        },
        (None, Some(thread)) => thread_link(thread),
        (None, None) => base,
    };
    (url.len() <= MAX_URL_BYTES).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MessageId;

    fn a_thread(id: &str, is_root: bool) -> MessageThread {
        MessageThread {
            hue: None,
            id: id.to_owned(),
            root_message_id: Some(MessageId("1".to_owned())),
            is_root,
            reply_count: None,
            reply_count_exact: false,
        }
    }

    #[test]
    fn a_slack_channel_message_links_to_its_permalink() {
        // `#408 copy-upstream-link`.
        assert_eq!(
            slack_permalink(
                "https://acme.slack.com/",
                "C0123ABCD",
                "1700000000.000100",
                None
            ),
            Some("https://acme.slack.com/archives/C0123ABCD/p1700000000000100".to_owned())
        );
    }

    #[test]
    fn a_slack_thread_reply_links_into_its_thread() {
        assert_eq!(
            slack_permalink(
                "https://acme.slack.com",
                "C0123ABCD",
                "1700000060.000200",
                Some("1700000000.000100")
            ),
            Some(
                "https://acme.slack.com/archives/C0123ABCD/p1700000060000200\
                 ?thread_ts=1700000000.000100&cid=C0123ABCD"
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_slack_app_link_names_the_team_channel_and_message() {
        assert_eq!(
            slack_app_url("T0123ABCD", "C0123ABCD", "1700000060.000200", None),
            Some(
                "slack://channel?team=T0123ABCD&id=C0123ABCD&message=1700000060.000200".to_owned()
            )
        );
        assert_eq!(
            slack_app_url(
                "T0123ABCD",
                "C0123ABCD",
                "1700000060.000200",
                Some("1700000000.000100")
            ),
            Some(
                "slack://channel?team=T0123ABCD&id=C0123ABCD&message=1700000060.000200\
                 &thread_ts=1700000000.000100"
                    .to_owned()
            )
        );
        // No team of Slack's shape, no conversation, no readable ts: no app address.
        assert_eq!(
            slack_app_url("acme", "C0123ABCD", "1700000060.000200", None),
            None
        );
        assert_eq!(
            slack_app_url("T0123ABCD", "general", "1700000060.000200", None),
            None
        );
        assert_eq!(slack_app_url("T0123ABCD", "C0123ABCD", "soon", None), None);
        assert_eq!(
            slack_app_url(
                "T0123ABCD",
                "C0123ABCD",
                "1700000060.000200",
                Some("1700000060.000200")
            ),
            None
        );
    }

    #[test]
    fn slack_parts_that_cannot_be_read_form_no_link() {
        let good = "1700000000.000100";
        // No workspace, and no workspace that is not an https origin.
        assert_eq!(slack_permalink("", "C0123ABCD", good, None), None);
        assert_eq!(
            slack_permalink("http://acme.slack.com", "C0123ABCD", good, None),
            None
        );
        assert_eq!(
            slack_permalink("acme.slack.com", "C0123ABCD", good, None),
            None
        );
        // No conversation of Slack's shape, and no readable ts.
        assert_eq!(
            slack_permalink("https://acme.slack.com", "general", good, None),
            None
        );
        assert_eq!(
            slack_permalink("https://acme.slack.com", "C0123ABCD", "soon", None),
            None
        );
        // A "reply" in its own thread is a root mislabelled: no link rather than a wrong one.
        assert_eq!(
            slack_permalink("https://acme.slack.com", "C0123ABCD", good, Some(good)),
            None
        );
        // A query smuggled in a ts is refused with the ts, never carried into the address.
        assert_eq!(
            slack_permalink(
                "https://acme.slack.com",
                "C0123ABCD",
                "1700000000.000100?x=1",
                None
            ),
            None
        );
        // A ts no permalink segment holds (eleven second digits) is no shape Slack resolves.
        assert_eq!(
            slack_permalink(
                "https://acme.slack.com",
                "C0123ABCD",
                "17000000000.000100",
                None
            ),
            None
        );
    }

    #[test]
    fn a_discord_message_links_by_guild_channel_and_message() {
        assert_eq!(
            discord_message_url(
                "900000000000000001",
                "900000000000000002",
                "900000000000000003"
            ),
            Some(
                "https://discord.com/channels/900000000000000001/900000000000000002/900000000000000003"
                    .to_owned()
            )
        );
    }

    #[test]
    fn discord_parts_that_are_not_snowflakes_form_no_link() {
        let snow = "900000000000000001";
        assert_eq!(discord_message_url("", snow, snow), None, "no guild known");
        assert_eq!(discord_message_url("@me", snow, snow), None);
        assert_eq!(discord_message_url(snow, "spaces/A", snow), None);
        assert_eq!(discord_message_url(snow, snow, "42"), None, "too short");
    }

    #[test]
    fn a_google_chat_message_links_by_space_thread_and_message() {
        // The system-assigned resource name, `spaces/{space}/messages/{thread}.{message}`.
        let thread = a_thread("spaces/AAAa1/threads/BBBb2", false);
        assert_eq!(
            google_chat_url(
                "spaces/AAAa1",
                "spaces/AAAa1/messages/BBBb2.CCCc3",
                Some(&thread)
            ),
            Some("https://chat.google.com/room/AAAa1/BBBb2/CCCc3".to_owned())
        );
        // The same name with no thread record on the copy: the name itself carries both parts.
        assert_eq!(
            google_chat_url("spaces/AAAa1", "spaces/AAAa1/messages/BBBb2.CCCc3", None),
            Some("https://chat.google.com/room/AAAa1/BBBb2/CCCc3".to_owned())
        );
        // A thread's root: its name is `{thread}.{thread}`.
        let root = a_thread("spaces/AAAa1/threads/BBBb2", true);
        assert_eq!(
            google_chat_url(
                "spaces/AAAa1",
                "spaces/AAAa1/messages/BBBb2.BBBb2",
                Some(&root)
            ),
            Some("https://chat.google.com/room/AAAa1/BBBb2/BBBb2".to_owned())
        );
    }

    #[test]
    fn a_google_chat_message_without_a_readable_message_part_links_to_its_thread_or_space() {
        // A bridge's own message numbering (a snowflake), in a known thread: the thread's link.
        let thread = a_thread("spaces/AAAa1/threads/BBBb2", false);
        assert_eq!(
            google_chat_url("spaces/AAAa1", "900000000000000003", Some(&thread)),
            Some("https://chat.google.com/room/AAAa1/BBBb2".to_owned())
        );
        // A resource name whose message part has no thread in it, and no thread record.
        assert_eq!(
            google_chat_url("spaces/AAAa1", "spaces/AAAa1/messages/custom-name", None),
            Some("https://chat.google.com/room/AAAa1".to_owned())
        );
        // Nothing but the space known.
        assert_eq!(
            google_chat_url("spaces/AAAa1", "900000000000000003", None),
            Some("https://chat.google.com/room/AAAa1".to_owned())
        );
    }

    #[test]
    fn google_chat_parts_that_cannot_be_read_form_no_link() {
        // No space in either id: not a Google Chat message in any known shape.
        assert_eq!(
            google_chat_url("900000000000000002", "900000000000000003", None),
            None
        );
        assert_eq!(google_chat_url("spaces/", "900000000000000003", None), None);
        // A segment that would escape its place is encoded into it, never obeyed.
        let made = google_chat_url("spaces/AAAa1", "spaces/AAAa1/messages/BB?x=1.CC#y", None)
            .expect("a link");
        assert!(!made.contains('?') && !made.contains('#'), "{made}");
        // A thread of another space is not this message's thread.
        let other = a_thread("spaces/ZZZz9/threads/BBBb2", false);
        assert_eq!(
            google_chat_url("spaces/AAAa1", "900000000000000003", Some(&other)),
            Some("https://chat.google.com/room/AAAa1".to_owned())
        );
        // A message name claiming another space than the channel it was served in: parts from
        // two spaces form no link at all.
        assert_eq!(
            google_chat_url("spaces/AAAa1", "spaces/ZZZz9/messages/BBBb2.CCCc3", None),
            None
        );
        // A `..` segment never escapes its place: refused as a segment, so the link degrades
        // to the space's rather than resolving out of the room.
        let dotted = a_thread("spaces/AAAa1/threads/..", false);
        let made = google_chat_url("spaces/AAAa1", "900000000000000003", Some(&dotted))
            .expect("the space's link");
        assert_eq!(made, "https://chat.google.com/room/AAAa1");
        // A channel narrowed to one thread names the thread its messages are in.
        assert_eq!(
            google_chat_url("spaces/AAAa1/threads/BBBb2", "900000000000000003", None),
            Some("https://chat.google.com/room/AAAa1/BBBb2".to_owned())
        );
    }
}
