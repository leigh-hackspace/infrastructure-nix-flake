//! OIDC login against authentik: authorization-code flow with PKCE (S256),
//! token exchange with client-secret, then token introspection to check the
//! user's groups — only members of the required group get a session.
//!
//! Sessions are opaque random cookies kept in memory (the process is the
//! single authority; they die on restart, which is fine for an operator UI).
//!
//! This is the shared half of the login for the LAN web apps (filestore,
//! gocardless-dashboard): both are authentik clients with the same client
//! contract and the same group gate, so the flow lives here once.  What stays
//! per app is the `OidcConfig` (client id/secret, endpoints, redirect URI,
//! cookie name, required group) and how the app's HTTP layer turns the
//! resulting `Session` into a cookie.  See the Authentik section of AGENTS.md
//! for the provider-side contract (S256 challenge must be unpadded, and the
//! `groups` claim only appears if the provider has a ScopeMapping child row).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64ct::{Base64, Base64Url, Base64UrlUnpadded, Encoding};
use serde::Deserialize;
use sha2::Digest;

#[derive(Clone)]
pub struct Session {
    pub username: String,
    pub email: Option<String>,
    /// unix seconds
    pub expires: i64,
}

pub struct Pending {
    pub verifier: String,
    /// unix seconds; entries older than 10 min are dropped.
    pub created: i64,
}

/// Per-app OIDC settings.  The endpoint URLs are absolute because authentik
/// serves them under `/application/o/…`, not under a discovery path.
#[derive(Clone)]
pub struct OidcConfig {
    /// Name of the session cookie this app sets (also what logout clears).
    pub cookie_name: String,
    pub client_id: String,
    pub client_secret: String,
    /// e.g. https://id.leighhack.org/application/o/authorize/
    pub authorize_url: String,
    /// e.g. https://id.leighhack.org/application/o/token/
    pub token_url: String,
    /// e.g. https://id.leighhack.org/application/o/introspect/
    pub introspect_url: String,
    /// e.g. https://filestore.int.leighhack.org/auth/callback
    pub redirect_uri: String,
    /// authentik group required for login.
    pub required_group: String,
}

const SESSION_TTL_SECS: i64 = 12 * 3600;
const PENDING_TTL: i64 = 600;

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).expect("getrandom");
    b
}

pub fn random_hex(nbytes: usize) -> String {
    let mut s = String::with_capacity(nbytes * 2);
    for b in random_bytes(nbytes) {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn base64url(bytes: &[u8]) -> String {
    let len = <Base64Url as Encoding>::encoded_len(bytes);
    let mut buf = vec![0u8; len];
    <Base64Url as Encoding>::encode(bytes, &mut buf)
        .expect("base64url")
        .to_string()
}

/// Unpadded base64url — RFC 7636 requires the S256 challenge to be
/// base64url **without** padding (authentik compares it unpadded).
fn base64url_unpadded(bytes: &[u8]) -> String {
    let len = <Base64UrlUnpadded as Encoding>::encoded_len(bytes);
    let mut buf = vec![0u8; len];
    <Base64UrlUnpadded as Encoding>::encode(bytes, &mut buf)
        .expect("base64url-unpadded")
        .to_string()
}

/// The `code_challenge` for a verifier: unpadded base64url of the SHA-256.
/// authentik recomputes `urlsafe_b64encode(sha256(verifier)).replace("=","")`
/// and compares for equality, so a padded challenge saves fine but fails at
/// token time with `invalid_grant` ("Code challenge not matching").
fn pkce_challenge(verifier: &str) -> String {
    base64url_unpadded(&sha256(verifier.as_bytes()))
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    let mut h = sha2::Sha256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

/// Standard base64 for an HTTP `Basic` credential pair.  This deliberately does
/// *not* use the base64url alphabet: a secret containing `+` or `/` would be
/// encoded with characters the server is not required to accept in a Basic
/// header (the two alphabets only differ for those two characters, which is why
/// the old copy of this code worked).
fn basic_credentials(user: &str, password: &str) -> String {
    let pair = format!("{user}:{password}");
    let len = <Base64 as Encoding>::encoded_len(pair.as_bytes());
    let mut buf = vec![0u8; len];
    <Base64 as Encoding>::encode(pair.as_bytes(), &mut buf)
        .expect("base64")
        .to_string()
}

pub struct AuthError(pub String);

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IntrospectionResponse {
    active: bool,
    #[serde(default)]
    #[allow(dead_code)]
    scope: Option<String>,
    #[serde(default)]
    groups: Option<Vec<String>>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

/// A session value with the standard TTL, **not** registered with any login.
/// The `--no-auth` path uses this: it never sets a cookie and treats every
/// request as a local session.
pub fn dev_session(username: &str) -> Session {
    Session {
        username: username.to_string(),
        email: None,
        expires: now_secs() + SESSION_TTL_SECS,
    }
}

pub struct Oidc {
    cfg: OidcConfig,
    http: reqwest::Client,
    sessions: Mutex<HashMap<String, Session>>,
    pending: Mutex<HashMap<String, Pending>>,
}

impl Oidc {
    pub fn new(cfg: OidcConfig, http: reqwest::Client) -> Self {
        Self {
            cfg,
            http,
            sessions: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &OidcConfig {
        &self.cfg
    }

    /// The name of the session cookie this app uses.
    pub fn cookie_name(&self) -> &str {
        &self.cfg.cookie_name
    }

    /// Step 1: build the /authorize/ redirect and remember the PKCE verifier.
    pub fn begin_login(&self) -> (String, String) {
        let state = random_hex(16);
        let verifier = base64url(&random_bytes(32));
        let challenge = pkce_challenge(&verifier);

        let mut pending = self.pending.lock().unwrap();
        let now = now_secs();
        pending.retain(|_, p| p.created + PENDING_TTL > now);
        pending.insert(
            state.clone(),
            Pending {
                verifier,
                created: now,
            },
        );

        // NB: the scope list is interpolated literally (raw spaces), exactly as
        // it always has been: authentik's provider accepts it and browsers
        // re-encode the space before the request goes out.  Changing it is not
        // worth an untested change to a working login.
        let url = format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope=openid profile email groups&state={}&code_challenge={}&code_challenge_method=S256",
            self.cfg.authorize_url,
            urlencoding::encode(&self.cfg.client_id),
            urlencoding::encode(&self.cfg.redirect_uri),
            urlencoding::encode(&state),
            urlencoding::encode(&challenge),
        );
        (url, state)
    }

    /// Step 2: exchange the code, introspect, enforce the group, make a session.
    /// Returns (cookie, session), or an error message suitable for the UI.
    pub async fn finish_login(
        &self,
        code: &str,
        state: &str,
    ) -> Result<(String, Session), AuthError> {
        let verifier = self
            .pending
            .lock()
            .unwrap()
            .remove(state)
            .ok_or_else(|| AuthError("unknown or expired state — start login again".to_string()))?
            .verifier;

        // Token exchange.  authentik accepts client credentials in the request
        // body (client_secret_post) as well as basic auth; we send both to be
        // robust.
        let resp = self
            .http
            .post(&self.cfg.token_url)
            .basic_auth(&self.cfg.client_id, Some(&self.cfg.client_secret))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("code_verifier", verifier.as_str()),
                ("redirect_uri", self.cfg.redirect_uri.as_str()),
                ("client_id", self.cfg.client_id.as_str()),
                ("client_secret", self.cfg.client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|e| AuthError(format!("token request failed: {e}")))?;
        let status = resp.status();
        let tok: TokenResponse = resp
            .json()
            .await
            .map_err(|e| AuthError(format!("bad token response: {e}")))?;
        if !status.is_success() {
            return Err(AuthError(format!(
                "token endpoint: {} ({})",
                tok.error.unwrap_or_default(),
                tok.error_description.unwrap_or_default()
            )));
        }
        let access_token = tok
            .access_token
            .ok_or_else(|| AuthError("no access_token in response".to_string()))?;

        // Introspection with basic auth (client_id:client_secret).
        let creds = basic_credentials(&self.cfg.client_id, &self.cfg.client_secret);
        let resp = self
            .http
            .post(&self.cfg.introspect_url)
            .header("Authorization", format!("Basic {creds}"))
            .form(&[("token", access_token.as_str())])
            .send()
            .await
            .map_err(|e| AuthError(format!("introspection request failed: {e}")))?;
        let status = resp.status();
        let intro: IntrospectionResponse = resp
            .json()
            .await
            .map_err(|e| AuthError(format!("bad introspection response: {e}")))?;
        if !status.is_success() {
            return Err(AuthError(format!(
                "introspection failed with HTTP {}",
                status.as_u16()
            )));
        }
        if !intro.active {
            return Err(AuthError("token is not active".to_string()));
        }

        let username = intro
            .preferred_username
            .unwrap_or_else(|| "unknown".to_string());
        enforce_group(&self.cfg.required_group, &username, intro.groups.as_ref())?;

        let session = Session {
            username,
            email: intro.email,
            expires: now_secs() + SESSION_TTL_SECS,
        };
        let cookie = random_hex(24);
        self.sessions
            .lock()
            .unwrap()
            .insert(cookie.clone(), session.clone());
        Ok((cookie, session))
    }

    /// Returns the session if a valid session value is present.
    pub fn session_for_cookie(&self, value: &str) -> Option<Session> {
        let now = now_secs();
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, s| s.expires > now);
        if value.is_empty() {
            return None;
        }
        sessions.get(value).cloned()
    }

    /// Mints a session without going through OIDC and registers it.  This is a
    /// testing-only escape hatch (the apps only wire it
    /// up behind an explicit flag, and filestore additionally restricts it to a
    /// loopback bind) — see `filestore --dev-user`.  Returns (cookie, session)
    /// because the caller has to set the cookie it is handed.
    pub fn add_dev_session(&self, username: &str) -> (String, Session) {
        let session = dev_session(username);
        let cookie = random_hex(24);
        self.sessions
            .lock()
            .unwrap()
            .insert(cookie.clone(), session.clone());
        (cookie, session)
    }

    /// Drops a session (logout).  Returns whether there was one.
    pub fn drop_session(&self, value: &str) -> bool {
        self.sessions.lock().unwrap().remove(value).is_some()
    }

    /// Drops expired sessions and abandoned logins.
    pub fn purge_expired(&self) {
        let now = now_secs();
        self.sessions.lock().unwrap().retain(|_, s| s.expires > now);
        self.pending
            .lock()
            .unwrap()
            .retain(|_, p| p.created + PENDING_TTL > now);
    }
}

/// The group gate.  If authentik did not include the groups claim we fail
/// closed rather than guessing — a provider whose `groups` ScopeMapping child
/// row is missing issues tokens with no group at all (see AGENTS.md).
fn enforce_group(
    required: &str,
    username: &str,
    groups: Option<&Vec<String>>,
) -> Result<(), AuthError> {
    match groups {
        Some(groups) if groups.iter().any(|g| g == required) => Ok(()),
        Some(_) => Err(AuthError(format!(
            "access denied: {username} is not in group '{required}'"
        ))),
        None => Err(AuthError(
            "introspection did not include a groups claim — cannot verify group membership"
                .to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> OidcConfig {
        OidcConfig {
            cookie_name: "test_session".into(),
            client_id: "cid".into(),
            client_secret: "s3cr3t".into(),
            authorize_url: "https://id.example/application/o/authorize/".into(),
            token_url: "https://id.example/application/o/token/".into(),
            introspect_url: "https://id.example/application/o/introspect/".into(),
            redirect_uri: "https://app.example/auth/callback".into(),
            required_group: "Infra".into(),
        }
    }

    fn oidc() -> Oidc {
        Oidc::new(cfg(), reqwest::Client::new())
    }

    /// The authentik PKCE contract.  A padded challenge saves fine at
    /// /authorize/ and only dies at token time with `invalid_grant` ("Code
    /// challenge not matching"), which is why this is pinned here.
    #[test]
    fn pkce_challenge_matches_the_rfc_7636_test_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn pkce_challenge_is_always_unpadded_base64url() {
        // A 32-byte verifier always yields 43 base64url chars and never a pad.
        for _ in 0..256 {
            let v = base64url(&random_bytes(32));
            let c = pkce_challenge(&v);
            assert_eq!(c.len(), 43, "challenge {c} for verifier {v}");
            assert!(!c.contains('='), "padded challenge {c}");
            assert!(!c.contains('+') && !c.contains('/'), "non-urlsafe {c}");
        }
    }

    #[test]
    fn begin_login_url_carries_the_client_contract() {
        let o = oidc();
        let (url, state) = o.begin_login();
        assert!(url
            .starts_with("https://id.example/application/o/authorize/?client_id=cid&redirect_uri="));
        for want in [
            "response_type=code",
            "scope=openid profile email groups",
            "code_challenge_method=S256",
        ] {
            assert!(url.contains(want), "missing {want} in {url}");
        }
        // The state handed to the browser is the key the verifier is stored
        // under, so the callback can never arrive with an unknown state.
        assert!(url.contains(&format!("state={state}")));
        assert!(o.pending.lock().unwrap().contains_key(&state));
    }

    #[test]
    fn begin_login_forgets_pending_logins_older_than_the_ttl() {
        let o = oidc();
        o.pending.lock().unwrap().insert(
            "stale".into(),
            Pending {
                verifier: "v".into(),
                created: now_secs() - PENDING_TTL - 1,
            },
        );
        o.begin_login();
        assert!(!o.pending.lock().unwrap().contains_key("stale"));
    }

    #[test]
    fn finish_login_rejects_an_unknown_or_replayed_state() {
        // The verifier is removed on lookup, so a replayed callback cannot
        // exchange the same state twice.
        let rt = tokio_block_on(|| async {
            let o = oidc();
            let (_, state) = o.begin_login();
            let first = o.finish_login("code", &state).await;
            let second = o.finish_login("code", &state).await;
            (first.is_err(), second.is_err())
        });
        assert!(rt.0 && rt.1);
    }

    #[test]
    fn group_gate_fails_closed_without_the_groups_claim() {
        let err = enforce_group("Infra", "cjdell", None).unwrap_err();
        assert!(err.0.contains("did not include a groups claim"));
    }

    #[test]
    fn group_gate_admits_only_the_required_group() {
        let members = vec!["everyone".to_string(), "Infra".to_string()];
        let outsiders = vec!["everyone".to_string(), "pgina".to_string()];
        assert!(enforce_group("Infra", "cjdell", Some(&members)).is_ok());
        let err = enforce_group("Infra", "cjdell", Some(&outsiders)).unwrap_err();
        assert_eq!(err.0, "access denied: cjdell is not in group 'Infra'");
    }

    #[test]
    fn introspection_without_groups_deserializes_as_none() {
        // The fail-closed path depends on `groups` being absent rather than an
        // empty list: an empty list means "member of nothing", absent means the
        // ScopeMapping is missing from the provider.
        let intro: IntrospectionResponse =
            serde_json::from_str(r#"{"active": true, "preferred_username": "cjdell"}"#).unwrap();
        assert!(intro.active);
        assert!(intro.groups.is_none());
        let intro: IntrospectionResponse =
            serde_json::from_str(r#"{"active": true, "groups": []}"#).unwrap();
        assert_eq!(intro.groups, Some(Vec::new()));
    }

    #[test]
    fn basic_credentials_use_the_standard_alphabet() {
        assert_eq!(basic_credentials("cid", "s3cr3t"), "Y2lkOnMzY3IzdA==");
        // Inputs whose base64 uses the two characters where the standard and
        // URL-safe alphabets diverge: a Basic header must emit '+' and '/',
        // never '-' and '_'.  (The old copy of this code base64url-encoded the
        // credentials, which only worked because no secret contained either.)
        assert_eq!(basic_credentials("?", "~"), "Pzp+");
        assert_eq!(basic_credentials("?", "?~"), "Pzo/fg==");
    }

    #[test]
    fn sessions_expire_are_droppable_and_purged() {
        let o = oidc();
        let (cookie, session) = o.add_dev_session("cjdell");
        assert_eq!(session.username, "cjdell");
        assert_eq!(o.session_for_cookie(&cookie).unwrap().username, "cjdell");
        assert!(o.drop_session(&cookie));
        assert!(!o.drop_session(&cookie));
        assert!(o.session_for_cookie(&cookie).is_none());
        // An empty cookie is never a session, whatever is in the map.
        assert!(o.session_for_cookie("").is_none());

        let stale = o.add_session_at(now_secs() - 1);
        o.purge_expired();
        assert!(!o.sessions.lock().unwrap().contains_key(&stale));
    }

    #[test]
    fn purge_expired_keeps_live_state() {
        let o = oidc();
        let live = o.add_session_at(now_secs() + 60);
        let dead = o.add_session_at(now_secs() - 60);
        o.pending.lock().unwrap().insert(
            "old".into(),
            Pending {
                verifier: "v".into(),
                created: now_secs() - PENDING_TTL - 1,
            },
        );
        o.purge_expired();
        let sessions = o.sessions.lock().unwrap();
        assert!(sessions.contains_key(&live));
        assert!(!sessions.contains_key(&dead));
        assert!(!o.pending.lock().unwrap().contains_key("old"));
    }

    impl Oidc {
        /// Test helper: register a session with an explicit expiry.
        fn add_session_at(&self, expires: i64) -> String {
            let cookie = random_hex(8);
            self.sessions.lock().unwrap().insert(
                cookie.clone(),
                Session {
                    username: "u".into(),
                    email: None,
                    expires,
                },
            );
            cookie
        }
    }

    /// Runs a future on a throwaway current-thread runtime (the crate is used
    /// from both an async and a threaded main, so the tests do not depend on
    /// `#[tokio::test]` being available in every crate).
    fn tokio_block_on<F: std::future::Future>(f: impl FnOnce() -> F) -> F::Output {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(f())
    }
}
