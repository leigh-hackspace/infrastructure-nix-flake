//! OIDC login against authentik: authorization-code flow with PKCE (S256),
//! token exchange with client-secret, then token introspection to check the
//! user's groups — only members of `Infra` get a session.
//!
//! Sessions are opaque random cookies kept in memory (the process is the
//! single authority; they die on restart, which is fine for an operator UI).

use std::time::{SystemTime, UNIX_EPOCH};

use base64ct::{Base64Url, Base64UrlUnpadded, Encoding};
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

pub const COOKIE_NAME: &str = "filestore";

/// A session with the standard TTL (used by the dev-login path).
pub fn session_for_dev(username: &str) -> Session {
    Session {
        username: username.to_string(),
        email: None,
        expires: now_secs() + SESSION_TTL_SECS,
    }
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

fn b64(b: &str) -> String {
    base64url(b.as_bytes())
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

fn sha256(bytes: &[u8]) -> Vec<u8> {
    let mut h = sha2::Sha256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

pub struct AuthError(pub String);

/// Step 1: build the /authorize/ redirect and remember the PKCE verifier.
pub fn begin_login(shared: &crate::Shared) -> (String, String) {
    let state = random_hex(16);
    let verifier = base64url(&random_bytes(32));
    let challenge = base64url_unpadded(&sha256(verifier.as_bytes()));

    let mut pending = shared.pending.lock().unwrap();
    let now = now_secs();
    pending.retain(|_, p| p.created + PENDING_TTL > now);
    pending.insert(state.clone(), Pending { verifier, created: now });

    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope=openid profile email groups&state={}&code_challenge={}&code_challenge_method=S256",
        shared.cfg.oidc_authorize,
        urlencoding::encode(&shared.cfg.oidc_client_id),
        urlencoding::encode(&shared.cfg.redirect_uri),
        urlencoding::encode(&state),
        urlencoding::encode(&challenge),
    );
    (url, state)
}

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

/// Step 2: exchange the code, introspect, enforce the group, make a session.
/// Returns (cookie, session), or an error message suitable for the UI.
pub async fn finish_login(
    shared: &crate::Shared,
    code: &str,
    state: &str,
) -> Result<(String, Session), AuthError> {
    let verifier = shared
        .pending
        .lock()
        .unwrap()
        .remove(state)
        .ok_or_else(|| AuthError("unknown or expired state — start login again".to_string()))?
        .verifier;

    // Token exchange.  authentik accepts client credentials in the request
    // body (client_secret_post) as well as basic auth; we send both to be
    // robust.
    let resp = shared
        .http
        .post(&shared.cfg.oidc_token)
        .basic_auth(&shared.cfg.oidc_client_id, Some(&shared.cfg.oidc_client_secret))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", shared.cfg.redirect_uri.as_str()),
            ("client_id", shared.cfg.oidc_client_id.as_str()),
            ("client_secret", shared.cfg.oidc_client_secret.as_str()),
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
    let creds = b64(&format!(
        "{}:{}",
        shared.cfg.oidc_client_id, shared.cfg.oidc_client_secret
    ));
    let resp = shared
        .http
        .post(&shared.cfg.oidc_introspect)
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

    // Group restriction.  If authentik did not include the groups claim we
    // fail closed rather than guessing.
    match &intro.groups {
        Some(groups) if groups.iter().any(|g| g == &shared.cfg.required_group) => {}
        Some(_groups) => {
            return Err(AuthError(format!(
                "access denied: {username} is not in group '{}'",
                shared.cfg.required_group
            )))
        }
        None => {
            return Err(AuthError(
                "introspection did not include a groups claim — cannot verify group membership"
                    .to_string(),
            ))
        }
    }

    let session = Session {
        username,
        email: intro.email,
        expires: now_secs() + SESSION_TTL_SECS,
    };
    let cookie = random_hex(24);
    shared
        .sessions
        .lock()
        .unwrap()
        .insert(cookie.clone(), session.clone());
    Ok((cookie, session))
}

/// Returns the session if a valid session value is present.
pub fn session_for_cookie(shared: &crate::Shared, value: &str) -> Option<Session> {
    let now = now_secs();
    let mut sessions = shared.sessions.lock().unwrap();
    sessions.retain(|_, s| s.expires > now);
    if value.is_empty() {
        return None;
    }
    sessions.get(value).cloned()
}


