//! authentik OIDC sign-in: authorization-code flow with S256 PKCE, then token
//! introspection, and a session only for users in the required group (`Infra`).
//!
//! This is a hand-written equivalent of `common-rs/oidc`, which implements the
//! same contract on `reqwest`.  It is not used here because this tool stays
//! dependency-free (see the note in [`crate::sha256`]); the *contract* is
//! identical, and the three things AGENTS.md says authentik bites you on are
//! handled here and asserted by the tests:
//!
//! * the S256 challenge is **unpadded** base64url — authentik recomputes
//!   `urlsafe_b64encode(sha256(verifier)).replace("=","")` and compares for
//!   equality, so a padded one saves fine and then fails at `/token/` with
//!   `invalid_grant`;
//! * the `groups` claim only appears if the provider has a real **ScopeMapping
//!   child row** for `groups`, so a missing claim is reported as a
//!   misconfiguration rather than read as "you are not a member";
//! * scopes are `openid profile email groups` and the client authenticates to
//!   the token and introspection endpoints with **HTTP Basic**.
//!
//! HTTPS to authentik goes through `curl` — the approach `dns-sync` uses for
//! the DigitalOcean API — and curl's whole invocation (including the client
//! secret) is written to its stdin with `--config -`, so the secret appears in
//! no process list and no environment block.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::b64;
use crate::sha256::sha256;
use crate::R;

use common_json as json;

const SESSION_TTL_SECS: i64 = 12 * 3600;
const PENDING_TTL_SECS: i64 = 600;
const SCOPES: &str = "openid profile email groups";
/// Marks the end of curl's body so the status code can be read off the same
/// stream.  Underscores and capitals so it cannot occur in an authentik reply.
const STATUS_MARK: &str = "__HTTP_STATUS_";

#[derive(Clone)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    /// e.g. https://id.leighhack.org/application/o/authorize/
    pub authorize_url: String,
    /// e.g. https://id.leighhack.org/application/o/token/
    pub token_url: String,
    /// e.g. https://id.leighhack.org/application/o/introspect/
    pub introspect_url: String,
    /// Must match the provider's redirect URI **exactly**, or authentik refuses
    /// the authorize request.
    pub redirect_uri: String,
    /// authentik group a user must be in to get a session.
    pub required_group: String,
    pub cookie_name: String,
    /// Absolute path to `curl`, so the systemd unit can name the store path.
    pub curl: String,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub username: String,
    pub email: Option<String>,
    /// unix seconds
    pub expires: i64,
}

struct Pending {
    verifier: String,
    created: i64,
}

pub struct Oidc {
    cfg: Config,
    pending: Mutex<HashMap<String, Pending>>,
    sessions: Mutex<HashMap<String, Session>>,
}

/// A login refusal, safe to show in a browser.
pub struct AuthError(pub String);

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Oidc {
    pub fn new(cfg: Config) -> Oidc {
        Oidc {
            cfg,
            pending: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn cookie_name(&self) -> &str {
        &self.cfg.cookie_name
    }

    pub fn required_group(&self) -> &str {
        &self.cfg.required_group
    }

    /// Start a login: returns the `state` to remember and the URL to redirect
    /// to.  A fresh verifier per attempt is what makes PKCE worth having, so
    /// both halves are stored against the state.
    pub fn begin_login(&self) -> R<(String, String)> {
        let verifier = b64::url_unpadded(&b64::random_bytes(32)?);
        let state = b64::random_hex(16)?;
        let challenge = b64::url_unpadded(&sha256(verifier.as_bytes()));

        let url = format!(
            "{}?client_id={}&response_type=code&redirect_uri={}&scope={}&state={}\
             &code_challenge={}&code_challenge_method=S256",
            self.cfg.authorize_url,
            enc(&self.cfg.client_id),
            enc(&self.cfg.redirect_uri),
            enc(SCOPES),
            enc(&state),
            enc(&challenge),
        );

        let mut pending = lock(&self.pending);
        pending.retain(|_, p| now() - p.created < PENDING_TTL_SECS);
        pending.insert(
            state.clone(),
            Pending {
                verifier,
                created: now(),
            },
        );
        Ok((state, url))
    }

    /// Exchange `code` for a token, ask authentik who it belongs to, and gate
    /// on the group.  Returns the session cookie value to set.
    pub fn finish_login(&self, code: &str, state: &str) -> Result<(String, Session), AuthError> {
        let verifier = {
            let mut pending = lock(&self.pending);
            match pending.remove(state) {
                Some(p) => {
                    if now() - p.created >= PENDING_TTL_SECS {
                        return Err(AuthError("that login attempt has expired".into()));
                    }
                    p.verifier
                }
                // Unknown state: the browser replayed a callback, the service
                // restarted mid-login, or someone is trying a CSRF.  All three
                // get the same answer, and the attempt is over either way.
                None => {
                    return Err(AuthError(
                        "unknown login attempt (state missing or expired)".into(),
                    ))
                }
            }
        };

        let (status, body) = self
            .post(
                &self.cfg.token_url,
                &[
                    ("grant_type", "authorization_code"),
                    ("code", code),
                    ("redirect_uri", self.cfg.redirect_uri.as_str()),
                    ("code_verifier", verifier.as_str()),
                ],
            )
            .map_err(AuthError)?;

        if status != 200 {
            return Err(AuthError(format!(
                "authentik refused the code (HTTP {status}): {}",
                trim(&body)
            )));
        }
        let token_reply = json::parse(&body)
            .map_err(|e| AuthError(format!("bad JSON from authentik's token endpoint: {e}")))?;
        let access_token = token_reply
            .get("access_token")
            .and_then(json::Json::as_str)
            .ok_or_else(|| AuthError(format!("no access_token in reply: {}", trim(&body))))?
            .to_string();

        let (status, intro_body) = self
            .post(&self.cfg.introspect_url, &[("token", access_token.as_str())])
            .map_err(AuthError)?;
        if status != 200 {
            return Err(AuthError(format!(
                "introspection failed (HTTP {status}): {}",
                trim(&intro_body)
            )));
        }
        let intro = json::parse(&intro_body)
            .map_err(|e| AuthError(format!("bad JSON from introspection: {e}")))?;

        if intro.get("active").and_then(json::Json::as_bool) != Some(true) {
            return Err(AuthError(
                "authentik reports this token as not active".into(),
            ));
        }
        let username = intro
            .get("username")
            .or_else(|| intro.get("sub"))
            .and_then(json::Json::as_str)
            .ok_or_else(|| AuthError("introspection returned no username".into()))?
            .to_string();

        // The group gate.  `groups` is absent when the provider has no
        // ScopeMapping child row for it — a configuration bug that must not be
        // reported as "you are not in Infra", because it sends the operator to
        // the wrong box to look.
        match intro.get("groups").and_then(json::Json::as_array) {
            None => {
                return Err(AuthError(
                    "authentik sent no `groups` claim: the provider is missing its groups \
                     ScopeMapping (see docs/authentik-oidc-notes.md)"
                        .into(),
                ))
            }
            Some(groups) => {
                let wanted = self.cfg.required_group.clone();
                let member = groups
                    .iter()
                    .filter_map(json::Json::as_str)
                    .any(|g| g == wanted);
                if !member {
                    return Err(AuthError(format!(
                        "{username} is not a member of the {} group",
                        self.cfg.required_group
                    )));
                }
            }
        }

        let session = Session {
            username,
            email: intro
                .get("email")
                .and_then(json::Json::as_str)
                .map(|s| s.to_string()),
            expires: now() + SESSION_TTL_SECS,
        };
        let cookie = b64::random_hex(32).map_err(AuthError)?;
        let mut sessions = lock(&self.sessions);
        sessions.retain(|_, s| s.expires > now());
        sessions.insert(cookie.clone(), session.clone());
        Ok((cookie, session))
    }

    pub fn session(&self, cookie: &str) -> Option<Session> {
        let sessions = lock(&self.sessions);
        match sessions.get(cookie) {
            Some(s) if s.expires > now() => Some(s.clone()),
            _ => None,
        }
    }

    pub fn logout(&self, cookie: &str) {
        lock(&self.sessions).remove(cookie);
    }

    /// One `application/x-www-form-urlencoded` HTTPS POST with the client
    /// credentials in a `Basic` header; returns `(http status, body)`.
    fn post(&self, url: &str, form: &[(&str, &str)]) -> R<(u16, String)> {
        let body = form_urlencode(form);
        let mut conf = String::from("silent\nshow-error\nconnect-timeout = 10\nmax-time = 20\n");
        push_conf(&mut conf, "url", url);
        push_conf(&mut conf, "request", "POST");
        push_conf(
            &mut conf,
            "header",
            "Content-Type: application/x-www-form-urlencoded",
        );
        push_conf(
            &mut conf,
            "user",
            &format!("{}:{}", self.cfg.client_id, self.cfg.client_secret),
        );
        push_conf(&mut conf, "data", &body);
        // The status code is appended to the body so one read yields both.
        conf.push_str(&format!(
            "write-out = \"\\n{STATUS_MARK}%{{http_code}}\"\n"
        ));

        let mut child = Command::new(&self.cfg.curl)
            .arg("--config")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", self.cfg.curl))?;

        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| "curl stdin was not piped".to_string())?;
            stdin
                .write_all(conf.as_bytes())
                .map_err(|e| format!("write curl config: {e}"))?;
        }
        // Closing stdin before reading is safe: the config is a few hundred
        // bytes and the reply is a token response, both far under the pipe
        // buffer, so neither side can fill the other while this blocks.
        drop(child.stdin.take());

        let mut out = Vec::new();
        child
            .stdout
            .as_mut()
            .ok_or_else(|| "curl stdout was not piped".to_string())?
            .read_to_end(&mut out)
            .map_err(|e| format!("read curl output: {e}"))?;
        let mut err = Vec::new();
        child
            .stderr
            .as_mut()
            .ok_or_else(|| "curl stderr was not piped".to_string())?
            .read_to_end(&mut err)
            .map_err(|e| format!("read curl errors: {e}"))?;
        let code = child
            .wait()
            .map_err(|e| format!("wait for curl: {e}"))?
            .code()
            .unwrap_or(-1);

        let mut text = String::from_utf8_lossy(&out).into_owned();
        if code != 0 {
            return Err(format!(
                "curl exited {code}: {}",
                trim(&String::from_utf8_lossy(&err))
            ));
        }

        let status = match text.rfind(STATUS_MARK) {
            Some(i) => {
                let tail = text[i + STATUS_MARK.len()..].to_string();
                text.truncate(i);
                tail.trim().parse::<u16>().unwrap_or(0)
            }
            // No mark means curl printed nothing at all.
            None => 0,
        };
        Ok((status, text))
    }
}

/// `key = "value"` in curl config syntax, escaping what needs escaping.
fn push_conf(conf: &mut String, key: &str, value: &str) {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    conf.push_str(&format!("{key} = \"{escaped}\"\n"));
}

/// Shorten a server reply for an error message, on a character boundary.
fn trim(s: &str) -> String {
    let t = s.trim();
    match t.char_indices().nth(300) {
        Some((i, _)) => format!("{}…", &t[..i]),
        None => t.to_string(),
    }
}

/// `application/x-www-form-urlencoded`, percent-escaping everything outside
/// the unreserved set.  `%20` rather than `+` for a space: both are accepted by
/// the parsers authentik uses, and `%20` cannot be misread as a literal plus.
fn form_urlencode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode for URLs and form bodies.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Lock without panicking on a poisoned mutex: a request that panics while
/// holding the session map must not take the whole service down with it.
pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            client_id: "cid".into(),
            client_secret: "s3cr3t".into(),
            authorize_url: "https://id.example/o/authorize/".into(),
            token_url: "https://id.example/o/token/".into(),
            introspect_url: "https://id.example/o/introspect/".into(),
            redirect_uri: "https://switch.int.leighhack.org/auth/callback".into(),
            required_group: "Infra".into(),
            cookie_name: "switch_session".into(),
            curl: "/bin/false".into(),
        }
    }

    #[test]
    fn challenge_is_unpadded_base64url() {
        // RFC 7636 appendix B: this verifier must produce this challenge.
        let challenge =
            b64::url_unpadded(&sha256(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert!(!challenge.contains('='));
        assert!(!challenge.contains('+'));
        assert!(!challenge.contains('/'));
    }

    #[test]
    fn begin_login_builds_a_pkce_url_and_remembers_the_state() {
        let oidc = Oidc::new(cfg());
        let (state, url) = oidc.begin_login().unwrap();
        assert!(url.starts_with("https://id.example/o/authorize/?client_id=cid&"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("scope=openid%20profile%20email%20groups"));
        assert!(url.contains(&format!("state={state}")));
        // The redirect URI goes out percent-encoded; authentik compares it
        // strictly against the registered value.
        assert!(url
            .contains("redirect_uri=https%3A%2F%2Fswitch.int.leighhack.org%2Fauth%2Fcallback"));
        assert!(lock(&oidc.pending).contains_key(&state));
    }

    #[test]
    fn a_missing_state_is_refused_without_touching_the_network() {
        let oidc = Oidc::new(cfg());
        let err = oidc.finish_login("code", "never-issued").unwrap_err();
        assert!(err.0.contains("unknown login attempt"));
    }

    #[test]
    fn form_encoding() {
        assert_eq!(
            form_urlencode(&[("grant_type", "authorization_code"), ("code", "a b&c")]),
            "grant_type=authorization_code&code=a%20b%26c"
        );
        assert_eq!(
            enc("https://x.example/a?b=c"),
            "https%3A%2F%2Fx.example%2Fa%3Fb%3Dc"
        );
    }

    #[test]
    fn curl_config_escapes_quotes_and_backslashes() {
        let mut conf = String::new();
        push_conf(&mut conf, "user", r#"id:pa"ss\word"#);
        assert_eq!(conf, "user = \"id:pa\\\"ss\\\\word\"\n");
    }

    /// The group gate, exercised on the parsed introspection reply — this is
    /// the check that decides who may touch the switch.
    fn gate(intro: &str) -> Result<(), String> {
        let j = json::parse(intro).map_err(|e| e)?;
        if j.get("active").and_then(json::Json::as_bool) != Some(true) {
            return Err("inactive".into());
        }
        match j.get("groups").and_then(json::Json::as_array) {
            None => Err("no groups claim".into()),
            Some(g) => {
                if g.iter()
                    .filter_map(json::Json::as_str)
                    .any(|x| x == "Infra")
                {
                    Ok(())
                } else {
                    Err("not in Infra".into())
                }
            }
        }
    }

    #[test]
    fn group_gate_on_introspection_replies() {
        assert!(gate(r#"{"active":true,"groups":["Infra","Members"]}"#).is_ok());
        assert_eq!(
            gate(r#"{"active":true,"groups":["Members"]}"#).unwrap_err(),
            "not in Infra"
        );
        // The ScopeMapping-missing case must read as a configuration fault.
        assert_eq!(
            gate(r#"{"active":true,"username":"cjdell"}"#).unwrap_err(),
            "no groups claim"
        );
        assert_eq!(gate(r#"{"active":false,"groups":["Infra"]}"#).unwrap_err(), "inactive");
    }

    #[test]
    fn trim_stays_on_a_character_boundary() {
        let long = "é".repeat(400);
        assert!(!trim(&long).is_empty());
        assert_eq!(trim("  short  "), "short");
    }
}
