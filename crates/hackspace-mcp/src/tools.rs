//! `tools` — the tool table, and the one way a tool gets into it.
//!
//! `build` is the whole surface the agent sees, so "how many tools does the model
//! have to choose between" is answerable by reading this one function. That number
//! is a design constraint and not a style preference: the engine is a
//! quantised coder model (see `docs/slack-agent-2026-11-16.md` §3), and the way
//! those models fail is not by mis-reading a schema but by picking the wrong one
//! of fifteen or narrating instead of calling. Seven tools, each broad, each
//! returning a *short text answer* rather than a JSON blob, beat seven narrow
//! ones — which is why `space_status` answers "is it open" and "what's the
//! temperature" together: they are one upstream call, and splitting them would
//! cost a round trip of ~243 tokens of prefill each.
//!
//! It is **ten** now, and the extra three are `tools_basics`' clock, calculator and unit
//! converter. The rule above is about near-duplicates, and those are the expensive case:
//! `space_status`/`temperature` would be two ways to answer one question. A wall clock, a
//! sum and a conversion are as far apart as any three tools here get, none of them competes
//! with an existing one, and every one of them replaces a job the model was previously told
//! to do in its head. Ten is still "readable as one list", which is what the number is
//! actually protecting; if a future tool's description has to say "not to be confused with
//! X", that is the signal to merge rather than add.
//!
//! [`register`] is the single funnel so every tool gets the same three things
//! without repeating them: a name that matches its function, the counter that
//! makes a broken tool visible on `/metrics`, and no access to anything except
//! the `Sources` it was handed.

use crate::sources::Sources;
use crate::tools_basics;
use crate::tools_hass;
use crate::tools_space;
use crate::tools_watch;
use common_json::Json;
use common_mcp::{Server, Tool};
use std::sync::Arc;

pub fn build(src: &Arc<Sources>) -> Server {
    let mut srv = Server::new("hackspace-mcp", env!("CARGO_PKG_VERSION"));
    tools_space::register_tools(&mut srv, src);
    tools_basics::register_tools(&mut srv, src);
    tools_hass::register_tools(&mut srv, src);
    tools_watch::register_tools(&mut srv, src);
    srv
}

/// The one registration funnel.  `fun` is a plain function pointer, so the only
/// state a tool can reach is the `Sources` shared with it — no tool can read a
/// file or another secret, which is the property that lets this process be the one
/// holding the Home Assistant token.
pub fn register(
    srv: &mut Server,
    name: &str,
    description: &str,
    schema: Json,
    src: &Arc<Sources>,
    fun: fn(&Sources, &Json) -> Result<String, String>,
) {
    let src = Arc::clone(src);
    let named = name.to_string();
    srv.add(
        Tool {
            name: name.to_string(),
            description: description.to_string(),
            schema,
        },
        Box::new(move |args: &Json| {
            let outcome = fun(&src, args);
            src.note_tool(&named, outcome.is_ok());
            outcome
        }),
    );
}

/// Shared list-rendering: `a · b · c`, and the "(+N more)" tail that keeps a
/// large result set cheap without silently dropping it.
pub fn join_short(items: &[String], limit: usize) -> String {
    if items.is_empty() {
        return "none".to_string();
    }
    let shown: Vec<&str> = items.iter().take(limit).map(String::as_str).collect();
    let mut out = shown.join(" · ");
    if items.len() > limit {
        out.push_str(&format!(" (+{} more)", items.len() - limit));
    }
    out
}

pub fn clamp_usize(v: Option<i64>, default: usize, max: usize) -> usize {
    v.map(|v| v.clamp(1, max as i64) as usize).unwrap_or(default)
}
