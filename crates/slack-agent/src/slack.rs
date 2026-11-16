//! `slack` — Socket Mode in, messages out.
//!
//! ## Why Socket Mode, restated because it is the security shape of the service
//!
//! Socket Mode is an **outbound** WebSocket to Slack: no listener, no port, no
//! vhost, no URL-verification endpoint, nothing for the front door or the router's
//! firewall to know about. The bot is reachable by nobody. A Slack-hosted Events
//! API endpoint would need an inbound HTTPS listener with a certificate and a row
//! in `common/front-door.nix`, which is a different service and a bigger hole.
//!
//! ## Three things about Slack's API that this file exists to get right
//!
//! 1. **`HTTP 200` does not mean it worked.** The Web API answers a bad call with
//!    `200 {"ok":false,"error":"not_authed"}`. Any client that checks the status
//!    code alone reports success forever. Hence [`Slack::call`], the only place a
//!    Web API response is read, which checks `ok` and returns the `error` string.
//! 2. **An event must be acknowledged**, by `envelope_id`, promptly — a
//!    `events_api` frame that is not acked is redelivered, and redelivered means a
//!    second answer to one question. So the ack goes out *before* the model is
//!    called, and [`seen`] backstops it by `event_id` for the case where Slack
//!    retries a delivery whose ack got lost.
//! 3. **This app sees its own output and other bots'.** Measured in the live
//!    journal on 2026-11-16 (see `docs/slack-agent-2026-11-16.md` §1): the app
//!    receives `message` events uploaded by a bot in a channel it watches. Answer
//!    anything with a `bot_id` or a `subtype` and you have written a feedback loop
//!    into a busy channel. [`parse_event`] drops both, unconditionally.
//!
//! ## The quiet-socket hazard
//!
//! A WebSocket that has been silently dropped by a NAT table reads as an endless
//! blocking read: connected, healthy-looking, and permanently deaf. That is this
//! fleet's known failure shape ("a channel can die while still answering"), so the
//! loop keeps a heartbeat stamp and a watchdog thread that **force-closes** the
//! socket when nothing has arrived for [`STALL_SECS`] — which makes the blocked
//! read return, and the reconnect path run.

use common_json::{self as json, Json};
use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Silence that counts as deafness is `--max-silence-secs` (the unit sets it), not
/// a constant here: the right number depends on how quiet the allow-listed channels
/// are, and the one failure mode to avoid is a bot that restarts itself on a
/// holiday weekend because nobody said anything.
/// How many processed `event_id`s to remember for redelivery dedup.
const SEEN_CAPACITY: usize = 256;

pub struct Slack {
    bot_token: String,
    app_token: String,
    client: reqwest::blocking::Client,
    /// `--dry-run`: read everything, post nothing. This is what makes the Slack leg
    /// rehearseable — the real events flow, the decision log prints, and no member
    /// of the workspace can tell the difference.
    pub dry: bool,
    /// Our own user id, from `auth.test`, so `<@U…>` can be stripped from the text.
    pub user_id: String,
}

/// Something worth answering.
#[derive(Debug, Clone, PartialEq)]
pub struct Mention {
    pub channel: String,
    pub ts: String,
    /// The thread to answer into, if the question was inside one.
    pub thread_ts: Option<String>,
    pub user: String,
    pub text: String,
    pub is_dm: bool,
    /// True for an `app_mention` — i.e. someone deliberately asked us. The
    /// allow-list answers "where may I speak unprompted"; a mention is always
    /// allowed, and conflating the two is how a bot starts talking in `#general`.
    pub is_mention: bool,
    pub event_id: Option<String>,
}

impl Slack {
    pub fn new(bot_token: String, app_token: String, dry: bool) -> Result<Slack, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("slack-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("cannot build the HTTP client: {e}"))?;
        let mut slack = Slack {
            bot_token,
            app_token,
            client,
            dry,
            user_id: String::new(),
        };
        // `auth.test` is the cheapest way to know who we are, and it fails loudly at
        // startup on a wrong or revoked token rather than at the first question.
        let me = slack.call("auth.test", &json::object(Vec::new()))?;
        slack.user_id = me
            .get("user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(slack)
    }

    /// One Web API call. The only place a response is interpreted — see the module
    /// header for why `ok` and not the status code is the signal.
    fn call(&self, method: &str, body: &Json) -> Result<Json, String> {
        let url = format!("https://slack.com/api/{method}");
        let payload = json::emit(body, 0);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .header("Content-Type", "application/json; charset=utf-8")
            .body(payload)
            .send()
            .map_err(|e| format!("{method}: {e}"))?;
        let status = resp.status();
        let text = resp.text().map_err(|e| format!("{method}: reading the reply: {e}"))?;
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(format!("{method}: rate limited"));
        }
        if !status.is_success() {
            return Err(format!("{method}: HTTP {status}: {:.200}", text));
        }
        let doc = json::parse(&text)
            .map_err(|e| format!("{method}: reply is not JSON ({e}): {:.200}", text))?;
        if doc.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return Err(format!(
                "{method}: {}",
                doc.get("error").and_then(|v| v.as_str()).unwrap_or("(no error given)")
            ));
        }
        Ok(doc)
    }

    /// `apps.connections.open` — the one call made with the **app-level** token: it
    /// returns the WSS URL and its one-time ticket.
    pub fn open_socket(&self) -> Result<(WebSocket<MaybeTlsStream<TcpStream>>, String), String> {
        let resp = self
            .client
            .post("https://slack.com/api/apps.connections.open")
            .header("Authorization", format!("Bearer {}", self.app_token))
            .header("Content-Type", "application/json; charset=utf-8")
            .body("{}")
            .send()
            .map_err(|e| format!("apps.connections.open: {e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| format!("apps.connections.open: reading the reply: {e}"))?;
        if !status.is_success() {
            return Err(format!("apps.connections.open: HTTP {status}: {:.200}", text));
        }
        let doc = json::parse(&text).map_err(|e| format!("apps.connections.open: not JSON ({e})"))?;
        if doc.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return Err(format!(
                "apps.connections.open: {}",
                doc.get("error").and_then(|v| v.as_str()).unwrap_or("(no error given)")
            ));
        }
        let url = doc
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or("apps.connections.open returned no `url` — is Socket Mode enabled on the app?")?
            .to_string();
        let (ws, _resp) = tungstenite::connect(url.as_str())
            .map_err(|e| format!("connecting to Slack's socket: {e}"))?;
        Ok((ws, url))
    }

    pub fn post(&self, channel: &str, thread_ts: Option<&str>, text: &str) -> Result<(), String> {
        if self.dry {
            println!("DRY-RUN would post to {channel}{}:\n{text}", thread_ts.map(|t| format!(" thread {t}")).unwrap_or_default());
            return Ok(());
        }
        let mut pairs = vec![
            ("channel".to_string(), json::string(channel)),
            ("text".to_string(), json::string(text)),
            // Link unfurling off: the tool outputs contain URLs we do not want
            // expanded into five-line previews in a busy channel.
            ("unfurl_links".to_string(), json::boolean(false)),
        ];
        if let Some(t) = thread_ts {
            pairs.push(("thread_ts".to_string(), json::string(t)));
        }
        self.call("chat.postMessage", &json::object(pairs)).map(|_| ())
    }

    /// React first. At ~32 tok/s decode a real answer is 10–30 s away, and an
    /// `:eyes:` on the question is what makes that wait read as "working" instead
    /// of "ignored me". A failure here is not worth reporting: a reaction is the
    /// politest thing in this file.
    pub fn react(&self, channel: &str, ts: &str, emoji: &str) {
        if self.dry {
            println!("DRY-RUN would react {emoji} to {channel}/{ts}");
            return;
        }
        let _ = self.call(
            "reactions.add",
            &json::object(vec![
                ("channel".to_string(), json::string(channel)),
                ("timestamp".to_string(), json::string(ts)),
                ("name".to_string(), json::string(emoji)),
            ]),
        );
    }

    pub fn done_reacting(&self, channel: &str, ts: &str, emoji: &str) {
        if self.dry {
            return;
        }
        let _ = self.call(
            "reactions.remove",
            &json::object(vec![
                ("channel".to_string(), json::string(channel)),
                ("timestamp".to_string(), json::string(ts)),
                ("name".to_string(), json::string(emoji)),
            ]),
        );
    }

    /// Channel names → ids, for an allow-list written in names a human recognises.
    /// A failure is not fatal: the caller falls back to matching ids only, and says
    /// so at startup.
    pub fn channel_ids(&self) -> Result<std::collections::BTreeMap<String, String>, String> {
        let mut out = std::collections::BTreeMap::new();
        let mut cursor: Option<String> = None;
        for _ in 0..10 {
            let mut pairs = vec![
                ("limit".to_string(), json::number_from_int(500)),
                ("exclude_archived".to_string(), json::boolean(true)),
                ("types".to_string(), json::string("public_channel,private_channel")),
            ];
            if let Some(c) = &cursor {
                pairs.push(("cursor".to_string(), json::string(c.clone())));
            }
            let doc = self.call("conversations.list", &json::object(pairs))?;
            if let Some(channels) = doc.get("channels").and_then(|c| c.as_array()) {
                for c in channels {
                    if let (Some(name), Some(id)) = (
                        c.get("name").and_then(|v| v.as_str()),
                        c.get("id").and_then(|v| v.as_str()),
                    ) {
                        out.insert(name.to_string(), id.to_string());
                    }
                }
            }
            cursor = doc
                .at(&["response_metadata", "next_cursor"])
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }
}

/// The ack frame. Sent before any work starts, for the reason in the header.
pub fn ack(envelope_id: &str) -> String {
    json::emit(
        &json::object(vec![("envelope_id".to_string(), json::string(envelope_id))]),
        0,
    )
}

/// Decide whether a socket frame is a question, and if so what the question is.
///
/// Returns `Ok(None)` for anything to ignore, and `Err(())` for a frame that is not
/// JSON at all — which the caller logs once rather than silently swallowing,
/// because a protocol we cannot read is not the same as a message we do not want.
pub fn parse_event(frame: &str, bot_user_id: &str) -> Result<Option<Mention>, ()> {
    let doc = json::parse(frame).map_err(|_| ())?;
    if doc.get("type").and_then(|t| t.as_str()) != Some("events_api") {
        return Ok(None);
    }
    let Some(event) = doc.get("payload").and_then(|p| p.get("event")) else {
        return Ok(None);
    };
    let kind = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    // Bots and message *subtypes* are the loop guard. `subtype` covers
    // `bot_message`, `file_share` (how the camera alerts arrive), `thread_broadcast`,
    // `message_changed`, `channel_join`, … — none of which is a question.
    if event.get("bot_id").is_some() || event.get("subtype").is_some() {
        return Ok(None);
    }
    let channel = event.get("channel").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let ts = event.get("ts").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if channel.is_empty() || ts.is_empty() {
        return Ok(None);
    }
    let is_dm = event.get("channel_type").and_then(|v| v.as_str()) == Some("im");
    let wanted = match kind {
        "app_mention" => true,
        // A DM is addressed to us by definition. `channel_type` is only present on
        // `message` events, which is why the mention case does not look at it.
        "message" => is_dm,
        _ => false,
    };
    if !wanted {
        return Ok(None);
    }
    let raw = event.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    // "<@U05H…> is it cold in the hackspace" → the question.
    let text = strip_mention(&raw, bot_user_id);
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(Mention {
        thread_ts: event
            .get("thread_ts")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        user: event.get("user").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        text,
        is_dm,
        is_mention: kind == "app_mention",
        event_id: doc
            .get("payload")
            .and_then(|p| p.get("event_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        channel,
        ts,
    }))
}

/// Remove every `<@id>` from a message, and the bot's name if someone typed it
/// instead of @-mentioning.
pub fn strip_mention(text: &str, bot_user_id: &str) -> String {
    let mut out = text.to_string();
    if !bot_user_id.is_empty() {
        out = out.replace(&format!("<@{bot_user_id}>"), " ");
    }
    // Any `<!…>` control token would ping people if it ever came back out of a
    // model, so all of them go, not just the two common ones.
    while let Some(start) = out.find("<!") {
        match out[start + 2..].find('>') {
            Some(end) => out = format!("{} {}", &out[..start], &out[start + 2 + end + 1..]),
            None => break,
        }
    }
    // Keep any `<#C0…|name>` channel reference readable rather than as an id.
    while let Some(start) = out.find("<#") {
        let rest = &out[start + 2..];
        match rest.find('>') {
            Some(end) => {
                let inner = &rest[..end];
                let label = inner.split('|').nth(1).unwrap_or("channel");
                out = format!("{} {label} {}", &out[..start], &out[start + 2 + end + 1..]);
            }
            None => break,
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where an answer goes, and whether it names the person who asked.
///
/// Slack gives a bot **no way to make a client show a thread**. "Expand the thread" is not an
/// API: the reply is either in the channel, where it is visible in the scrollback, or in a
/// thread, where it is a badge on a message and many clients do not open it. So a question
/// asked *at* a channel is answered *in* the channel — the way `vulcan-bot` did it with
/// `msg.reply()` — and it names the asker, because a bare list of temperatures in a busy room
/// does not obviously belong to anyone.
///
/// A question asked *inside* a thread is different: there the thread is the conversation, and
/// replying beside it would abandon the people already reading it. That one stays in the
/// thread and needs no mention, since everyone in the thread can see it.
///
/// `follow_threads` is the operator's override (`--no-thread-replies`) for a deployment that
/// wants everything in the channel regardless.
pub fn reply_target(
    question_in_thread: Option<&str>,
    follow_threads: bool,
    asker: &str,
) -> (Option<String>, Option<String>) {
    match (follow_threads, question_in_thread) {
        (true, Some(ts)) => (Some(ts.to_string()), None),
        (true, None) => (None, Some(format!("<@{asker}>"))),
        // Told not to use threads at all: say it in the channel, and still tell the asker
        // which message is theirs.
        (false, _) => (None, Some(format!("<@{asker}>"))),
    }
}

/// Escape the three characters Slack treats as markup. Anything else in our own
/// tool output (temperatures, entity ids, `*`s from a Prometheus label set) is
/// passed through, which is what makes `@here` in a device name fail safe rather
/// than ping the workspace.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A bounded set of processed `event_id`s: Slack redelivers an un-acked event, and
/// two answers to one question is the failure a member would notice first.
#[derive(Default)]
pub struct Seen {
    order: VecDeque<String>,
    ids: HashSet<String>,
}

impl Seen {
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    pub fn insert(&mut self, id: String) {
        if self.ids.insert(id.clone()) {
            self.order.push_back(id);
        }
        while self.order.len() > SEEN_CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
    }
}

/// The receive loop.
///
/// Yields every frame Slack sends, including the ones [`parse_event`] ignores, so
/// the caller can log protocol-level things (`hello`, `disconnect`) rather than
/// having them vanish. `last_frame` is stamped on *every* frame including pings,
/// which is what makes "deaf but still connected" a number rather than a rumour.
///
/// `on_frame` runs **inline on this thread**, so a twenty-second model call stops
/// the reading of new frames for its duration. That is chosen, not tolerated: the
/// engine has one slot and ~32 tok/s of decode, so serialising turns is what we
/// want anyway, and Slack redelivers the queued events (which [`Seen`] then
/// dedups). It also keeps the reconnect path a plain loop with no second thread
/// owning half the socket.
pub fn frame_loop(
    mut ws: WebSocket<MaybeTlsStream<TcpStream>>,
    last_frame: &AtomicU64,
    mut on_frame: impl FnMut(&str, &mut dyn FnMut(String) -> Result<(), String>) -> Result<(), String>,
) -> Result<(), String> {
    last_frame.store(now_secs(), Ordering::Relaxed);
    loop {
        // The read is finished before the match, so the handler can be handed
        // `&mut ws`: the ack has to go out *before* the model call that follows it,
        // and a closure created outside the match would be holding the socket
        // borrow across the read that produces the message it is answering.
        let message = match ws.read() {
            Ok(m) => m,
            Err(e) => return Err(format!("socket read: {e}")),
        };
        match message {
            Message::Text(text) => {
                last_frame.store(now_secs(), Ordering::Relaxed);
                let mut send = |frame: String| -> Result<(), String> {
                    // Written as two steps rather than one `and_then`: the library's
                    // error type is large, and clippy is right that a failed `flush`
                    // should not be reported as a "socket write".
                    ws.write(Message::Text(frame.into()))
                        .map_err(|e| format!("socket write: {e}"))?;
                    ws.flush().map_err(|e| format!("socket flush: {e}"))
                };
                on_frame(&text, &mut send)?;
            }
            Message::Binary(_) => {
                last_frame.store(now_secs(), Ordering::Relaxed);
                return Err("Slack sent a binary frame, which Socket Mode does not use".to_string());
            }
            // A ping is answered by tungstenite on the next read or write; the stamp
            // is the only thing we take from it, and it is the whole liveness signal.
            Message::Ping(_) | Message::Pong(_) => {
                last_frame.store(now_secs(), Ordering::Relaxed);
            }
            Message::Close(frame) => {
                let why = frame
                    .as_ref()
                    .map(|f| format!(" code {} reason {:?}", f.code, f.reason))
                    .unwrap_or_default();
                return Err(format!("Slack closed the socket{why}"));
            }
            _ => {}
        }
    }
}

/// Watch the clock, and end the process when the socket has heard nothing for too
/// long.
///
/// **Why `exit` and not a reconnect:** the read is blocking, and the honest ways to
/// interrupt it are a read timeout (which would mean reaching through
/// `MaybeTlsStream::Rustls` into rustls' own stream type — a coupling to two
/// upstreams' internals for one line of convenience) or closing the socket from
/// another thread (which needs the `&mut WebSocket` the blocked read is holding).
/// The third option is to stop pretending to be clever: the process exits with a
/// distinctive status, the journal says why, and
/// `systemd.services.slack-agent.serviceConfig.Restart=always` dials a brand-new
/// socket seconds later — which is exactly the never-give-up pattern every other
/// unit on this box uses, and it also covers the subtler failure where the socket is
/// fine and Slack has simply stopped delivering to it.
///
/// The alternative to exiting is going unnoticed, and this fleet has already been
/// bitten by a channel that died while still looking alive
/// (`docs/nix-dev-workflow.md`).
pub fn watch_stall(last_frame: Arc<AtomicU64>, stall: Duration, exit_code: i32) -> thread::JoinHandle<()> {
    let check = Duration::from_secs(30).min(stall);
    thread::spawn(move || loop {
        thread::sleep(check);
        let stamp = last_frame.load(Ordering::Relaxed);
        if stamp == 0 {
            // Not started yet, or just reconnected: nothing to be stale about.
            last_frame.store(now_secs(), Ordering::Relaxed);
            continue;
        }
        let silent = now_secs().saturating_sub(stamp);
        if silent > stall.as_secs() {
            eprintln!(
                "socket: {silent}s without a single frame from Slack — exiting {exit_code} so \
                 supervision reconnects (the socket is connected; it is simply not hearing anything)"
            );
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
    })
}

/// Seconds since the last frame, for `/metrics` and for the health line.
pub fn silence_secs(last_frame: &AtomicU64) -> u64 {
    now_secs().saturating_sub(last_frame.load(Ordering::Relaxed))
}

/// Whole seconds since the epoch, for the liveness stamps.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Reconnect pacing: 2 s, 4 s, 8 s … capped, so a Slack outage or a revoked token
/// costs the log one line per backoff rather than a hot loop.
pub struct Backoff {
    attempt: u32,
}

impl Backoff {
    pub fn new() -> Self {
        Backoff { attempt: 0 }
    }
    pub fn wait(&mut self) -> Duration {
        self.attempt += 1;
        const STEPS: [u64; 6] = [2, 4, 8, 16, 32, 60];
        Duration::from_secs(STEPS[(self.attempt as usize - 1).min(STEPS.len() - 1)])
    }
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bot_or_a_typed_message_is_never_a_question() {
        // The real shape from the live journal: a camera alert uploaded by a bot.
        let frame = r#"{"type":"events_api","envelope_id":"e1","payload":{"event_id":"Ev1","event":{
            "type":"message","text":"Event - person detected on camera main_space","subtype":"file_share",
            "bot_id":"B05HF8YAHUN","channel":"C09KCS3EGGN","ts":"1794794694.851079"}}}"#;
        assert_eq!(parse_event(frame, "U05H7CJGBD5").unwrap().map(|m| m.text), None);
    }

    #[test]
    fn a_mention_becomes_the_question_without_the_address() {
        let frame = r#"{"type":"events_api","envelope_id":"e2","payload":{"event_id":"Ev2","event":{
            "type":"app_mention","text":"<@U05H7CJGBD5>   is it cold in the hackspace?",
            "user":"U01ABC","channel":"C0GENERAL","ts":"1794.1"}}}"#;
        let m = parse_event(frame, "U05H7CJGBD5").unwrap().expect("a mention");
        assert_eq!(m.text, "is it cold in the hackspace?");
        assert_eq!(m.channel, "C0GENERAL");
        assert_eq!(m.ts, "1794.1");
        assert_eq!(m.thread_ts, None);
        assert!(!m.is_dm);
        assert!(m.is_mention, "an app_mention must be flagged as one");
        assert_eq!(m.event_id.as_deref(), Some("Ev2"));
    }

    #[test]
    fn a_direct_message_is_addressed_without_a_mention_and_a_thread_stays_a_thread() {
        let frame = r#"{"type":"events_api","envelope_id":"e3","payload":{"event_id":"Ev3","event":{
            "type":"message","text":"is the hackspace open","user":"U01ABC","channel":"D01",
            "channel_type":"im","ts":"1794.2","thread_ts":"1794.11"}}}"#;
        let m = parse_event(frame, "U05H7CJGBD5").unwrap().expect("a DM");
        assert!(m.is_dm);
        assert!(!m.is_mention, "a DM is addressed but not a mention");
        assert_eq!(m.text, "is the hackspace open");
        assert_eq!(m.thread_ts.as_deref(), Some("1794.11"));
        // The same text in a *channel* without a mention is not for us.
        let frame = frame.replace("\"channel_type\":\"im\",", "").replace("\"D01\"", "\"C0X\"");
        assert!(parse_event(&frame, "U05H7CJGBD5").unwrap().is_none());
    }

    #[test]
    fn a_bare_mention_is_not_a_question() {
        let frame = r#"{"type":"events_api","envelope_id":"e4","payload":{"event_id":"Ev4","event":{
            "type":"app_mention","text":"<@U05H7CJGBD5>","user":"U01ABC","channel":"C0X","ts":"1794.3"}}}"#;
        assert!(parse_event(frame, "U05H7CJGBD5").unwrap().is_none());
    }

    #[test]
    fn non_event_frames_are_ignored_rather_than_answered() {
        assert!(parse_event(r#"{"type":"hello","connection_info":{}}"#, "U1").unwrap().is_none());
        assert!(parse_event(r#"{"type":"disconnect","reason":"reload"}"#, "U1").unwrap().is_none());
        assert!(parse_event("not json", "U1").is_err());
    }

    #[test]
    fn an_ack_is_just_the_envelope_id_back() {
        assert_eq!(ack("abc"), r#"{"envelope_id":"abc"}"#);
    }

    #[test]
    fn markup_that_pings_the_workspace_is_neutralised() {
        assert_eq!(escape("a < b & c > d"), "a &lt; b &amp; c &gt; d");
        // `@here`, `@channel`, `@everyone` must never survive a round trip through
        // the model: echoing one back would ping the whole workspace.
        assert_eq!(strip_mention("<!here> what's up", "U1"), "what's up");
        assert_eq!(strip_mention("<!everyone> <!channel> party", "U1"), "party");
    }

    #[test]
    fn a_stripped_mention_keeps_the_rest_of_the_sentence() {
        assert_eq!(strip_mention("<@U1> <#C0123|general> is busy?", "U1"), "general is busy?");
        assert_eq!(strip_mention("no ids here", "U1"), "no ids here");
    }

    #[test]
    fn a_channel_question_is_answered_in_the_channel_and_names_the_asker() {
        // The complaint this answers: "auto expand the reply? otherwise do old style and
        // just add reply in channel with requesting user's name mentioned." There is no
        // expand; so it is the channel, with the mention.
        let (thread, mention) = reply_target(None, true, "U05H7CJGBD5");
        assert_eq!(thread, None, "a root message must not spawn a thread");
        assert_eq!(mention.as_deref(), Some("<@U05H7CJGBD5>"));
    }

    #[test]
    fn a_threaded_question_stays_in_its_thread() {
        let (thread, mention) = reply_target(Some("1794833049.446659"), true, "U05H7CJGBD5");
        assert_eq!(thread.as_deref(), Some("1794833049.446659"));
        assert_eq!(mention, None, "everyone in the thread can already see it");
    }

    #[test]
    fn the_operator_can_send_everything_to_the_channel() {
        let (thread, mention) = reply_target(Some("1794833049.446659"), false, "U1");
        assert_eq!(thread, None);
        assert_eq!(mention.as_deref(), Some("<@U1>"));
    }

    #[test]
    fn redelivered_events_are_answered_once() {
        let mut seen = Seen::default();
        assert!(!seen.contains("Ev1"));
        seen.insert("Ev1".to_string());
        assert!(seen.contains("Ev1"));
        // Bounded, so a long-running unit cannot grow its memory by being fed ids.
        for i in 0..(SEEN_CAPACITY * 3) {
            seen.insert(format!("Ev{i}"));
        }
        assert!(seen.order.len() <= SEEN_CAPACITY);
        assert!(!seen.contains("Ev0"), "the oldest id should have been forgotten");
    }

    #[test]
    fn reconnect_pacing_grows_and_is_capped() {
        let mut b = Backoff::new();
        let first = b.wait();
        assert_eq!(first, Duration::from_secs(2));
        // It grows, then plateaus at the cap: a day-long Slack outage must cost one
        // line in the journal per minute, not one per second and not one per hour.
        let mut last = first;
        for _ in 0..8 {
            let w = b.wait();
            assert!(w <= Duration::from_secs(60), "{w:?} exceeds the cap");
            assert!(w >= last, "pacing must not shrink: {last:?} -> {w:?}");
            last = w;
        }
        assert_eq!(last, Duration::from_secs(60));
        b.reset();
        assert_eq!(b.wait(), Duration::from_secs(2));
    }
}
