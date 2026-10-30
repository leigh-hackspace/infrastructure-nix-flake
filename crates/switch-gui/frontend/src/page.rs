//! `GET /api/switch` — one switch: the header, the VLAN table, the port table and
//! the VLAN membership matrix.
//!
//! One fetch draws the whole page, and it is a read of Postgres: nothing here
//! waits for a Telnet round trip, and nothing here can make a switch answer
//! faster.  The rows are the shadow copy, so every one of them carries the age of
//! the copy it came from, and the pending changes arrive in the same document
//! ([`dto::Page::pending`]) rather than in a second request that could disagree
//! with the snapshot about what is queued.
//!
//! ## What an edit is
//!
//! A click never touches a switch.  It builds an [`dto::Intent`], `POST`s it to
//! `/api/intent`, and the server either refuses with a sentence (`a trunk member's
//! VLAN membership cannot be changed`) or queues a row.  The banner then says
//! *queued*, and the port stays marked until the queue stops saying so — which is
//! why that marking is derived from the fetched rows on every poll rather than kept
//! as local state: a change made in another tab, or by another operator, marks the
//! same row.
//!
//! The membership matrix is the one place that holds more than a click's worth of
//! state: the row the operator has built up lives in [`RowEdit`] until they press
//! Apply.  It is an **overlay** on the snapshot, so the poll keeps running
//! underneath it — a refresh can neither lose a tick nor invent one.

use common_ui::checkbox::{Checkbox, CheckboxIndicator, CheckboxState};
use dioxus::prelude::*;
use std::collections::BTreeMap;
use switch_gui_dto as dto;

use crate::queue::Queue;
use crate::{
    age, any_dirty, dash, enc, get_json, post_json, row_dirty, row_from_snapshot, row_wanted, Nav,
    Note, RowEdit, UserLine, POLL_SECS,
};

/// One switch's dashboard.
#[component]
pub fn SwitchView(name: String, route: Signal<Option<String>>, user: Signal<String>) -> Element {
    // The banner every action writes its result into.
    let note = use_signal(|| Option::<(bool, String)>::None);
    // The membership editor's overlay: only the rows the operator has touched.
    let edits = use_signal(BTreeMap::<String, RowEdit>::new);
    // Every text input on the page, keyed by its DOM id (`pn7`, `vn225`).  One map
    // because they are all the same question — "what did the operator type here" —
    // and because the keys cannot collide.
    let inputs = use_signal(BTreeMap::<String, String>::new);
    // The Refresh button disables itself while its request is in flight: the sync
    // coalesces a second request anyway, but a button that can be pressed twice
    // should not look like it did nothing.
    let busy = use_signal(|| false);

    // The poll.  `tick` is the only thing the resource watches, so the fetch stays
    // a plain function of the switch's name and a bump re-reads it.
    let mut tick = use_signal(|| 0u64);
    use_effect(move || {
        spawn(async move {
            loop {
                gloo_timers::future::sleep(std::time::Duration::from_secs(POLL_SECS)).await;
                tick += 1;
            }
        });
    });

    let shadow = use_resource({
        let name = name.clone();
        move || {
            let _ = tick();
            let n = name.clone();
            async move { get_json::<dto::Page>(&format!("/api/switch?switch={}", enc(&n))).await }
        }
    });

    let page = shadow.read().as_ref().cloned().and_then(|r| r.ok());
    let fetch_err = shadow
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().err().cloned());

    let dirty = any_dirty(&edits.read());
    let queued = page
        .as_ref()
        .map(|p| p.pending.len())
        .unwrap_or_default();

    rsx! {
        header {
            div {
                h1 { "{head(&page, &name)}" }
                div { class: "muted", "{subhead(&page)}" }
            }
            div { class: "spacer" }
            {badges(&page, busy, note)}
            a { href: "/logout", button { "Sign out" } }
        }
        UserLine { user }
        Note { note }
        Nav { current: name.clone(), route }
        if let Some(e) = fetch_err {
            div { class: "note err", "{e}" }
        }
        if let Some(p) = &page {
            {notices(p)}
            {cards(p)}
        }
        if queued > 0 {
            div { class: "note info",
                b { "Pending sync" }
                " — {queued} change(s) queued for this switch. Each port and VLAN they touch is \
                 marked below until the queue is empty."
            }
        }
        if let Some(p) = &page {
            h2 { "VLANs" }
            {vlan_table(p, inputs, note)}
            h2 { "Ports" }
            {port_table(p, inputs, note)}
            h2 { "VLAN membership" }
            {matrix(p, edits, note)}
            h2 { "Queue and audit log" }
            Queue { name: name.clone(), note }
            if dirty {
                div { class: "note info",
                    "Rows highlighted are what you have changed in the form. Nothing is sent until \
                     you press Apply on the row."
                }
            }
            p { class: "muted",
                "switch-gui · read over Telnet, written with the ProCurve CLI, audited in Postgres. "
                a { href: "/api/export?switch={enc(&name)}", "JSON" }
            }
        } else {
            p { class: "spin", "reading the shadow copy…" }
        }
    }
}

// ------------------------------------------------------------------- header

/// The `h1`: the switch's own `System Name` when it has one, the configured name
/// otherwise.  When the two differ the cards below show both — that is someone
/// renaming a device out from under this table, and the address is what still
/// identifies it.
fn head(page: &Option<dto::Page>, fallback: &str) -> String {
    match page {
        Some(p) if !p.system_name.is_empty() => p.system_name.clone(),
        Some(p) => p.name.clone(),
        None => fallback.to_string(),
    }
}

fn subhead(page: &Option<dto::Page>) -> String {
    match page {
        None => "ProCurve, read over Telnet, shadowed in Postgres".to_string(),
        Some(p) => format!(
            "{} · {} · up {}",
            dash(&p.location),
            dash(&p.software),
            dash(&p.uptime)
        ),
    }
}

/// The header's right-hand side: what this session may do, how old the copy is,
/// and the two buttons that reach outside the page.
fn badges(
    page: &Option<dto::Page>,
    mut busy: Signal<bool>,
    mut note: Signal<Option<(bool, String)>>,
) -> Element {
    let Some(p) = page else {
        return rsx! { span { class: "badge", "{age(None)}" } };
    };
    let manager = p.privilege == "manager";
    let privilege = p.privilege.clone();
    let address = p.address.clone();
    let name = p.name.clone();
    let age_text = format!("cache {}", age(p.age_secs));

    rsx! {
        if manager {
            span { class: "badge mgr", title: "the CLI session is in the Manager context: configuration is accepted", "Manager" }
        } else {
            span { class: "badge ro", title: "the CLI session is in the Operator context: every write is refused", "{privilege} · read-only" }
        }
        if p.read_only {
            span { class: "badge ro", title: "writes are disabled on this deployment (--read-only)", "writes off" }
        }
        span { class: "badge", title: "when the shadow copy was last written", "{age_text}" }
        button {
            disabled: *busy.read(),
            onclick: move |_| {
                busy.set(true);
                let sw = name.clone();
                spawn(async move {
                    let r = send(intent("refresh", &sw)).await;
                    note.set(Some(r));
                    busy.set(false);
                });
            },
            title: "ask the sync to re-read this switch now (it does not block this page)",
            "Refresh"
        }
        // http, not https: measured on 10.3.1.2, the switch's own web UI does not
        // complete a TLS handshake at all.
        a { href: "http://{address}", button { "Switch UI" } }
    }
}

/// Everything the last read wants the operator to know, in the order they need it:
/// the read failed, some commands failed, the session cannot write, writes are off.
fn notices(p: &dto::Page) -> Element {
    let warnings = p.warnings.clone();
    rsx! {
        if let Some(e) = &p.error {
            div { class: "note err",
                b { "Last read failed" }
                " {e}"
                if p.ports.is_empty() {
                    " — nothing has been read from this switch, so every table below is empty."
                } else {
                    " — the tables below are the last good copy."
                }
            }
        }
        if !warnings.is_empty() {
            div { class: "note err",
                b { "Some commands failed" }
                ul { for w in warnings.iter() { li { "{w}" } } }
            }
        }
        if p.privilege == "operator" && !p.read_only {
            div { class: "note err",
                b { "Read-only session." }
                " The switch refused `enable`, so this login is in the Operator context and cannot \
                 configure anything. The enable credentials default to the login account, so this \
                 means they were refused."
            }
        }
        if p.read_only {
            div { class: "note info",
                b { "This instance runs with --read-only." }
                " It reads the switches and refuses every write before queueing it."
            }
        }
    }
}

/// The `show system` facts worth one card each.  Blank ones are skipped: a card
/// that says "—" for a field this firmware does not report is noise.
fn cards(p: &dto::Page) -> Element {
    let mut cells: Vec<(String, String)> = Vec::new();
    for (k, v) in [
        ("System name", &p.system_name),
        ("Address", &p.address),
        ("Location", &p.location),
        ("Revision", &p.software),
        ("Serial", &p.serial),
        ("Uptime", &p.uptime),
    ] {
        if !v.is_empty() {
            cells.push((k.to_string(), v.clone()));
        }
    }
    cells.push((
        "Shadow copy".to_string(),
        p.fetched_at
            .clone()
            .unwrap_or_else(|| "not read yet".to_string()),
    ));
    rsx! {
        div { class: "grid",
            for (k , v) in cells {
                div { class: "card", b { "{k}" }, "{v}" }
            }
        }
    }
}

// ----------------------------------------------------------------- VLAN table

/// The VLAN table: what `show vlan` says, plus the members this snapshot puts on
/// each VLAN (computed from the ports, which is the only place the CLI's five
/// tables agree).  Renaming and deleting live here; membership does not — that is
/// the matrix's job, because a per-VLAN port picker cannot show the native VLAN or
/// a trunk.
fn vlan_table(
    p: &dto::Page,
    inputs: Signal<BTreeMap<String, String>>,
    note: Signal<Option<(bool, String)>>,
) -> Element {
    let editable = can_write(p);
    if p.vlans.is_empty() {
        return rsx! { p { class: "note err", "No VLANs were read." } };
    }
    rsx! {
        table {
            thead {
                tr {
                    th { "VLAN" }
                    th { "Name" }
                    th { "Status" }
                    th { "Untagged" }
                    th { "Tagged" }
                    th {}
                }
            }
            tbody {
                for v in p.vlans.iter() {
                    {
                        let id = v.id;
                        let key = format!("vn{id}");
                        let name = v.name.clone();
                        let untagged = members(p, id, false);
                        let tagged = members(p, id, true);
                        let field = text_input(&key, &name, inputs);
                        // A dioxus event handler is `'static`, so each one owns its
                        // own copies of what it needs from this row — including the
                        // ones that look like they could share.
                        let sw_rename = p.name.clone();
                        let key_rename = key.clone();
                        let sw_delete = p.name.clone();
                        let mut note = note;
                        rsx! {
                            tr {
                                td { class: "n", b { "{id}" } }
                                td { "{dash(&name)}" }
                                td { class: "muted", "{dash(&v.status)}" }
                                td { class: "muted", "{untagged}" }
                                td { class: "muted", "{tagged}" }
                                td {
                                    div { class: "row",
                                        {field}
                                        button {
                                            disabled: !editable,
                                            onclick: move |_| {
                                                let wanted = typed(&inputs, &key_rename);
                                                let mut i = intent("vlan-name", &sw_rename);
                                                i.vlan = Some(id);
                                                i.name = Some(wanted);
                                                spawn(async move {
                                                    let r = send(i).await;
                                                    note.set(Some(r));
                                                });
                                            },
                                            "Rename"
                                        }
                                        if id != 1 {
                                            button {
                                                class: "danger",
                                                disabled: !editable,
                                                title: "the switch refuses this while ports are still on the VLAN",
                                                onclick: move |_| {
                                                    let mut i = intent("vlan-delete", &sw_delete);
                                                    i.vlan = Some(id);
                                                    spawn(async move {
                                                        let r = send(i).await;
                                                        note.set(Some(r));
                                                    });
                                                },
                                                "Delete"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The ports on one VLAN, as a comma list.  `tagged` picks the column; the
/// untagged one is the native VLAN.
fn members(p: &dto::Page, vlan: i64, tagged: bool) -> String {
    let list = p
        .ports
        .iter()
        .filter(|port| {
            if tagged {
                port.tagged.contains(&vlan)
            } else {
                port.native == Some(vlan)
            }
        })
        .map(|port| port.id.clone())
        .collect::<Vec<_>>();
    dash(&list.join(", ")).to_string()
}

// ----------------------------------------------------------------- Port table

/// The port table: the merge of the five `show` commands the switch splits a
/// port's state across.  The only writes here are the name and
/// `shutdown`/`no shutdown`; VLAN membership deliberately lives in the matrix,
/// which is the one control that knows about tagged memberships.
fn port_table(
    p: &dto::Page,
    inputs: Signal<BTreeMap<String, String>>,
    note: Signal<Option<(bool, String)>>,
) -> Element {
    let editable = can_write(p);
    rsx! {
        table {
            thead {
                tr {
                    th { "Port" }
                    th { "Name" }
                    th { "Media" }
                    th { "Link" }
                    th { "STP" }
                    th { "Trunk" }
                    th { "VLAN (untagged)" }
                    th { "Tagged" }
                    th { class: "n", "Rx err" }
                    th { class: "n", "Drops" }
                    th { class: "n", "Shut" }
                    th {}
                }
            }
            tbody {
                for port in p.ports.iter() {
                    {
                        let id = port.id.clone();
                        let key = format!("pn{id}");
                        let state = port.stp_state.clone();
                        let state_cls = if state.is_empty() {
                            "muted"
                        } else if state.eq_ignore_ascii_case("forwarding") {
                            "up"
                        } else {
                            "down"
                        };
                        let state_text = dash(&state).to_string();
                        let media = dash(&port.media).to_string();
                        let trunk = if port.trunk_group.is_empty() {
                            "—".to_string()
                        } else {
                            format!("{} · {}", port.trunk_group, port.trunk_type)
                        };
                        let untagged = port
                            .native
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "—".into());
                        let tagged = if port.tagged.is_empty() {
                            "—".to_string()
                        } else {
                            port.tagged
                                .iter()
                                .map(|v| v.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        };
                        let errs = port
                            .counters
                            .as_ref()
                            .map(|c| c.errors_rx.trim().to_string())
                            .unwrap_or_else(|| "—".into());
                        let drops = port
                            .counters
                            .as_ref()
                            .map(|c| c.drops_rx.trim().to_string())
                            .unwrap_or_else(|| "—".into());
                        // Highlight a port with errors rather than burying it in a column.
                        let err_cls = if errs == "0" || errs == "—" { "" } else { " err" };
                        // A trunk member's state belongs to the bundle; say so rather than
                        // imply the port was measured.
                        let inherited = if port.stp_state_from.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "state of {}, inherited: this firmware reports trunk members only \
                                 as a group",
                                port.stp_state_from
                            )
                        };
                        let (link_cls, link_text) = match port.link_up {
                            Some(true) => ("up", "Up"),
                            Some(false) => ("down", "Down"),
                            None => ("muted", "—"),
                        };
                        // The rows still queued for this port: what makes the difference
                        // between "the switch is like this" and "we asked for this".
                        let queued = p
                            .pending
                            .iter()
                            .filter(|c| c.touches(&id))
                            .map(|c| format!("{} · {}", c.status, c.summary))
                            .collect::<Vec<_>>();
                        let queued_title = queued.join("\n");
                        let pending = !queued.is_empty();
                        let field = text_input(&key, &port.name, inputs);
                        // One set of copies per handler (see the VLAN rows).
                        let sw_name = p.name.clone();
                        let key_name = key.clone();
                        let id_name = id.clone();
                        let sw_state = p.name.clone();
                        let id_state = id.clone();
                        let up = port.enabled;
                        let mut note = note;
                        rsx! {
                            tr {
                                class: if pending { "dirty" } else { "" },
                                td { class: "n", b { "{id}" } }
                                td { "{dash(&port.name)}" }
                                td { class: "muted", "{media}" }
                                td { class: "{link_cls}", title: "{inherited}", "{link_text}" }
                                td { class: "{state_cls}", title: "{inherited}", "{state_text}" }
                                td { class: "muted", "{trunk}" }
                                td { "{untagged}" }
                                td { class: "muted", "{tagged}" }
                                td { class: "n{err_cls}", "{errs}" }
                                td { class: "n", "{drops}" }
                                td {
                                    class: if up { "muted".to_string() } else { "warn".to_string() },
                                    title: "`shutdown` present in the running config",
                                    if up { "—" } else { "shut" }
                                }
                                td {
                                    class: "apply",
                                    title: "{queued_title}",
                                    if pending { span { class: "badge pending", "pending" } }
                                    div { class: "row",
                                        {field}
                                        button {
                                            disabled: !editable,
                                            onclick: move |_| {
                                                let wanted = typed(&inputs, &key_name);
                                                let mut i = intent("port-name", &sw_name);
                                                i.port = Some(id_name.clone());
                                                i.name = Some(wanted);
                                                spawn(async move {
                                                    let r = send(i).await;
                                                    note.set(Some(r));
                                                });
                                            },
                                            "Name"
                                        }
                                        button {
                                            class: if up { "danger" } else { "" },
                                            disabled: !editable,
                                            title: if up { "shutdown this port" } else { "no shutdown: bring the port up" },
                                            onclick: move |_| {
                                                let mut i = intent("port-state", &sw_state);
                                                i.port = Some(id_state.clone());
                                                i.enabled = Some(!up);
                                                spawn(async move {
                                                    let r = send(i).await;
                                                    note.set(Some(r));
                                                });
                                            },
                                            if up { "Shut" } else { "Up" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// --------------------------------------------------------- Membership matrix

/// The VLAN membership editor: one row per port, one vertical column per VLAN.
///
/// Why a matrix and not a per-VLAN port list, which the VLAN table above already
/// is: the switch's own model is VLAN-first — `vlan 225 tagged 7,9-11` is a
/// statement about a VLAN and a pile of ports — but an operator at a patch panel
/// thinks per port.  This is the per-port question ("which VLANs is 7 in, and
/// which one is native?") answered on one screen.
///
/// The form sends the **whole wanted row**, not the click that changed it, and the
/// server diffs it against the shadow copy.  That is what makes a double-submit
/// harmless and what stops a half-edited row from being sent, and it is why a row
/// is marked `dirty` the moment it stops matching the switch: this page is a cache,
/// so it shows what it is *about to change* instead of pretending it *is* the
/// switch.  Nothing is queued until Apply, and the poll afterwards replaces the
/// row with what the switch says.
///
/// The VLAN names are vertical because there are ten VLANs and fifty ports: a
/// horizontal header needs roughly 90 px per column and the table stops fitting
/// the screen, while a vertical one costs the width of a checkbox.
///
/// The form keeps one switch rule true as it is edited: **a port is never tagged
/// in its own native VLAN**.  The column of the row's native VLAN is therefore not
/// tickable, and moving the native VLAN off a ticked VLAN unticks it.  Both are
/// things the switch enforces anyway (`commands::port_members` refuses the other
/// state), but a form that offers an impossible row only trades a click for a
/// refusal the operator has to go looking for.
fn matrix(
    p: &dto::Page,
    edits: Signal<BTreeMap<String, RowEdit>>,
    note: Signal<Option<(bool, String)>>,
) -> Element {
    let editable = can_write(p);
    let vlans: Vec<(i64, String)> = p
        .vlans
        .iter()
        .map(|v| (v.id, format!("{} · {}", v.id, dash(&v.name))))
        .collect();
    rsx! {
        p { class: "muted",
            "Tick the VLANs a port is ", b { "tagged" }, " in and pick its ", b { "untagged" },
            " (native, access) VLAN. Only the rows you have changed are sent, and only when you \
             press Apply."
        }
        table { class: "mx",
            thead {
                tr {
                    th { "Port" }
                    th { "Name" }
                    th { "Untagged" }
                    for (vid , label) in vlans.iter() {
                        th { class: "v", abbr { title: "VLAN {vid} — {label}", "{label}" } }
                    }
                    th {}
                }
            }
            tbody {
                for port in p.ports.iter() {
                    {
                        let id = port.id.clone();
                        let snapshot = row_from_snapshot(port);
                        let wanted = row_wanted(&edits.read(), port);
                        let dirty = row_dirty(&edits.read(), port);
                        let queued = p.pending.iter().any(|c| c.touches(&id));
                        // Two kinds of "trunk", and both are locked: a **member** carries the
                        // group in `trunk_group` and the switch refuses to move it on its own;
                        // the **group itself** is a row whose id is the token (`Trk1`), and its
                        // membership moves with its members — a command list this tool has never
                        // been pointed at a live trunk, so it does not offer it.
                        let locked = !editable || !port.membership_editable();
                        let why = if !port.trunk_group.is_empty() {
                            format!(
                                "in LACP group {} — the switch will not change the VLAN membership \
                                 of a trunk member",
                                port.trunk_group
                            )
                        } else if !port.membership_editable() {
                            "a link-aggregation group: its membership moves with its members, which \
                             this tool does not attempt"
                                .to_string()
                        } else if !editable {
                            "this session cannot write".to_string()
                        } else {
                            String::new()
                        };
                        let native = wanted.native.clone();
                        // One set of copies per handler, and the row itself is cloned
                        // for the dropdown: a dioxus handler is `'static`, so it cannot
                        // borrow `p` (or the row it is drawing).
                        let mut ed_select = edits.clone();
                        let id_select = id.clone();
                        let row = port.clone();
                        let mut ed_apply = edits.clone();
                        let id_apply = id.clone();
                        let sw_apply = p.name.clone();
                        // For putting the row back when the server refuses it.
                        let ed_back = edits.clone();
                        let id_back = id.clone();
                        let mut note = note;
                        rsx! {
                            tr {
                                class: if dirty { "dirty" } else { "" },
                                // The test hooks the headless suite selects on
                                // (see `switch-gui/tests/README.md`), in the same
                                // spirit as filestore's `data-fs-*`: a port is
                                // identified by the switch's own id, and a VLAN
                                // column by its id, so a test does not have to
                                // count `td`s against a VLAN order that comes from
                                // the device.
                                "data-port": id.clone(),
                                td { class: "n", b { "{id}" } }
                                td { title: "{why}", "{dash(&port.name)}" }
                                td {
                                    select {
                                        disabled: locked,
                                        onchange: move |e| {
                                            let v = e.value();
                                            let mut w = wanted_of(&ed_select, &row);
                                            // A port has one native VLAN and can never be tagged
                                            // in it, so moving the native takes that tick with it.
                                            // The switch does the move in one go (`no vlan X tagged`
                                            // then `vlan X untagged`, which is exactly the order
                                            // `commands::port_members` emits); a row that asked for
                                            // both halves at once would only be a refusal.
                                            w.tagged.retain(|x| x.to_string() != v);
                                            w.native = v;
                                            ed_select.write().insert(id_select.clone(), w);
                                        },
                                        for (vid , label) in vlans.iter() {
                                            // `selected` on the option rather than `value` on the
                                            // select: the interpreter assigns `node.value` to the
                                            // select before the options are appended, which selects
                                            // nothing.
                                            option {
                                                selected: native == vid.to_string(),
                                                value: "{vid}",
                                                "{label}"
                                            }
                                        }
                                        if port.native.is_none() {
                                            option { selected: native.is_empty(), value: "", "— not set —" }
                                        }
                                    }
                                }
                                for (vid , _label) in vlans.iter() {
                                    {
                                        let vid = *vid;
                                        let on = wanted.tagged.contains(&vid);
                                        // The row's own native VLAN is the one column this port can
                                        // never join: `vlan 225 tagged 2` on a port that is
                                        // *untagged* in 225 is a state the switch has no room for,
                                        // and it refuses it.  Offered anyway, the tick looked legal,
                                        // the Apply was refused, and the only answer was a banner at
                                        // the top of a fifty-row page.  The server still checks it
                                        // (`commands::port_members`); this is only so the page never
                                        // asks for what it knows will be refused.
                                        //
                                        // A tick that is already there is never locked away: if a
                                        // shadow copy ever does show a port tagged in its own native
                                        // VLAN, taking it out has to stay possible.
                                        let native_here = !on
                                            && !native.is_empty()
                                            && native == vid.to_string();
                                        // What the cell is, and why it is locked — the two things
                                        // the widget cannot know.  They used to live only in a
                                        // `title`, which is a pointer-only channel: `aria-label`
                                        // names the cell and `aria-describedby` points at this text,
                                        // so the same sentence reaches a screen reader and a
                                        // keyboard user.
                                        let cell_why = if native_here {
                                            format!(
                                                "VLAN {vid} is this port's untagged (native) VLAN — \
                                                 a port cannot be tagged in the VLAN it is untagged \
                                                 in.  Change Untagged first."
                                            )
                                        } else if on {
                                            "tagged".to_string()
                                        } else {
                                            "not tagged".to_string()
                                        };
                                        let name = format!("port {id}, VLAN {vid}");
                                        // An id a CSS selector can actually take: `#23-228` is a
                                        // valid HTML id but not a valid selector, so the suite had to
                                        // reach it through `getElementById`. `why-` makes the
                                        // reference readable in both.
                                        let why_id = format!("why-{id}-{vid}");
                                        rsx! {
                                            Cell {
                                                edits,
                                                port: id.clone(),
                                                vlan: vid,
                                                snap: snapshot.clone(),
                                                locked: locked || native_here,
                                                name,
                                                reason: cell_why,
                                                why_id,
                                            }
                                        }
                                    }
                                }
                                td { class: "apply",
                                    if queued {
                                        span { class: "badge pending", title: "a change for this port is queued", "pending" }
                                    }
                                    if !locked {
                                        button {
                                            disabled: !dirty,
                                            "data-apply": id_apply.clone(),
                                            onclick: move |_| {
                                                let Some(w) = ed_apply.read().get(&id_apply).cloned() else {
                                                    return;
                                                };
                                                let mut i = intent("port-members", &sw_apply);
                                                i.port = Some(id_apply.clone());
                                                i.untagged = Some(w.native.clone());
                                                i.tagged = Some(
                                                    w.tagged.iter().map(|v| v.to_string()).collect(),
                                                );
                                                // Drop the overlay before the reply: the queue
                                                // row is the truth from here on, and the poll marks
                                                // the port from it.  It also disables Apply for the
                                                // round trip, which is what stops a double-click
                                                // queueing the same write twice.
                                                ed_apply.write().remove(&id_apply);
                                                let mut ed_back = ed_back.clone();
                                                let id_back = id_back.clone();
                                                spawn(async move {
                                                    let r = send(i).await;
                                                    if !r.0 {
                                                        // Refused, so nothing was queued and the
                                                        // queue row is *not* the truth: the row goes
                                                        // back exactly as the operator left it rather
                                                        // than snapping to the switch and losing the
                                                        // edit they now have to fix.  This is what
                                                        // "the tick unset itself and nothing
                                                        // happened" used to mean.
                                                        ed_back.write().insert(id_back, w);
                                                    }
                                                    note.set(Some(r));
                                                });
                                            },
                                            "Apply"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One cell of the matrix.  The widget is the library's `Checkbox`, and it owns the
/// parts of the ARIA contract a hand-written cell never had: `role="checkbox"`, the
/// announced state (`aria-checked`), the Space-only rule ARIA gives a checkbox, and
/// focus-on-click on the browsers that do not focus a button when it is clicked.
/// What it cannot know is what the cell *means*, so the name and the reason stay on
/// this side of the line.
///
/// `Checkbox` is controlled by a signal, while this page's truth is the poll, so the
/// signal is created once per cell and reconciled with every fetch: a tick made in
/// another tab, or by another operator, still moves this cell — the rule stated at
/// the top of this file.
#[component]
fn Cell(
    edits: Signal<BTreeMap<String, RowEdit>>,
    port: String,
    vlan: i64,
    snap: RowEdit,
    locked: bool,
    name: String,
    reason: String,
    why_id: String,
) -> Element {
    // `Checkbox` is *controlled*: it reads `checked` on every render, so the cell's state
    // has to be a signal that follows the overlay. A `bool` prop captured at the first
    // render keeps announcing the tick as it was when the row was drawn while the overlay
    // moves underneath it; the effect re-runs because the overlay is a signal, and that is
    // the difference — a tick made in another tab, or by another operator, still moves
    // this cell.
    let mut state = use_signal(|| Some(cell_state(snap.tagged.contains(&vlan))));
    // Two closures, so two copies: `String` and `RowEdit` are not `Copy`, and the effect
    // cannot move them while the click handler still needs them.
    let effect_port = port.clone();
    let effect_snap = snap.clone();
    let handler_port = port.clone();
    let handler_snap = snap.clone();
    use_effect(move || {
        let w = edits.read().get(&effect_port).cloned().unwrap_or(effect_snap.clone());
        state.set(Some(cell_state(w.tagged.contains(&vlan))));
    });

    rsx! {
        td {
            class: "ck",
            // The hook the headless suite selects on, kept from the old cell. It is on the
            // cell rather than on the widget: the cell is markup this app writes, while the
            // control inside it is the library's, and its attribute list comes from the
            // component's own props rather than from keys at the call site.
            "data-vlan": vlan.to_string(),
            Checkbox {
                checked: ReadSignal::from(state),
                // A `ReadSignal<bool>`, not the plain `bool`: a bare `locked` here lands on the
                // button as the HTML `disabled` attribute, and the widget's own disabled state —
                // `data-disabled`, and the state a screen reader is told — is never set.
                disabled: ReadSignal::new(Signal::new(locked)),
                aria_label: name.clone(),
                aria_describedby: why_id.clone(),
                on_checked_change: move |s| {
                    let mut w = edits
                        .read()
                        .get(&handler_port)
                        .cloned()
                        .unwrap_or(handler_snap.clone());
                    w.tagged.retain(|x| *x != vlan);
                    if s == CheckboxState::Checked {
                        w.tagged.push(vlan);
                        w.tagged.sort();
                    }
                    edits.write().insert(handler_port.clone(), w);
                },
                CheckboxIndicator { "✓" }
            }
            span { id: why_id, class: "why", "{reason}" }
        }
    }
}

fn cell_state(checked: bool) -> CheckboxState {
    if checked {
        CheckboxState::Checked
    } else {
        CheckboxState::Unchecked
    }
}

/// The row a port should be edited from: the operator's overlay if there is one,
/// the switch's state if not.
fn wanted_of(edits: &Signal<BTreeMap<String, RowEdit>>, port: &dto::Port) -> RowEdit {
    edits
        .read()
        .get(&port.id)
        .cloned()
        .unwrap_or_else(|| row_from_snapshot(port))
}

// ------------------------------------------------------------------ plumbing

/// Whether the controls on this page may send anything: the session has to have
/// Manager **and** this instance has to accept writes at all.  The server checks
/// both again — this is only so a control that cannot work is never offered.
fn can_write(p: &dto::Page) -> bool {
    p.privilege == "manager" && !p.read_only
}

/// A text input that remembers what was typed, keyed by `key`.
///
/// Written on `change`, not on `input`: a controlled input re-renders the page on
/// every keystroke, and this page has fifty rows by ten VLAN columns of checkboxes
/// under it.  `change` fires when the field loses focus after an edit — which a
/// button click does before its own handler runs, so the button next to the field
/// always reads what the operator typed.
fn text_input(key: &str, value: &str, mut inputs: Signal<BTreeMap<String, String>>) -> Element {
    let k = key.to_string();
    rsx! {
        input {
            r#type: "text",
            size: "10",
            id: "{k}",
            value: "{value}",
            onchange: move |e| {
                let v = e.value();
                inputs.write().insert(k.clone(), v);
            },
        }
    }
}

/// What a text input holds: what the operator typed, or the switch's value if they
/// never touched the field.
fn typed(inputs: &Signal<BTreeMap<String, String>>, key: &str) -> String {
    inputs.read().get(key).cloned().unwrap_or_default()
}

/// An intent for this switch.  The kind's own fields are filled in by the caller.
fn intent(kind: &str, switch_name: &str) -> dto::Intent {
    dto::Intent {
        switch_name: switch_name.to_string(),
        kind: kind.to_string(),
        ..Default::default()
    }
}

/// `POST /api/intent` and turn the answer into the banner's text.
///
/// Nothing here waits for a switch: a successful reply means the row is in the
/// queue, and the page keeps marking the port until a poll says it is not.
async fn send(intent: dto::Intent) -> (bool, String) {
    match post_json::<dto::Intent, dto::Ack>("/api/intent", &intent).await {
        Ok(a) if a.ok => match a.change {
            Some(c) => (
                true,
                format!("Queued #{}: {} — the sync will apply it.", c.id, c.summary),
            ),
            // "already in that state" is a success with no row: the matrix sends the
            // whole wanted row, so this is an ordinary answer, not a failure.
            None => (
                true,
                "Nothing to send — the switch is already in that state.".to_string(),
            ),
        },
        // The refusal sentence is the thing the operator needs (`a trunk member's
        // VLAN membership cannot be changed`), so it is shown verbatim.
        Ok(a) => (false, a.error.unwrap_or_else(|| "refused".to_string())),
        Err(e) => (false, e),
    }
}
