//! `common-mcp` — the **Model Context Protocol**, both halves, small.
//!
//! `hackspace-mcp` serves it (the building's data as tools) and `slack-agent`
//! reads it (the agent's tool list comes from here, not from a hardcoded array),
//! so the two halves live in one place and cannot drift.
//!
//! ## What is implemented, and what that costs
//!
//! The **Streamable HTTP** transport in its **stateless** form — one JSON-RPC 2.0
//! request per `POST`, one JSON body back — plus the four methods an agent needs:
//!
//! | method | server | client |
//! | --- | --- | --- |
//! | `initialize` | answers with `protocolVersion`, `capabilities.tools`, `serverInfo` | sent once, lazily |
//! | `notifications/initialized` | accepted, no reply | sent after `initialize` |
//! | `ping` | answers `{}` | — |
//! | `tools/list` | the tool table, with JSON schemas | parsed into [`Tool`] |
//! | `tools/call` | dispatch, wrapped as `content[]` + `isError` | text extracted, `isError` surfaced as `Err` |
//!
//! Not implemented: SSE *as a transport we push on*, sessions, `resources/*`,
//! `prompts/*`, `sampling/*`, roots, logging, cancellation. Home Assistant's own
//! `mcp_server` integration is in the same position (its docs list Sampling and
//! Notifications as unsupported) and it answers statelessly on `/api/mcp`, which is
//! why [`Client::post`] will still unwrap an `event:`/`data:` frame if a server
//! replies that way: accepting both framings costs twelve lines and is the whole
//! difference between pointing this client at a foreign server and not.
//!
//! **This is the crate to delete if we ever need more of the spec.** The
//! replacement is upstream's own SDK — `rmcp` for Rust — which is a real dependency
//! (a schema framework plus proc macros) for a surface we use four methods of. It
//! was measured against our lock and deliberately not taken: the workspace already
//! carries enough dependency weight, and the same judgement that produced
//! `common-rs/json` and `common-rs/web` — hand-roll the small protocol, write down
//! exactly which subset, keep the zero-crate style — applies here. Re-verify with:
//! `cargo test -p common-mcp`, which round-trips a server and a client over a
//! socket, so the wire format is tested rather than asserted.
//!
//! ## Two rules the API enforces
//!
//! - **A tool's failure is a result, not a protocol error.** MCP says so and it
//!   matters more here than in a desktop client: a `Err`-typed JSON-RPC error makes
//!   some models treat the whole tool as broken and stop calling it, whereas
//!   `isError: true` with a sentence saying *why* ("401 — no Home Assistant token")
//!   is what makes the bot answer "I can see the space API, not the lights".
//!   [`Server`] therefore only produces JSON-RPC errors for bad requests, and
//!   handlers return `Result<String, String>` where `Err` becomes `isError`.
//! - **Unknown tool is a protocol error** (`-32602`), because that is a bug in the
//!   caller rather than a fact about the world.

use common_json::{self as json, Json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The protocol version we advertise.  Servers must reply with a version they
/// support; a client that does not recognise the answer should fail rather than
/// assume, which is what [`Client::initialize`] does.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

// JSON-RPC 2.0's reserved codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// One tool as it appears on the wire in `tools/list`.
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// The JSON Schema for `arguments`, verbatim: it goes from one side of a
    /// model's `tools` array to the other without this crate deciding anything.
    pub schema: Json,
}

impl Tool {
    /// The same tool in the shape an OpenAI-dialect chat-completions request
    /// wants: `{"type":"function","function":{name,description,parameters}}`.
    ///
    /// It lives here rather than in the agent because it is a second rendering of
    /// the *same* wire object, and the day the two disagree is the day the agent
    /// calls a tool with arguments its own server never advertised.
    pub fn openai_definition(&self) -> Json {
        json::object(vec![
            ("type".to_string(), json::string("function")),
            (
                "function".to_string(),
                json::object(vec![
                    ("name".to_string(), json::string(self.name.clone())),
                    ("description".to_string(), json::string(self.description.clone())),
                    (
                        "parameters".to_string(),
                        if self.schema.get("type").is_some() {
                            self.schema.clone()
                        } else {
                            // An engine that gets a schema with no `type: object`
                            // is entitled to reject it; llama.cpp has been seen to.
                            let mut s = match self.schema.clone() {
                                Json::Obj(m) => m,
                                _ => BTreeMap::new(),
                            };
                            s.insert("type".to_string(), json::string("object"));
                            Json::Obj(s)
                        },
                    ),
                ]),
            ),
        ])
    }
}

/// A tool implementation: takes parsed `arguments`, answers with text or explains
/// why it could not.
pub type Handler = Box<dyn Fn(&Json) -> Result<String, String> + Send + Sync + 'static>;

pub struct Server {
    name: String,
    version: String,
    tools: BTreeMap<String, (Tool, Handler)>,
}

impl Server {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Server {
            name: name.into(),
            version: version.into(),
            tools: BTreeMap::new(),
        }
    }

    /// Register a tool.  Re-registering a name replaces it and keeps the map
    /// sorted, so `tools/list` is stable across restarts and diffable.
    pub fn add(&mut self, tool: Tool, handler: Handler) {
        self.tools.insert(tool.name.clone(), (tool, handler));
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Handle one request body and return the response to write, or `None` for
    /// nothing to write (a notification, or a client's own response message).
    ///
    /// This is a pure function of the body, which is what lets the tests drive it
    /// with no socket and lets the server choose any HTTP shape it likes.
    pub fn handle(&self, body: &str) -> Option<String> {
        let req = match json::parse(body) {
            Ok(v) => v,
            Err(e) => return Some(rpc_error(&Json::Null, PARSE_ERROR, &format!("invalid JSON: {e}"))),
        };
        if req.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
            let id = req.get("id").cloned().unwrap_or(Json::Null);
            return Some(rpc_error(
                &id,
                INVALID_REQUEST,
                "expected {\"jsonrpc\":\"2.0\",…} — this endpoint speaks MCP over Streamable HTTP",
            ));
        }
        let method = req
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        // A notification (no id) gets no reply.  `notifications/initialized`
        // is the only one we expect, and its arrival is not something to argue
        // about: the absence of an id is the protocol's own signal.
        let id = req.get("id").cloned()?;
        let params = match req.get("params") {
            Some(v) => v.clone(),
            None => json::object(Vec::new()),
        };

        let outcome = match method.as_str() {
            "initialize" => Ok(initialize_result(&self.name, &self.version)),
            "ping" => Ok(json::object(Vec::new())),
            "tools/list" => Ok(json::object(vec![(
                "tools".to_string(),
                Json::Arr(
                    self.tools
                        .values()
                        .map(|(t, _)| {
                            json::object(vec![
                                ("name".to_string(), json::string(t.name.clone())),
                                ("description".to_string(), json::string(t.description.clone())),
                                (
                                    "inputSchema".to_string(),
                                    if t.schema.get("type").is_some() {
                                        t.schema.clone()
                                    } else {
                                        with_object_type(&t.schema)
                                    },
                                ),
                            ])
                        })
                        .collect(),
                ),
            )])),
            "tools/call" => self.call(&params),
            other => Err((
                METHOD_NOT_FOUND,
                format!("method not supported by this server: {other}"),
            )),
        };
        Some(match outcome {
            Ok(result) => rpc_result(&id, result),
            Err((code, message)) => rpc_error(&id, code, &message),
        })
    }

    fn call(&self, params: &Json) -> Result<Json, (i64, String)> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or((INVALID_PARAMS, "tools/call needs a \"name\"".to_string()))?
            .to_string();
        let (_, handler) = self
            .tools
            .get(&name)
            .ok_or((INVALID_PARAMS, format!("unknown tool: {name}")))?;
        let empty = json::object(Vec::new());
        let arguments = params.get("arguments").unwrap_or(&empty);
        // Handler failures are results: see the module header.
        match handler(arguments) {
            Ok(text) => Ok(tool_contents(&text, false)),
            Err(e) => Ok(tool_contents(&e, true)),
        }
    }
}

fn initialize_result(name: &str, version: &str) -> Json {
    json::object(vec![
        (
            "protocolVersion".to_string(),
            json::string(PROTOCOL_VERSION),
        ),
        (
            "capabilities".to_string(),
            json::object(vec![(
                "tools".to_string(),
                json::object(vec![("listChanged".to_string(), json::boolean(false))]),
            )]),
        ),
        (
            "serverInfo".to_string(),
            json::object(vec![
                ("name".to_string(), json::string(name)),
                ("version".to_string(), json::string(version)),
            ]),
        ),
    ])
}

fn tool_contents(text: &str, is_error: bool) -> Json {
    json::object(vec![
        (
            "content".to_string(),
            Json::Arr(vec![json::object(vec![
                ("type".to_string(), json::string("text")),
                ("text".to_string(), json::string(text)),
            ])]),
        ),
        ("isError".to_string(), json::boolean(is_error)),
    ])
}

fn with_object_type(schema: &Json) -> Json {
    let mut map = match schema.clone() {
        Json::Obj(m) => m,
        _ => BTreeMap::new(),
    };
    map.insert("type".to_string(), json::string("object"));
    Json::Obj(map)
}

/// `{"type":"object","properties":{…},"required":[…]}` — the shape every MCP tool's
/// `inputSchema` is, and every caller here writes by hand otherwise.
pub fn object_schema(properties: Vec<(&str, Json)>, required: Vec<&str>) -> Json {
    json::object(vec![
        ("type".to_string(), json::string("object")),
        (
            "properties".to_string(),
            json::object(properties.into_iter().map(|(k, v)| (k.to_string(), v))),
        ),
        (
            "required".to_string(),
            json::string_array(required.iter().map(|s| (*s).to_string())),
        ),
    ])
}

/// `{"type":…,"description":…}` — one parameter.  The description is the only
/// channel a model has for a parameter's meaning, so it is part of the type here,
/// not a comment: it is where "°C" and "domain, e.g. sensor|light" go.
pub fn param(kind: &str, description: &str) -> Json {
    json::object(vec![
        ("type".to_string(), json::string(kind)),
        ("description".to_string(), json::string(description)),
    ])
}

/// A `string` parameter constrained to a fixed set of values.
pub fn enum_param(kind: &str, description: &str, values: &[&str]) -> Json {
    let mut map = match param(kind, description) {
        Json::Obj(m) => m,
        _ => BTreeMap::new(),
    };
    map.insert(
        "enum".to_string(),
        json::string_array(values.iter().map(|v| (*v).to_string())),
    );
    Json::Obj(map)
}

fn rpc_result(id: &Json, result: Json) -> String {
    json::emit(
        &json::object(vec![
            ("jsonrpc".to_string(), json::string("2.0")),
            ("id".to_string(), id.clone()),
            ("result".to_string(), result),
        ]),
        0,
    )
}

fn rpc_error(id: &Json, code: i64, message: &str) -> String {
    json::emit(
        &json::object(vec![
            ("jsonrpc".to_string(), json::string("2.0")),
            ("id".to_string(), id.clone()),
            (
                "error".to_string(),
                json::object(vec![
                    ("code".to_string(), Json::Num(code as f64)),
                    ("message".to_string(), json::string(message)),
                ]),
            ),
        ]),
        0,
    )
}

// --- the client side -----------------------------------------------------------

/// A stateless MCP client over Streamable HTTP.
pub struct Client {
    url: String,
    next_id: AtomicU64,
    initialized: AtomicBool,
}

impl Client {
    pub fn new(url: impl Into<String>) -> Self {
        Client {
            url: url.into(),
            next_id: AtomicU64::new(1),
            initialized: AtomicBool::new(false),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Send `initialize` once per process, then `notifications/initialized`.
    ///
    /// Its failure is deliberately *not* fatal: `tools/list` is the real signal of
    /// a live server, and a server that answers `tools/list` but not `initialize`
    /// is a server we would rather use than refuse.  The reason is kept in the
    /// error the caller sees on the next call, not swallowed: `last_initialize`
    /// is folded into [`Client::post`]'s error text when a call *does* fail.
    fn ensure_initialized(&self) {
        if self.initialized.swap(true, Ordering::SeqCst) {
            return;
        }
        let params = json::object(vec![
            ("protocolVersion".to_string(), json::string(PROTOCOL_VERSION)),
            (
                "capabilities".to_string(),
                json::object(vec![(
                    "tools".to_string(),
                    json::object(vec![("clientInfo".to_string(), json::string(""))]),
                )]),
            ),
            (
                "clientInfo".to_string(),
                json::object(vec![
                    ("name".to_string(), json::string("slack-agent")),
                    ("version".to_string(), json::string(env!("CARGO_PKG_VERSION"))),
                ]),
            ),
        ]);
        let _ = self.post("initialize", params, false);
        // A notification: no id, and nothing to wait for.
        let _ = self.post("notifications/initialized", json::object(Vec::new()), true);
    }

    fn post(&self, method: &str, params: Json, notification: bool) -> Result<Json, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut pairs = vec![
            ("jsonrpc".to_string(), json::string("2.0")),
            ("method".to_string(), json::string(method)),
            ("params".to_string(), params),
        ];
        if !notification {
            pairs.push(("id".to_string(), json::number_from_int(id)));
        }
        let body = json::emit(&json::object(pairs), 0);
        let resp = common_http::post_json(
            &self.url,
            &body,
            &[("Accept", "application/json, text/event-stream")],
        )
        .map_err(|e| format!("{method} → {}: {e}", self.url))?;
        if !resp.ok() {
            return Err(format!("{method} → {}: {}", self.url, resp.describe()));
        }
        if notification {
            return Ok(json::object(Vec::new()));
        }
        let text = unwrap_event_stream(&resp.body);
        let doc = json::parse(text.trim())
            .map_err(|e| format!("{method} → {}: reply is not JSON ({e}): {:.200}", self.url, text.trim()))?;
        if let Some(err) = doc.get("error") {
            let code = err.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("(no message)");
            return Err(format!("{method} → {}: JSON-RPC error {code}: {msg}", self.url));
        }
        Ok(doc.get("result").cloned().unwrap_or_else(|| json::object(Vec::new())))
    }

    /// The server's tool table.
    pub fn list_tools(&self) -> Result<Vec<Tool>, String> {
        self.ensure_initialized();
        let result = self.post("tools/list", json::object(Vec::new()), false)?;
        let items = result
            .get("tools")
            .and_then(|t| t.as_array())
            .ok_or_else(|| format!("{}: tools/list returned no \"tools\" array", self.url))?;
        Ok(items
            .iter()
            .map(|t| Tool {
                name: t.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                description: t
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                schema: t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json::object(Vec::new())),
            })
            .filter(|t| !t.name.is_empty())
            .collect())
    }

    /// Call a tool and return its text.  `isError` comes back as `Err` with the
    /// server's sentence, so a caller can print or feed it onward unchanged.
    pub fn call_text(&self, name: &str, arguments: Json) -> Result<String, String> {
        self.ensure_initialized();
        let params = json::object(vec![
            ("name".to_string(), json::string(name)),
            ("arguments".to_string(), arguments),
        ]);
        let result = self.post("tools/call", params, false)?;
        let mut parts = Vec::new();
        if let Some(items) = result.get("content").and_then(|c| c.as_array()) {
            for item in items {
                match item.get("type").and_then(|t| t.as_str()) {
                    // Only text is passed to a text model; anything else is
                    // named, so "the tool returned an image" is not silently an
                    // empty answer.
                    Some("text") => parts.push(
                        item.get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string(),
                    ),
                    Some(other) => parts.push(format!("[{other} content omitted]")),
                    None => {}
                }
            }
        }
        let text = parts.join("\n");
        if result.get("isError").and_then(|v| v.as_bool()) == Some(true) {
            return Err(if text.is_empty() {
                format!("{name}: tool reported an error with no message")
            } else {
                format!("{name}: {text}")
            });
        }
        Ok(text)
    }
}

/// Accept a reply that arrived as one or more `data:` frames rather than a bare
/// JSON body (the other framing Streamable HTTP allows, and what a server pushes
/// when it streams).  A bare body passes through untouched.
fn unwrap_event_stream(body: &str) -> std::borrow::Cow<'_, str> {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return std::borrow::Cow::Borrowed(body);
    }
    let mut joined = String::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            joined.push_str(rest.trim());
        }
    }
    std::borrow::Cow::Owned(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum_tool() -> (Tool, Handler) {
        (
            Tool {
                name: "sum".to_string(),
                description: "Add two numbers".to_string(),
                schema: object_schema(
                    vec![
                        ("a", param("number", "first addend")),
                        ("b", param("number", "second addend")),
                    ],
                    vec!["a", "b"],
                ),
            },
            Box::new(|args: &Json| {
                let a = args
                    .at(&["a"])
                    .and_then(|v| v.as_f64())
                    .ok_or("argument \"a\" is missing or not a number")?;
                let b = args
                    .at(&["b"])
                    .and_then(|v| v.as_f64())
                    .ok_or("argument \"b\" is missing or not a number")?;
                Ok(format!("{}", a + b))
            }),
        )
    }

    fn server() -> Server {
        let mut s = Server::new("test-mcp", "0");
        let (tool, h) = sum_tool();
        s.add(tool, h);
        s
    }

    /// `handle` is pure, so most of this file is testable with no socket.
    fn ask(s: &Server, body: &str) -> Json {
        json::parse(&s.handle(body).expect("a reply")).expect("a JSON-RPC reply")
    }

    #[test]
    fn initialize_advertises_the_tools_capability() {
        let doc = ask(
            &server(),
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );
        assert_eq!(doc.at(&["result", "protocolVersion"]).unwrap().as_str(), Some(PROTOCOL_VERSION));
        assert!(doc.at(&["result", "capabilities", "tools"]).is_some());
        assert_eq!(doc.at(&["result", "serverInfo", "name"]).unwrap().as_str(), Some("test-mcp"));
    }

    #[test]
    fn tools_list_carries_a_valid_json_schema() {
        let doc = ask(&server(), r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = doc.at(&["result", "tools"]).unwrap().as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].at(&["inputSchema", "type"]).unwrap().as_str(), Some("object"));
        assert_eq!(
            tools[0].at(&["inputSchema", "properties", "a", "description"]).unwrap().as_str(),
            Some("first addend")
        );
    }

    #[test]
    fn a_tool_answers_with_content_not_a_bare_value() {
        let doc = ask(
            &server(),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"sum","arguments":{"a":2,"b":3}}}"#,
        );
        assert_eq!(doc.at(&["result", "isError"]).unwrap().as_bool(), Some(false));
        let items = doc.at(&["result", "content"]).unwrap().as_array().unwrap();
        assert_eq!(items[0].at(&["type"]).unwrap().as_str(), Some("text"));
        assert_eq!(items[0].at(&["text"]).unwrap().as_str(), Some("5"));
    }

    #[test]
    fn a_handler_failure_is_a_result_with_is_error_so_the_model_can_read_it() {
        let doc = ask(
            &server(),
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"sum","arguments":{}}}"#,
        );
        assert!(doc.get("error").is_none(), "a tool failure must not be a JSON-RPC error");
        assert_eq!(doc.at(&["result", "isError"]).unwrap().as_bool(), Some(true));
        let text = doc.at(&["result", "content"]).unwrap().as_array().unwrap()[0]
            .at(&["text"])
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("\"a\""), "{text}");
    }

    #[test]
    fn an_unknown_tool_is_a_protocol_error_and_an_unknown_method_too() {
        let doc = ask(
            &server(),
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}"#,
        );
        assert_eq!(doc.at(&["error", "code"]).unwrap().as_f64(), Some(INVALID_PARAMS as f64));
        let doc = ask(&server(), r#"{"jsonrpc":"2.0","id":6,"method":"prompts/list"}"#);
        assert_eq!(doc.at(&["error", "code"]).unwrap().as_f64(), Some(METHOD_NOT_FOUND as f64));
    }

    #[test]
    fn a_notification_gets_no_reply_and_garbage_gets_a_parse_error() {
        assert!(server().handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let doc = json::parse(&server().handle("not json").unwrap()).unwrap();
        assert_eq!(doc.at(&["error", "code"]).unwrap().as_f64(), Some(PARSE_ERROR as f64));
    }

    #[test]
    fn openai_definition_is_the_shape_a_chat_completions_request_wants() {
        let tool = server().tool_names();
        let mut s = Server::new("t", "0");
        let (t, h) = sum_tool();
        s.add(t, h);
        assert_eq!(tool, vec!["sum".to_string()]);
        let listed = s.handle(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
        let wire = json::parse(&listed).unwrap();
        let advertised = &wire.at(&["result", "tools"]).unwrap().as_array().unwrap()[0];
        let openai = Tool {
            name: "sum".to_string(),
            description: "Add two numbers".to_string(),
            schema: advertised.at(&["inputSchema"]).unwrap().clone(),
        }
        .openai_definition();
        assert_eq!(openai.at(&["type"]).unwrap().as_str(), Some("function"));
        assert_eq!(openai.at(&["function", "name"]).unwrap().as_str(), Some("sum"));
        assert_eq!(
            openai.at(&["function", "parameters", "type"]).unwrap().as_str(),
            Some("object")
        );
    }

    #[test]
    fn an_event_stream_reply_is_unwrapped() {
        assert_eq!(unwrap_event_stream("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(
            unwrap_event_stream("event: message\r\ndata: {\"a\":1}\r\n\r\n"),
            "{\"a\":1}"
        );
    }

    /// The real test of using this crate rather than a dependency: our server and
    /// our client agree over a socket, including `initialize` and the notification
    /// that follows it.
    #[test]
    fn a_client_and_a_server_talk_over_a_socket() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let s = server();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => return,
                };
                // common-web's reader, because a request that arrives in two
                // reads must not be treated as two requests — and a fixture that
                // gets this wrong tests the fixture.
                let req = match common_web::read_request(&mut stream) {
                    Some(r) => r,
                    None => continue,
                };
                let body = req.body_text();
                let reply = match s.handle(&body) {
                    Some(r) => r,
                    None => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n",
                        );
                        continue;
                    }
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    reply.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.flush();
            }
        });

        let client = Client::new(format!("http://127.0.0.1:{port}/mcp"));
        let tools = client.list_tools().expect("tools/list");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "sum");
        let answer = client
            .call_text(
                "sum",
                json::object(vec![
                    ("a".to_string(), json::number_from_f64(20.0)),
                    ("b".to_string(), json::number_from_f64(22.0)),
                ]),
            )
            .expect("tools/call");
        assert_eq!(answer, "42");
        let err = client
            .call_text("sum", json::object(Vec::new()))
            .expect_err("isError must arrive as Err");
        assert!(err.contains("\"a\""), "{err}");
    }
}
