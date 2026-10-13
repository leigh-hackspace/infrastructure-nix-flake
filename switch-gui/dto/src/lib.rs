//! The wire types of switch-gui: what the API returns and what the SPA renders.
//!
//! One definition per shape, shared by the server and the wasm client so they
//! cannot disagree about a field name — the same role `gocardless-dashboard/dto`
//! plays.  Serde only: this compiles for `wasm32-unknown-unknown` too, so there
//! is no sqlx, no chrono and no wall-clock reading here.  Every timestamp is a
//! string Postgres formatted (UTC, via `to_char`) and every "how old is this"
//! number is computed by the server: a browser clock and a switch read time must
//! not be compared in the client.
//!
//! The shapes mirror `src/model.rs`, which mirrors the CLI's tables, with two
//! additions that only exist because the data now comes from a database instead
//! of a live read: [`Change`] (the write queue and audit trail) and the
//! freshness/error fields on [`SwitchSummary`] and [`Page`].
//!
//! Field names are camelCase on the wire (`rename_all`), which is what a JS/Dioxus
//! view reads naturally; `Intent`'s switch field is the one explicit rename, in
//! both directions of that rule — `switch` on the wire, `switch_name` in Rust.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `GET /api/state` — the index: every switch, how fresh it is, what is queued.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub switches: Vec<SwitchSummary>,
    /// The signed-in user, so the SPA can name it in the header.  Empty when
    /// auth is off (`--dev-user`/`--no-auth` on a loopback bind).
    pub user: String,
    /// False until at least one switch has been read once, so the SPA can say
    /// "reading…" rather than showing a table that looks like a switch with no
    /// ports.
    pub ready: bool,
}

/// One row of the index.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SwitchSummary {
    pub name: String,
    /// The address the sync dials (from `common/hosts.nix`, never a name: this
    /// is a tool for debugging a network, so it must not resolve anything).
    pub address: String,
    /// `show system`'s own `System Name`, which is normally the same string as
    /// `name` — and worth showing because when it differs, someone renamed the
    /// device and the table here is wrong.
    pub system_name: String,
    pub location: String,
    pub software: String,
    /// Ports and VLANs in the shadow copy.  Zero with `error` set means "never
    /// read successfully", which the SPA renders as a down switch.
    pub ports: i64,
    pub vlans: i64,
    /// When the shadow copy was written, UTC.
    pub fetched_at: Option<String>,
    /// Seconds between that write and now, by the server's clock.
    pub age_secs: Option<i64>,
    /// The last read failure, if the last read failed.  A stale snapshot with a
    /// visible error beats a page that looks healthy and is stale.
    pub error: Option<String>,
    /// `show` commands that failed during the last read: a partial snapshot.
    pub warnings: Vec<String>,
    /// Changes queued or in flight for this switch (the "pending sync" count).
    pub pending: i64,
    /// Whether the sync's session has Manager (writes) or only Operator — the
    /// same value the page shows, on the row that says whether writes are even
    /// possible.  `unknown` until the first read.
    pub privilege: String,
    /// Whether **this instance** refuses writes at all (`--read-only`).
    ///
    /// A different fact from `privilege`, which is about the switch: a read-only
    /// deployment with a Manager session could write and chooses not to, and the
    /// page has to say which of the two the operator is fighting.  The server
    /// refuses the intent either way; this is so the control is greyed out with a
    /// reason instead of failing after the click.
    #[serde(default)]
    pub read_only: bool,
}

/// `GET /api/switch?name=` — one switch: the shadow copy plus its queue.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub name: String,
    pub address: String,
    pub system_name: String,
    pub location: String,
    pub software: String,
    pub uptime: String,
    pub serial: String,
    pub fetched_at: Option<String>,
    pub age_secs: Option<i64>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    /// Whether the sync's session has Manager (writes) or only Operator.
    /// `unknown` until the first read: the privilege is a property of the live
    /// console, not of the shadow copy.
    ///
    /// The lowercase slug (`manager` / `operator`), not the page prose: the index
    /// and the page show the same fact, and a value a query can compare against
    /// beats text a client would have to pattern-match.
    pub privilege: String,
    /// Whether **this instance** refuses writes at all (`--read-only`) — see
    /// [`SwitchSummary::read_only`] for why that is not the same question as
    /// `privilege`.
    #[serde(default)]
    pub read_only: bool,
    pub vlans: Vec<Vlan>,
    pub ports: Vec<Port>,
    /// Everything still queued or in flight for this switch, newest first — the
    /// same rows `GET /api/pending` returns, joined in so one fetch draws the
    /// page.  The SPA marks a port row whose id appears in one of these (see
    /// [`Change::touches`]) and builds the "pending sync" notice from it.
    ///
    /// Failed rows are deliberately not here: they are out of the queue, and the
    /// port they wanted is the switch's state, not a pending one.  They show in
    /// the history panel (`GET /api/changes`).
    pub pending: Vec<Change>,
}

/// A VLAN as `show vlan` reports it.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Vlan {
    pub id: i64,
    pub name: String,
    /// `Port-based` or `Protocol-based`.
    pub status: String,
    pub jumbo: String,
}

/// Port counters, the two columns an operator actually scans for a bad cable.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Counters {
    pub errors_rx: String,
    pub drops_rx: String,
}

/// A port, merged from the five `show` commands the switch splits this across.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Port {
    /// The switch's own port identity: `1`, `9`, `47`, or `Trk1`.  A string
    /// because that is what the CLI takes and what its tables print — and
    /// because a trunk group sorts after the physical ports, which a number
    /// cannot express.
    pub id: String,
    /// From `show running-config`: the only command on this firmware that
    /// reports names for dynamically-learned ports (which on switch1 is every
    /// port).  Empty when the port has no name configured.
    pub name: String,
    /// Media, from `show spanning-tree`.
    pub media: String,
    /// `Forwarding`, `Disabled`, `Learning`, …
    pub stp_state: String,
    /// Set when `stp_state` came from this port's trunk group rather than from
    /// the port itself (`show spanning-tree` has a row for `Trk3` and none for
    /// its members).  The row says so.
    pub stp_state_from: String,
    /// Link state from the VLAN membership tables — the only per-port liveness
    /// the CLI gives.  `None` when the port is in no VLAN at all.
    pub link_up: Option<bool>,
    /// Non-empty when this port is in an LACP trunk.  The switch refuses to move
    /// a trunk member's VLAN membership, so the GUI shows the row locked instead
    /// of queueing a write that cannot work.
    pub trunk_group: String,
    /// `LACP`, `Trunk` or `Distributed`.
    pub trunk_type: String,
    /// The VLAN the port is `Untagged` in — its PVID, the access VLAN.
    pub native: Option<i64>,
    /// Every VLAN the port is `Tagged` in.
    pub tagged: Vec<i64>,
    /// `no shutdown` in the running config.  Read from `show running-config`,
    /// the only command that reports it, and the one write here that a snapshot
    /// can therefore be wrong about if that read failed.
    pub enabled: bool,
    pub counters: Option<Counters>,
}

impl Port {
    /// `5` or `5 · NAS2 Port 1` — the shortest label that fits a table cell.
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            self.id.clone()
        } else {
            format!("{} · {}", self.id, self.name)
        }
    }

    /// Whether the switch will let this row's VLAN membership be edited.
    ///
    /// Two kinds of row are not, and they are different kinds of not: a trunk
    /// **member** carries its group in [`Port::trunk_group`], and the switch itself
    /// refuses `vlan 225 tagged 7` for it; a trunk **group** is a row whose id is the
    /// token (`Trk1`), whose membership moves with its members — a command list this
    /// tool has never been pointed at a live trunk, so it does not offer it.
    ///
    /// The group test is on the id because that is the only thing the group's own row
    /// has: `trunk_group` is filled in on the *members* from `show trunks` and never
    /// on the group, so checking one and assuming it covers the other is how a
    /// group's row ends up looking like an ordinary port.
    pub fn membership_editable(&self) -> bool {
        self.trunk_group.is_empty() && !is_trunk_group(&self.id)
    }
}

/// One write: what was asked for, where it is in the queue, and what happened.
///
/// This is the audit trail and the write queue in one row, which is the point:
/// "who changed what, when, and did it reach the switch" is one query, and a
/// pending change is a row rather than browser state that a refresh loses.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: i64,
    pub switch_name: String,
    /// `port-name`, `port-state`, `port-members`, `vlan-name`, `vlan-members`,
    /// `vlan-delete`, `refresh`.
    pub kind: String,
    /// Ports this change touches, for marking rows.  Empty for a VLAN-only or a
    /// refresh change.
    pub ports: Vec<String>,
    /// One line for a human: `Port 7: untagged 9, tagged 7, 20`.
    pub summary: String,
    /// `pending` | `applying` | `applied` | `failed`.
    pub status: String,
    pub attempt: i32,
    pub max_attempts: i32,
    pub created_at: String,
    pub created_by: String,
    pub applied_at: Option<String>,
    /// What the switch said, when the last attempt failed.
    pub last_error: Option<String>,
    /// Set when this row was queued as the undo of change `id`.
    pub reverted_from: Option<i64>,
    /// What the shadow copy said **before** this change was queued, as a flat
    /// map of the keys that kind reads: `name`, `enabled`, `native`, `tagged`,
    /// `vlan-name`, `untagged-members`, `tagged-members`.
    ///
    /// Two jobs, both of which the audit trail needs.  One: a log line that says
    /// "set the name" is half a sentence, and "`GW Port 1` → `Uplink A`" is the
    /// thing someone asks six weeks later.  Two: it is what Revert queues from,
    /// which is why it is stored rather than recomputed — by the time anyone
    /// clicks Revert the switch may have moved again, and "undo" has to mean the
    /// state this change replaced, not whatever is there now.
    pub before: BTreeMap<String, String>,
    /// The CLI lines, one entry per port (the sync applies each entry as its own
    /// config-mode round trip).  Shown so an operator can read exactly what will
    /// be written before it goes out.
    pub commands: Vec<Vec<String>>,
    /// The operator's own note on the request, from [`Intent::note`].
    ///
    /// Stored rather than folded into `summary`, which is generated from the diff
    /// and has to stay a faithful description of the CLI lines: what a human
    /// believed is a different claim from what the switch was told, and the audit
    /// log is where those two are allowed to differ.  It reaches a switch only
    /// through this row — the CLI has no comment syntax, so nothing here is ever
    /// sent to the console.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Change {
    /// Whether this change affects one port row.
    pub fn touches(&self, port_id: &str) -> bool {
        self.ports.iter().any(|p| p == port_id)
    }

    /// Still to reach the switch (as opposed to applied or given up on).
    pub fn is_open(&self) -> bool {
        self.status == "pending" || self.status == "applying"
    }

    /// Out of the queue: applied, or failed for good.
    pub fn is_settled(&self) -> bool {
        !self.is_open()
    }
}

/// `GET /api/changes` / `GET /api/changes?switch=`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub changes: Vec<Change>,
}

/// `POST /api/intent` — what the operator asked for, before it becomes CLI.
///
/// One endpoint for every edit, with the fields each kind needs optional, so the
/// SPA does not have to know which of six routes a given form posts to.  The
/// server validates that a kind's fields are present and legal for this switch
/// (`intent.rs`), which is also where the shadow copy is diffed.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Intent {
    /// Which switch.  `switch` is a Rust keyword, so the field is `switch_name`
    /// and the wire name stays `switch`.
    #[serde(rename = "switch", default)]
    pub switch_name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub port: Option<String>,
    /// New name for `port-name`; new VLAN name for `vlan-name`.
    #[serde(default)]
    pub name: Option<String>,
    /// Wanted untagged VLAN as a string, because that is what a form field is.
    /// Absent means "leave the native VLAN alone"; `""` is not special here —
    /// send it absent.
    #[serde(default)]
    pub untagged: Option<String>,
    /// Wanted tagged memberships — the **whole wanted set**, not a delta.  The
    /// server diffs it against the shadow copy, which is what makes a re-sent
    /// form idempotent (and a double-submit harmless).
    #[serde(default)]
    pub tagged: Option<Vec<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub vlan: Option<i64>,
    /// Free text for the audit log.
    #[serde(default)]
    pub note: Option<String>,
    /// For `POST /api/change/retry` and `/api/change/forget`: the row to act on.
    #[serde(default)]
    pub id: Option<i64>,
}

/// The switch's own token for a link-aggregation group: `Trk` and digits.
///
/// The one copy of this rule.  It lives here because both halves of the tool need
/// it and this crate is the only place they meet: the server reaches it through
/// [`crate::model::is_trunk_group`], and the SPA through [`Port::membership_editable`].
pub fn is_trunk_group(id: &str) -> bool {
    match id.strip_prefix("Trk") {
        Some(n) => !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

/// `POST /api/intent` reply: the queued row, or why it was refused.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Ack {
    pub fn queued(change: Change) -> Ack {
        Ack {
            ok: true,
            change: Some(change),
            error: None,
        }
    }

    /// Nothing to do is a success with no row: the membership editor sends the
    /// whole wanted row, so "already in that state" is an ordinary answer.
    pub fn noop() -> Ack {
        Ack {
            ok: true,
            change: None,
            error: None,
        }
    }

    pub fn refused(error: impl Into<String>) -> Ack {
        Ack {
            ok: false,
            change: None,
            error: Some(error.into()),
        }
    }
}

/// `GET /api/whoami`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Whoami {
    pub user: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `link_up` is an `Option<bool>` because "in no VLAN" is not "down", and the
    /// row renders the two differently.  Serde must keep all three states.
    #[test]
    fn link_up_keeps_three_states() {
        let up = Port {
            link_up: Some(true),
            ..Default::default()
        };
        let down = Port {
            link_up: Some(false),
            ..Default::default()
        };
        let unknown = Port::default();
        assert!(serde_json::to_string(&up)
            .unwrap()
            .contains("\"linkUp\":true"));
        assert!(serde_json::to_string(&down)
            .unwrap()
            .contains("\"linkUp\":false"));
        assert!(serde_json::to_string(&unknown)
            .unwrap()
            .contains("\"linkUp\":null"));
    }

    /// `switch` is a Rust keyword; the struct field is `switch_name` and the wire
    /// name has to stay `switch` or every intent the SPA posts names no switch.
    #[test]
    fn an_intent_parses_the_switch_key() {
        let i: Intent = serde_json::from_str(r#"{"switch":"switch1","kind":"refresh"}"#).unwrap();
        assert_eq!(i.switch_name, "switch1");
        assert_eq!(i.kind, "refresh");
        assert!(i.port.is_none());
        assert!(i.tagged.is_none());
    }

    /// A form that leaves the tagged column alone posts `"tagged":[]` (nothing
    /// wanted) and a form that does not send the field posts nothing at all — the
    /// server tells those apart, so deserialisation must not invent one.
    #[test]
    fn absent_and_empty_tagged_are_different_things() {
        let none: Intent = serde_json::from_str(r#"{"switch":"s","kind":"port-name"}"#).unwrap();
        assert!(none.tagged.is_none());
        let empty: Intent =
            serde_json::from_str(r#"{"switch":"s","kind":"port-name","tagged":[]}"#).unwrap();
        assert_eq!(empty.tagged, Some(vec![]));
    }

    #[test]
    fn a_change_knows_which_rows_it_touches_and_whether_it_is_open() {
        let c = Change {
            ports: vec!["7".into(), "9".into()],
            status: "pending".into(),
            ..Default::default()
        };
        assert!(c.touches("7"));
        assert!(!c.touches("27"));
        assert!(c.is_open());
        assert!(!c.is_settled());
        for status in ["applying", "applied", "failed"] {
            let c = Change {
                status: status.into(),
                ..Default::default()
            };
            assert_eq!(c.is_open(), status == "applying", "{status}");
            assert_eq!(c.is_settled(), status != "applying", "{status}");
        }
    }

    /// A port with no name is still a port: the label is what the table shows.
    #[test]
    fn port_labels_fall_back_to_the_number() {
        let named = Port {
            id: "22".into(),
            name: "AP3".into(),
            ..Default::default()
        };
        let blank = Port {
            id: "23".into(),
            ..Default::default()
        };
        assert_eq!(named.label(), "22 · AP3");
        assert_eq!(blank.label(), "23");
        assert!(!Port {
            id: "47".into(),
            trunk_group: "Trk3".into(),
            ..Default::default()
        }
        .membership_editable());
        assert!(Port {
            id: "47".into(),
            ..Default::default()
        }
        .membership_editable());
        // The group's own row: no `trunk_group` of its own, and its membership is the
        // union of its members'.  `Trk` on its own is not a token, so it stays a row
        // the tool is willing to talk about.
        assert!(!Port {
            id: "Trk1".into(),
            ..Default::default()
        }
        .membership_editable());
        assert!(!Port {
            id: "Trk12".into(),
            ..Default::default()
        }
        .membership_editable());
        assert!(Port {
            id: "Trk".into(),
            ..Default::default()
        }
        .membership_editable());
    }

    /// The SPA reads `portType`-style camelCase for multi-word fields; a rename
    /// that drops the convention would blank a column rather than fail.
    #[test]
    fn multi_word_fields_are_camel_case_on_the_wire() {
        let p = Port {
            stp_state: "Forwarding".into(),
            trunk_group: "Trk1".into(),
            ..Default::default()
        };
        let j = serde_json::to_string(&p).unwrap();
        assert!(j.contains("\"stpState\":\"Forwarding\""), "{j}");
        assert!(j.contains("\"trunkGroup\":\"Trk1\""), "{j}");
        assert!(!j.contains("stp_state"), "{j}");
    }

    /// `before` is a map of strings rather than a nested document: the audit log
    /// prints it and Revert reads it back, and both want the same flat encoding
    /// (`tagged` as `"7,20"`, `enabled` as `"true"`).
    #[test]
    fn before_is_a_flat_string_map() {
        let mut before = BTreeMap::new();
        before.insert("name".to_string(), "GW Port 1".to_string());
        before.insert("tagged".to_string(), "7,20".to_string());
        let c = Change {
            before,
            ..Default::default()
        };
        let j = serde_json::to_string(&c).unwrap();
        // No closing brace in the expected text: `commands` follows `before` and is
        // always serialised (an empty list is a real answer — a change that has not
        // run yet has no CLI lines), so `before` is never the last key.
        assert!(
            j.contains(r#""before":{"name":"GW Port 1","tagged":"7,20"}"#),
            "{j}"
        );
        let back: Change = serde_json::from_str(&j).unwrap();
        assert_eq!(back.before.get("tagged").map(String::as_str), Some("7,20"));
        assert!(Change::default().before.is_empty());
    }
}
