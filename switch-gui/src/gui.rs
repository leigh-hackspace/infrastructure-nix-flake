//! The dashboard: server-rendered HTML, one small script.
//!
//! Same shape as `status-dashboard` — the page is built here so it works with
//! scripting off, and the only JavaScript is the `act()` helper that POSTs an
//! action and reports what the switch said.  Actions are `POST`s with their
//! parameters in the query string and no request body, which is what lets this
//! use the shared body-less HTTP plumbing in `common-rs/web` (a real body would
//! mean changing a crate two other servers depend on for no benefit here).
//!
//! Everything interpolated is escaped: port names and descriptions are free
//! text an operator typed into the switch, and they end up inside HTML and
//! inside `onclick` strings.

use crate::model::{self, Snapshot};

/// One switch as the page lists it — the same row serves the index and the nav
/// bar on every dashboard, so the two cannot disagree about which switches exist.
///
/// Built from the *cache*, never from a fresh read: the index is a directory of
/// switches, and a page that reads all three on every hit would pay six-plus
/// Telnet round-trips per switch just to say how many ports each one has.
#[derive(Debug, Clone)]
pub struct SwitchRow {
    pub name: String,
    pub host: String,
    pub current: bool,
    pub system_name: String,
    pub location: String,
    pub software: String,
    pub ports: usize,
    pub vlans: usize,
    pub cache_age_secs: u64,
    /// The last read failure, when the cache is empty. A switch that is down has
    /// to be visible here, or the page looks like a switch with no ports.
    pub error: Option<String>,
}

pub struct Page<'a> {
    /// The configured name of the switch this page is about (`switch2`), used as
    /// the title when the switch has no `System Name` of its own.
    pub name: &'a str,
    pub snap: &'a Snapshot,
    /// Every configured switch, for the nav bar.
    pub rows: &'a [SwitchRow],
    /// `Operator (read-only)` or `Manager`.
    pub privilege: &'static str,
    pub is_manager: bool,
    pub user: &'a str,
    /// The signed-in user's address from authentik, when it sent one.
    pub email: Option<&'a str>,
    /// Configuration actions are disabled: either the CLI session is read-only
    /// or `--read-only` was passed.
    pub read_only: bool,
    pub cache_age_secs: u64,
    /// The switch's own management UI, linked for the things this page does not
    /// do (SNMP, syslogging, the STP bridge settings).
    pub switch_url: &'a str,
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const STYLE: &str = r#"
:root{color-scheme:dark;--bg:#14161a;--panel:#1b1e24;--line:#2a2f38;--fg:#e6e9ef;
--dim:#98a2b3;--ok:#4ade80;--warn:#fbbf24;--err:#f87171;--accent:#60a5fa}
*{box-sizing:border-box}
body{margin:0;padding:20px;background:var(--bg);color:var(--fg);
font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace}
h1{font-size:17px;margin:0 0 4px}h2{font-size:13px;margin:26px 0 8px;color:var(--dim);
text-transform:uppercase;letter-spacing:.08em}
a{color:var(--accent)}
header{display:flex;flex-wrap:wrap;gap:14px;align-items:baseline;
border-bottom:1px solid var(--line);padding-bottom:12px;margin-bottom:16px}
.spacer{flex:1}
.badge{border:1px solid var(--line);border-radius:999px;padding:2px 10px;font-size:12px}
.badge.ro{color:var(--warn);border-color:#5a4a1d}
.badge.mgr{color:var(--ok);border-color:#1f4d33}
table{border-collapse:collapse;width:100%;font-size:13px}
th,td{padding:5px 9px;border-bottom:1px solid var(--line);text-align:left;
white-space:nowrap;vertical-align:top}
th{color:var(--dim);font-weight:600;font-size:11px;text-transform:uppercase}
td.n{text-align:right;font-variant-numeric:tabular-nums}
tr:hover td{background:#191c22}
.up{color:var(--ok)}.down{color:var(--dim)}.err{color:var(--err)}.warn{color:var(--warn)}
.note{border:1px solid var(--line);border-left-width:3px;border-radius:6px;
padding:9px 12px;margin:0 0 14px}
.note.ok{border-left-color:var(--ok)}.note.err{border-left-color:var(--err)}
.note code{color:var(--fg);background:#0e1013;padding:1px 5px;border-radius:4px;font-size:12px}
button{background:#22262e;color:var(--fg);border:1px solid var(--line);border-radius:6px;
padding:4px 10px;font:inherit;font-size:12px;cursor:pointer}
button:hover{border-color:var(--accent)}
button.danger:hover{border-color:var(--err);color:var(--err)}
input[type=text],input[type=number],select{background:#0e1013;color:var(--fg);
border:1px solid var(--line);border-radius:6px;padding:4px 7px;font:inherit;font-size:12px}
.row{display:flex;gap:6px;align-items:center;flex-wrap:wrap}
.muted{color:var(--dim)}
pre{background:#0e1013;border:1px solid var(--line);border-radius:6px;padding:10px;
overflow:auto;font-size:12px}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(190px,1fr));gap:10px}
.card{background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:10px 12px}
.card b{display:block;color:var(--dim);font-size:11px;font-weight:600;
text-transform:uppercase;letter-spacing:.06em;margin-bottom:3px}
nav{display:flex;gap:8px;flex-wrap:wrap;margin:0 0 14px}
nav a.badge{text-decoration:none}
nav .cur{border-color:var(--accent);color:var(--fg)}
/* The VLAN membership matrix.  One column per VLAN, named vertically so forty
   ports and ten VLANs fit without horizontal scrolling: a horizontal header
   would need ~90px per column, a vertical one needs the width of a checkbox. */
table.mx{table-layout:auto}
.mx th.v{writing-mode:vertical-rl;transform:rotate(180deg);text-align:left;
vertical-align:bottom;height:11em;padding:6px 3px;font-size:10px;letter-spacing:.04em}
.mx th.v abbr{text-decoration:none;cursor:help}
.mx td.ck{text-align:center;padding:4px 3px}
.mx td.ck label{display:block;cursor:pointer}
.mx input[type=checkbox]{accent-color:var(--accent);width:15px;height:15px;margin:0;cursor:pointer}
.mx input[type=checkbox]:disabled{cursor:not-allowed;opacity:.35}
/* A row whose form no longer matches the switch.  The page is a cache, so say
   which rows are about to change rather than making the operator remember. */
.mx tr.dirty td{background:#1d2531}
.mx tr.dirty th{background:#1d2531}
.mx td.apply{white-space:nowrap}
.mx tr.dirty button{border-color:var(--accent);color:var(--accent)}
"#;

const SCRIPT: &str = r#"
async function act(url, msg){
  if(msg && !confirm(msg)) return false;
  const box = document.getElementById('note');
  // `&` once the url already has a query.  Always appending `?switch=` made
  // '/api/port/5/name?v=AP3' into '...?v=AP3?switch=switch1', and the server
  // splits the query on the first `?` — so the name arrived as
  // 'AP3?switch=switch1' and every rename died on the 15-character limit.
  const sep = url.indexOf('?') >= 0 ? '&' : '?';
  try{
    const r = await fetch(url + sep + 'switch=' + encodeURIComponent(SW), {method:'POST'});
    const t = await r.text();
    box.className = 'note ' + (r.ok ? 'ok' : 'err');
    box.textContent = t || (r.ok ? 'done' : 'failed');
    if(r.ok){ setTimeout(()=>location.reload(), 900); }
  }catch(e){
    box.className = 'note err'; box.textContent = String(e);
  }
  return false;
}
function refresh(){ act('/api/refresh'); return false; }

// --- the VLAN membership matrix -------------------------------------------
// A row's state is its untagged select plus its checked tagged boxes.  The row
// remembers what the switch said when the page was built (`data-init`), which
// is the only way this page can be honest about being a cache: it shows which
// rows are about to change, and never claims a change it has not confirmed.
function rowBoxes(id){
  return document.querySelectorAll('input[data-port="' + id + '"]');
}
function taggedOf(id){
  const t = [];
  rowBoxes(id).forEach(c => { if (c.checked) t.push(c.dataset.vlan); });
  t.sort((a, b) => a - b);
  return t;
}
function rowState(id){
  const u = document.getElementById('mu' + id);
  return (u ? u.value : '') + '|' + taggedOf(id).join(',');
}
// A port cannot be tagged and untagged on the same VLAN.  The server refuses it
// too, but ticking a box that is about to be refused is a bad way to find out.
function syncRow(id){
  const u = document.getElementById('mu' + id);
  const v = u ? u.value : '';
  rowBoxes(id).forEach(c => {
    const clash = v !== '' && c.dataset.vlan === v;
    if (c.dataset.locked !== '1') c.disabled = clash;
    if (clash) c.checked = false;
  });
  markRow(id);
}
function markRow(id){
  const row = document.getElementById('mrow' + id);
  if (!row) return;
  row.classList.toggle('dirty', rowState(id) !== row.dataset.init);
}
async function saveMembers(id, label){
  const u = document.getElementById('mu' + id);
  const q = '?untagged=' + encodeURIComponent(u ? u.value : '') +
            '&tagged=' + encodeURIComponent(taggedOf(id).join(','));
  return act('/api/port/' + encodeURIComponent(id) + '/members' + q,
             'Apply the VLAN membership of port ' + label + ' to the switch?');
}
"#;

/// The page script, with the switch it belongs to.
///
/// Every endpoint is per switch, so every request has to say which one. A page
/// showing `switch3` that posted to the default switch would rename a port on
/// the wrong device — the one mistake this cannot walk back.
fn script(name: &str) -> String {
    format!("const SW = '{}';{SCRIPT}", name.replace(['\'', '\\'], ""))
}

/// The banner `act()` writes its result into.  It starts hidden: the page is
/// rendered before any action has happened, and the script fills it in.
fn note_slot() -> String {
    "<div class=\"note ok\" id=\"note\" hidden></div>".to_string()
}

fn shell(title: &str, body: String, name: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <style>{STYLE}</style><script>{script}</script></head><body>{body}</body></html>",
        script = script(name)
    )
}

/// The signed-out page.  Kept separate from the dashboard so a failed login
/// never renders a half-populated switch view.
pub fn login(url: &str, group: &str, switches: &str, error: Option<&str>) -> String {
    let err = match error {
        Some(e) => format!("<div class=\"note err\">{}</div>", esc(e)),
        None => String::new(),
    };
    shell(
        "Switches · sign in",
        format!(
            "<div style=\"max-width:520px;margin:8vh auto\">\
             <h1>network switches</h1>\
             <p class=\"muted\">Ports and VLANs on {switches}. \
             Sign in with an authentik account in the <b>{group}</b> group.</p>\
             {err}\
             <p><a class=\"badge\" href=\"{}\">Sign in with authentik</a></p>\
             </div>",
            esc(url),
            switches = esc(switches),
            group = esc(group)
        ),
        "",
    )
}

/// The switch list, as the nav bar on every page.
fn nav(rows: &[SwitchRow]) -> String {
    let mut h = String::from("<nav>");
    for r in rows {
        let label = esc(&r.name);
        if r.current {
            h.push_str(&format!("<span class=\"badge cur\">{label}</span>"));
        } else {
            h.push_str(&format!(
                "<a class=\"badge\" href=\"/switch/{}\">{label}</a>",
                esc(&r.name)
            ));
        }
    }
    h.push_str("</nav>");
    h
}

/// The landing page: one row per switch, so a switch that is down is visible
/// before anyone has to click into it and find an empty table.
///
/// It reads the cache only. The numbers are the ones the last refresh produced,
/// and the page says how old they are — a landing page that re-reads every
/// switch would cost six-plus Telnet round-trips each, on a CPU that pages its
/// own output.
pub fn index(rows: &[SwitchRow], user: &str, email: Option<&str>) -> String {
    let mut h = String::new();
    h.push_str(
        "<header><div><h1>network switches</h1><div class=\"muted\">ProCurve, read over Telnet</div></div>\
         <div class=\"spacer\"></div>\
         <a href=\"/logout\"><button>Sign out</button></a></header>",
    );
    h.push_str(&format!(
        "<div class=\"muted\" style=\"margin:-8px 0 14px\">signed in as {}{}</div>",
        esc(user),
        match email {
            Some(e) => format!(" &lt;{}&gt;", esc(e)),
            None => String::new(),
        }
    ));
    h.push_str("<table><tr><th>Switch</th><th>Address</th><th>System</th><th>Location</th>\
                 <th>Revision</th><th class=\"n\">Ports</th><th class=\"n\">VLANs</th>\
                 <th>Cache</th></tr>");
    for r in rows {
        let counts = if r.ports == 0 && r.error.is_some() {
            ("—".to_string(), "—".to_string())
        } else {
            (r.ports.to_string(), r.vlans.to_string())
        };
        let cache = if r.error.is_some() {
            format!("<span class=\"err\">{}</span>", esc(&r.error.clone().unwrap_or_default()))
        } else if r.cache_age_secs == u64::MAX {
            "not read yet".to_string()
        } else {
            format!("{}s", r.cache_age_secs)
        };
        h.push_str(&format!(
            "<tr><td><b><a href=\"/switch/{name}\">{name}</a></b></td><td class=\"muted\">{host}</td>\
             <td>{sys}</td><td class=\"muted\">{loc}</td><td class=\"muted\">{rev}</td>\
             <td class=\"n\">{ports}</td><td class=\"n\">{vlans}</td><td class=\"muted\">{cache}</td></tr>",
            name = esc(&r.name),
            host = esc(&r.host),
            sys = esc(if r.system_name.is_empty() { "—" } else { &r.system_name }),
            loc = esc(if r.location.is_empty() { "—" } else { &r.location }),
            rev = esc(if r.software.is_empty() { "—" } else { &r.software }),
            ports = esc(&counts.0),
            vlans = esc(&counts.1),
            cache = cache,
        ));
    }
    h.push_str("</table>");
    h.push_str("<p class=\"muted\">switch-gui · read over Telnet, written with the ProCurve CLI.</p>");
    shell("network switches", h, "")
}

pub fn dashboard(p: &Page) -> String {
    let s = p.snap;
    let mut h = String::new();

    let badge = if p.is_manager {
        "<span class=\"badge mgr\">Manager</span>"
    } else {
        "<span class=\"badge ro\" title=\"The CLI session is in the Operator context: \
         configuration buttons are disabled\">Operator · read-only</span>"
    };
    let ro = if p.read_only {
        " <span class=\"badge ro\" title=\"Writes are disabled on this deployment \
         (--read-only): the write path has not been proven against the switch\">writes off</span>"
    } else {
        ""
    };

    h.push_str(&format!(
        "<header><div><h1>{}</h1><div class=\"muted\">{} · {} · {}</div></div>\
         <div class=\"spacer\"></div>\
         {badge}{ro}\
         <span class=\"badge\">cache {}s</span>\
         <span class=\"badge\">{}</span>\
         <button onclick=\"return refresh()\">Refresh</button>\
         <a href=\"{switch_url}\"><button>Switch UI</button></a>\
         <a href=\"/logout\"><button>Sign out</button></a>\
         </header>",
        esc(if s.system.name.is_empty() {
            p.name
        } else {
            &s.system.name
        }),
        esc(if s.system.location.is_empty() {
            "—"
        } else {
            &s.system.location
        }),
        esc(if s.system.software.is_empty() {
            "unknown revision"
        } else {
            &s.system.software
        }),
        esc(if s.system.uptime.is_empty() {
            "—"
        } else {
            &s.system.uptime
        }),
        p.cache_age_secs,
        esc(p.privilege),
        switch_url = esc(p.switch_url),
    ));
    // The user is shown after the header markup so the badge order stays
    // readable in the source above.
    h.push_str(&format!(
        "<div class=\"muted\" style=\"margin:-8px 0 14px\">signed in as {}{}</div>",
        esc(p.user),
        match p.email {
            Some(e) => format!(" &lt;{}&gt;", esc(e)),
            None => String::new(),
        }
    ));

    h.push_str(&note_slot());
    h.push_str(&nav(p.rows));
    h.push_str(&nav(p.rows));

    if !s.warnings.is_empty() {
        h.push_str("<div class=\"note err\"><b>Some commands failed</b><ul>");
        for w in &s.warnings {
            h.push_str(&format!("<li>{}</li>", esc(w)));
        }
        h.push_str("</ul></div>");
    }
    if !p.is_manager && !p.read_only {
        h.push_str(
            "<div class=\"note err\"><b>Read-only session.</b> The switch refused `enable`, so this \
             login is in the Operator context and cannot configure anything. The enable \
             credentials default to the login account, so this means they were refused.</div>",
        );
    }

    h.push_str("<div class=\"grid\">");
    for (k, v) in [
        ("System name", &s.system.name),
        ("MAC address", &s.system.mac),
        ("Serial", &s.system.serial),
        ("ROM", &s.system.rom),
        ("CPU", &s.system.cpu),
        ("Contact", &s.system.contact),
    ] {
        if !v.is_empty() {
            h.push_str(&format!(
                "<div class=\"card\"><b>{k}</b>{}</div>",
                esc(v)
            ));
        }
    }
    h.push_str("</div>");

    h.push_str("<h2>VLANs</h2>");
    h.push_str(&vlan_table(p));
    h.push_str("<h2>Ports</h2>");
    h.push_str(&port_table(p));
    h.push_str("<h2>VLAN membership</h2>");
    h.push_str(&membership_matrix(p));
    h.push_str(&console_box(p.is_manager));

    // Wrapped in the shell: without it the page has no stylesheet and no script,
    // so every `act()` button on it is inert.
    shell(
        &format!("{} · switch-gui", if s.system.name.is_empty() {
            p.name
        } else {
            &s.system.name
        }),
        format!(
            "{h}<p class=\"muted\">switch-gui · read over Telnet, written with the ProCurve CLI. \
             <a href=\"/api/snapshot?switch={name}\">JSON</a> · <a href=\"/api/audit\">audit log</a></p>",
            h = h,
            name = esc(p.name)
        ),
        p.name,
    )
}

fn vlan_table(p: &Page) -> String {
    let s = p.snap;
    if s.vlans.is_empty() {
        return "<p class=\"note err\">No VLANs were read.</p>".to_string();
    }
    let mut h = String::from(
        "<table><tr><th>VLAN</th><th>Name</th><th>Status</th><th>Untagged</th>\
         <th>Tagged</th><th></th></tr>",
    );
    for v in &s.vlans {
        let mut untagged = Vec::new();
        let mut tagged = Vec::new();
        for port in s.sorted_ports() {
            if port.untagged_vlan == Some(v.id) {
                untagged.push(port.id.as_str());
            }
            if port.tagged_vlans.contains(&v.id) {
                tagged.push(port.id.as_str());
            }
        }
        h.push_str(&format!(
            "<tr><td class=\"n\"><b>{}</b></td><td>{}</td><td class=\"muted\">{}</td>\
             <td class=\"muted\">{}</td><td class=\"muted\">{}</td><td>{}</td></tr>",
            v.id,
            esc(&v.name),
            esc(&v.status),
            esc(&untagged.join(", ")),
            esc(&tagged.join(", ")),
            vlan_actions(p, v.id, &v.name)
        ));
    }
    h.push_str("</table>");
    h
}

fn vlan_actions(p: &Page, id: u32, name: &str) -> String {
    if p.read_only {
        return String::new();
    }
    let dis = if p.is_manager { "" } else { " disabled " };
    let rename = format!(
        "<input type=\"text\" size=\"10\" id=\"vn{id}\" value=\"{name}\" placeholder=\"new name\">\
         <button{dis} onclick=\"return act('/api/vlan/{id}/rename?v='+encodeURIComponent(\
         document.getElementById('vn{id}').value))\">Rename</button>",
        id = id,
        name = esc(name)
    );
    let del = if id == 1 {
        String::new()
    } else {
        format!(
            "<button class=\"danger\"{dis} onclick=\"return act('/api/vlan/{id}/delete', \
             'Delete VLAN {id}? The switch refuses this while ports are still on it.')\">Delete</button>",
            id = id
        )
    };
    format!("{rename}{del}")
}

fn port_table(p: &Page) -> String {
    let s = p.snap;
    let mut h = String::from(
        "<table><tr><th>Port</th><th>Name</th><th>Media</th><th>Link</th><th>STP</th>\
         <th>Trunk</th><th>VLAN (untagged)</th><th>Tagged</th><th class=\"n\">Rx err</th>\
         <th class=\"n\">Drops</th><th></th></tr>",
    );
    for port in s.sorted_ports() {
        // Built from locals rather than one long format! call: the row has ten
        // cells and mixing positional with captured arguments is how a column
        // silently shifts by one.
        let state = port.stp_state.as_str();
        let state_cls = if state.is_empty() {
            "muted"
        } else if state.eq_ignore_ascii_case("forwarding") {
            "up"
        } else {
            "down"
        };
        let state_text = if state.is_empty() { "—" } else { state };
        let media = if port.media.is_empty() {
            "—".to_string()
        } else {
            port.media.clone()
        };
        let name = if port.name.is_empty() {
            "—".to_string()
        } else {
            port.name.clone()
        };
        let trunk = if port.trunk_group.is_empty() {
            "—".to_string()
        } else {
            format!("{} · {}", port.trunk_group, port.trunk_type)
        };
        let untagged = port
            .untagged_vlan
            .map(|v| v.to_string())
            .unwrap_or_else(|| "—".into());
        let tagged = if port.tagged_vlans.is_empty() {
            "—".to_string()
        } else {
            port.tagged_vlans
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
        // A trunk member's state belongs to the bundle, and the page has to say
        // that rather than imply the port was measured.
        let inherited = if port.stp_state_from.is_empty() {
            String::new()
        } else {
            format!(
                " title=\"state of {}, inherited: the switch reports trunk members only as a group\"",
                esc(&port.stp_state_from)
            )
        };
        let (link_cls, link_text) = match port.link_up {
            Some(true) => ("up", "Up"),
            Some(false) => ("down", "Down"),
            None => ("muted", "—"),
        };
        h.push_str(&format!(
            "<tr><td class=\"n\"><b>{id}</b></td><td>{name}</td><td class=\"muted\">{media}</td>\
             <td class=\"{link_cls}\"{inherited}>{link}</td><td class=\"{state_cls}\"{inherited}>{state}</td>\
             <td class=\"muted\">{trunk}</td><td>{untagged}</td><td class=\"muted\">{tagged}</td>\
             <td class=\"n{err_cls}\">{errs}</td><td class=\"n\">{drops}</td><td>{actions}</td></tr>",
            id = esc(&port.id),
            name = esc(&name),
            media = esc(&media),
            link_cls = link_cls,
            inherited = inherited,
            link = link_text,
            state_cls = state_cls,
            state = esc(state_text),
            trunk = esc(&trunk),
            untagged = esc(&untagged),
            tagged = esc(&tagged),
            err_cls = err_cls,
            errs = esc(&errs),
            drops = esc(&drops),
            actions = port_actions(p, port),
        ));
    }
    h.push_str("</table>");
    h
}

fn port_actions(p: &Page, port: &crate::model::Port) -> String {
    if p.read_only {
        return String::new();
    }
    let id = port.id.as_str();
    let dis = if p.is_manager { "" } else { " disabled " };
    // The confirm message names the port the way the table does, because the
    // bare number is the same number on every switch in the rack.
    let label = esc(&port.label());
    // Only the name lives here.  The untagged VLAN used to have a dropdown of
    // its own in this row and a second one in the membership matrix, which is
    // two controls for one setting with different rules — the matrix is the one
    // that knows about tagged memberships, so it is the only one left.
    format!(
        "<span class=\"row\">\
         <input type=\"text\" size=\"8\" id=\"pn{id}\" placeholder=\"name\">\
         <button{dis} onclick=\"return act('/api/port/{id}/name?v='+encodeURIComponent(\
         document.getElementById('pn{id}').value), 'Rename port {label}?')\">Name</button>\
         </span>",
        id = id,
        dis = dis,
        label = label
    )
}

/// The VLAN membership editor: one row per port, one vertical column per VLAN.
///
/// Why a matrix and not a per-VLAN port list, which is what the VLAN table above
/// already is: the switch's own model is VLAN-first — `vlan 225 tagged 7,9-11` is
/// a statement about a VLAN and a pile of ports — but an operator at a patch
/// panel thinks per port.  This is the per-port question ("which VLANs is 7 in,
/// and which one is native?") answered on one screen.
///
/// The form submits the **whole wanted row**, not the click that changed it, and
/// [`crate::commands::port_members`] turns it into the difference against this
/// snapshot.  That is what stops a tick from sending a half-state, and it is why
/// the row is marked `dirty` the moment it stops matching the switch: this page
/// is a cache, so it shows what it is about to change instead of pretending it
/// *is* the switch.  Nothing is written until Apply is pressed, and the reload
/// afterwards re-reads the device — so what is on screen after an apply is what
/// the switch says, not what the form hoped for.
///
/// The VLAN names are vertical because there are ten VLANs and fifty ports: a
/// horizontal header needs roughly 90 px per column and the table stops fitting
/// the screen, while a vertical one costs the width of a checkbox.
fn membership_matrix(p: &Page) -> String {
    let s = p.snap;
    let editable = p.is_manager && !p.read_only;
    let mut h = String::from(
        "<p class=\"muted\">Tick the VLANs a port is <b>tagged</b> in and pick its <b>untagged</b> \
         (native, access) VLAN. Only rows you have changed are sent, and only when you press \
         Apply.</p>\
         <table class=\"mx\"><tr><th>Port</th><th>Name</th><th>Untagged</th>",
    );
    for v in &s.vlans {
        // The tooltip carries the whole thing: a vertical header truncates, and
        // `AD Lab` or `CS_VLAN_Paul` are not names to guess at.
        h.push_str(&format!(
            "<th class=\"v\"><abbr title=\"VLAN {} · {} · {}\">{} · {}</abbr></th>",
            v.id,
            esc(&v.name),
            esc(&v.status),
            v.id,
            esc(&v.name)
        ));
    }
    h.push_str("<th></th></tr>");

    for port in s.sorted_ports() {
        let id = port.id.as_str();
        // Two kinds of "trunk", and both are locked: a **member** carries the
        // group in `trunk_group`, the **group itself** is a row whose id is the
        // token (`Trk1`).  The switch refuses the first, and the second has
        // never been exercised — so neither gets a live control.  Checking only
        // `trunk_group` is the mistake to avoid: it is set on members, so the
        // group's own row comes out looking like an ordinary port.
        let trunk = !port.trunk_group.is_empty() || model::is_trunk_group(id);
        let locked = !editable || trunk;
        let label = esc(&port.label());
        let why = if !port.trunk_group.is_empty() {
            format!(
                "title=\"in LACP group {} — the switch will not change the VLAN membership of a trunk member\"",
                esc(&port.trunk_group)
            )
        } else if trunk {
            "title=\"a link-aggregation group: its membership moves with its members, which this tool does not attempt\"".to_string()
        } else if !editable {
            "title=\"read-only session\"".to_string()
        } else {
            String::new()
        };

        // The same shape the script recomputes, so `dirty` means "different from
        // the switch" and not "different from what the page guessed".
        let mut tagged: Vec<u32> = port.tagged_vlans.clone();
        tagged.sort();
        let init = format!(
            "{}|{}",
            port.untagged_vlan
                .map(|v| v.to_string())
                .unwrap_or_default(),
            tagged
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let mut opts = String::new();
        if port.untagged_vlan.is_none() {
            opts.push_str("<option value=\"\" selected>— not set —</option>");
        }
        for v in &s.vlans {
            let sel = if port.untagged_vlan == Some(v.id) {
                " selected"
            } else {
                ""
            };
            opts.push_str(&format!(
                "<option value=\"{}\"{}>{} · {}</option>",
                v.id,
                sel,
                v.id,
                esc(&v.name)
            ));
        }
        let sdis = if locked { " disabled" } else { "" };
        let select = format!(
            "<select id=\"mu{id}\"{sdis} onchange=\"syncRow('{id}')\">{opts}</select>",
            id = id,
            sdis = sdis,
            opts = opts
        );

        let mut cells = String::new();
        for v in &s.vlans {
            let on = port.tagged_vlans.contains(&v.id);
            let chk = if on { " checked" } else { "" };
            let cdis = if locked { " disabled" } else { "" };
            let lock = if locked { " data-locked=\"1\"" } else { "" };
            cells.push_str(&format!(
                "<td class=\"ck\"><label title=\"{} · VLAN {}\"><input type=\"checkbox\" \
                 data-port=\"{id}\" data-vlan=\"{vid}\"{lock}{cdis}{chk} \
                 onchange=\"markRow('{id}')\"></label></td>",
                if on { "tagged" } else { "not tagged" },
                v.id,
                id = id,
                vid = v.id
            ));
        }

        let btn = if locked {
            String::new()
        } else {
            format!(
                "<button onclick=\"return saveMembers('{id}','{label}')\">Apply</button>"
            )
        };

        h.push_str(&format!(
            "<tr id=\"mrow{id}\" data-init=\"{init}\"><td class=\"n\"><b>{id}</b></td>\
             <td{why}>{name}</td><td>{select}</td>{cells}<td class=\"apply\">{btn}</td></tr>",
            id = id,
            init = init,
            why = if why.is_empty() {
                String::new()
            } else {
                format!(" {why}")
            },
            name = esc(if port.name.is_empty() { "—" } else { &port.name }),
            select = select,
            cells = cells,
            btn = btn
        ));
    }
    h.push_str("</table>");
    h
}

fn console_box(is_manager: bool) -> String {
    let hint = if is_manager {
        "Manager session. Only <code>show</code> commands run here; configuration goes \
         through the buttons so it is audited."
    } else {
        "Read-only session: <code>show</code> commands only."
    };
    format!(
        "<h2>Console</h2><p class=\"muted\">{hint}</p>\
         <div class=\"row\"><input type=\"text\" size=\"48\" id=\"cmd\" value=\"show vlan\" \
         onkeydown=\"if(event.key==='Enter'){{runCmd();return false;}}\">\
         <button onclick=\"return runCmd()\">Run</button></div>\
         <pre id=\"out\" style=\"display:none\"></pre>\
         <script>async function runCmd(){{const o=document.getElementById('out');\
         o.style.display='block';o.textContent='…';\
         const r=await fetch('/api/command?cmd='+encodeURIComponent(\
         document.getElementById('cmd').value),{{method:'POST'}});\
         o.textContent=await r.text();}}</script>",
        hint = hint
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Counters, Port, Vlan};

    /// A snapshot shaped like the real switch: two VLANs (one with a space in
    /// its name), one forwarding access port in a trunk, one down trunk member.
    fn fixture() -> Snapshot {
        let mut s = Snapshot::default();
        s.system.name = "switch1".into();
        s.system.software = "Y.11.44".into();
        s.vlans.push(Vlan {
            id: 1,
            name: "DEFAULT".into(),
            status: "Port-based".into(),
            jumbo: "No".into(),
        });
        s.vlans.push(Vlan {
            id: 232,
            name: "AD Lab".into(),
            status: "Port-based".into(),
            jumbo: "No".into(),
        });
        let mut p1 = Port::new("1");
        p1.name = "NAS2 Port 1".into();
        p1.untagged_vlan = Some(1);
        p1.stp_state = "Forwarding".into();
        p1.link_up = Some(true);
        p1.trunk_group = "Trk3".into();
        p1.trunk_type = "LACP".into();
        p1.counters = Some(Counters {
            port: "1".into(),
            total_bytes: "2,993,652...".into(),
            total_frames: "1,219,890...".into(),
            errors_rx: "0".into(),
            drops_rx: "0".into(),
            flow_ctrl: "off".into(),
            bcast_limit: "0".into(),
        });
        s.ports.insert("1".into(), p1);
        let mut p2 = Port::new("2");
        p2.tagged_vlans = vec![232];
        p2.stp_state = "Disabled".into();
        p2.link_up = Some(false);
        p2.counters = Some(Counters {
            port: "2".into(),
            total_bytes: "688,085".into(),
            total_frames: "1700".into(),
            errors_rx: "7".into(),
            drops_rx: "0".into(),
            flow_ctrl: "off".into(),
            bcast_limit: "0".into(),
        });
        s.ports.insert("2".into(), p2);
        s
    }

    /// A nav bar with the three switches, so a test can check the page knows
    /// which one it is showing.
    fn rows(current: &str) -> Vec<SwitchRow> {
        ["switch1", "switch2", "switch3"]
            .iter()
            .map(|n| SwitchRow {
                name: n.to_string(),
                host: format!("10.3.1.{}", n[6..].parse::<u32>().unwrap()),
                current: *n == current,
                system_name: String::new(),
                location: String::new(),
                software: String::new(),
                ports: 0,
                vlans: 0,
                cache_age_secs: 0,
                error: None,
            })
            .collect()
    }

    /// The snapshot is borrowed by the Page, so it is built outside the closure
    /// and kept alive by the caller.
    fn with_page<T>(f: impl FnOnce(Page) -> T) -> T {
        let s = fixture();
        let r = rows("switch1");
        let p = Page {
            name: "switch1",
            snap: &s,
            rows: &r,
            privilege: "Operator (read-only)",
            is_manager: false,
            user: "cjdell",
            email: None,
            read_only: false,
            cache_age_secs: 3,
            switch_url: "http://10.3.1.2",
        };
        f(p)
    }

    /// The fixture as a Manager session, for the tests that ask which controls
    /// are live rather than which exist.
    fn manager_page(s: &Snapshot) -> Page<'_> {
        Page {
            name: "switch1",
            snap: s,
            rows: &[],
            privilege: "Manager",
            is_manager: true,
            user: "cjdell",
            email: None,
            read_only: false,
            cache_age_secs: 0,
            switch_url: "http://10.3.1.2",
        }
    }

    fn read_only_page(s: &Snapshot) -> Page<'_> {
        Page {
            privilege: "Operator (read-only)",
            is_manager: false,
            read_only: true,
            cache_age_secs: 0,
            switch_url: "http://10.3.1.2",
            ..manager_page(s)
        }
    }

    fn count(hay: &str, needle: &str) -> usize {
        hay.matches(needle).count()
    }

    #[test]
    fn escaping() {
        assert_eq!(esc("\"><&"), "&quot;&gt;&lt;&amp;");
        assert_eq!(esc("<script>"), "&lt;script&gt;");
        assert_eq!(esc("a'b"), "a&#39;b");
        assert_eq!(esc("plain"), "plain");
    }

    #[test]
    fn dashboard_renders_both_tables() {
        with_page(|p| {
            let html = dashboard(&p);
            assert!(html.contains("AD Lab"), "VLAN name with a space");
            assert!(html.contains("NAS2 Port 1"));
            assert!(html.contains("Forwarding"));
            assert!(html.contains("read-only"));
            assert!(html.contains("cache 3s"));
            assert!(html.contains("signed in as cjdell"));
            assert!(html.contains("http://10.3.1.2"));
            // The trunk group and its type, merged out of `show trunks`.
            assert!(html.contains("Trk3 · LACP"));
        });
    }

    /// Link state and STP state are different columns and must not be merged:
    /// a port can be down with STP merely disabled.
    #[test]
    fn link_state_is_its_own_column() {
        with_page(|p| {
            let html = dashboard(&p);
            assert!(html.contains(">Up<"));
            assert!(html.contains(">Down<"));
        });
    }

    /// A port name is operator-typed text; it must not be able to close an
    /// attribute and run something.
    #[test]
    fn switch_supplied_text_is_escaped() {
        let mut s = Snapshot::default();
        let mut p = Port::new("1");
        p.name = r#"" onload="alert(1)"#.to_string();
        s.ports.insert("1".into(), p);
        let page = Page {
            name: "switch1",
            snap: &s,
            rows: &[],
            privilege: "Manager",
            is_manager: true,
            user: "cjdell",
            email: None,
            read_only: false,
            cache_age_secs: 0,
            switch_url: "http://10.3.1.2",
        };
        let html = dashboard(&page);
        assert!(!html.contains("onload=\"alert"));
        assert!(html.contains("&quot; onload="));
    }

    #[test]
    fn manager_sessions_get_enabled_buttons() {
        with_page(|p| {
            assert!(dashboard(&p).contains(" disabled "));
        });
        let s = fixture();
        let mgr = Page {
            name: "switch1",
            snap: &s,
            rows: &[],
            privilege: "Manager",
            is_manager: true,
            user: "cjdell",
            email: None,
            read_only: false,
            cache_age_secs: 0,
            switch_url: "http://10.3.1.2",
        };
        let html = dashboard(&mgr);
        // A Manager session gets live buttons.  The matrix still renders a trunk
        // row's controls disabled — the switch refuses membership changes for a
        // trunk member — so "nothing on the page is disabled" was the wrong
        // assertion once the matrix existed, and saying so is better than
        // loosening it to nothing at all.
        assert!(!html.contains("button disabled"), "{html}");
        assert!(!html.contains("Read-only session"));
        assert!(
            html.contains("in LACP group Trk3"),
            "a locked trunk row has to say why"
        );
    }

    #[test]
    fn read_only_hides_the_controls() {
        let s = fixture();
        let q = Page {
            name: "switch1",
            snap: &s,
            rows: &[],
            privilege: "Operator (read-only)",
            is_manager: false,
            user: "cjdell",
            email: None,
            read_only: true,
            cache_age_secs: 0,
            switch_url: "http://10.3.1.2",
        };
        let html = dashboard(&q);
        assert!(!html.contains("Set VLAN"));
        assert!(!html.contains("Rename"));
    }

    /// A failed command must be visible: a page that looks healthy but is
    /// missing a table is worse than one that says so.
    #[test]
    fn warnings_are_shown() {
        let mut s = fixture();
        s.warnings.push("show trunks: Invalid input: trunks".into());
        let p = Page {
            name: "switch1",
            snap: &s,
            rows: &[],
            privilege: "Manager",
            is_manager: true,
            user: "cjdell",
            email: None,
            read_only: false,
            cache_age_secs: 0,
            switch_url: "http://10.3.1.2",
        };
        let html = dashboard(&p);
        assert!(html.contains("Some commands failed"));
        assert!(html.contains("Invalid input: trunks"));
    }

    /// The group gate is the login page's whole promise, so the group name it
    /// shows has to be the one the server enforces (it comes from config).
    #[test]
    fn login_page_names_the_required_group() {
        let html = login("https://id/x", "Network-Admins", "switch1 (10.3.1.2)", None);
        assert!(html.contains("<b>Network-Admins</b>"));
    }

    #[test]
    fn the_signed_in_user_and_email_are_shown() {
        let s = fixture();
        let p = Page {
            name: "switch1",
            snap: &s,
            rows: &[],
            privilege: "Manager",
            is_manager: true,
            user: "cjdell",
            email: Some("cjdell@leighhack.org"),
            read_only: false,
            cache_age_secs: 0,
            switch_url: "http://10.3.1.2",
        };
        let html = dashboard(&p);
        assert!(html.contains("signed in as cjdell &lt;cjdell@leighhack.org&gt;"));
    }

    /// A trunk member's state is the bundle's, and the row has to say so.
    #[test]
    fn an_inherited_state_is_labelled() {
        let mut s = fixture();
        let mut p = Port::new("47");
        p.trunk_group = "Trk1".into();
        p.trunk_type = "LACP".into();
        p.stp_state = "Forwarding".into();
        p.stp_state_from = "Trk1".into();
        p.link_up = Some(true);
        s.ports.insert("47".into(), p);
        let page = Page {
            name: "switch1",
            snap: &s,
            rows: &[],
            privilege: "Manager",
            is_manager: true,
            user: "cjdell",
            email: None,
            read_only: false,
            cache_age_secs: 0,
            switch_url: "http://10.3.1.2",
        };
        let html = dashboard(&page);
        assert!(html.contains("state of Trk1, inherited"));
    }

    /// The switches on the sign-in page come from config, so they cannot go
    /// stale when one moves — and the list is the same one the nav bar shows.
    #[test]
    fn login_page_shows_the_switch_address() {
        let html = login("https://id/x", "Infra", "switch3 (10.9.9.9)", None);
        assert!(html.contains("switch3 (10.9.9.9)"));
    }

    #[test]
    fn login_page_does_not_leak_the_dashboard() {
        let html = login("https://id/x", "Infra", "switch3 (10.3.1.4)", Some("not in Infra"));
        assert!(html.contains("not in Infra"));
        assert!(html.contains("Sign in with authentik"));
        assert!(!html.contains("Ports</h2>"));
    }

    /// The shared script contains the words "note err" (it sets that class), so
    /// the assertion is about the error div, not the substring.
    #[test]
    fn login_page_without_an_error() {
        let html = login("https://id/x", "Infra", "switch3 (10.3.1.4)", None);
        assert!(!html.contains("<div class=\"note err\">"));
        assert!(html.contains("Sign in with authentik"));
    }

    /// The page script has to carry the switch it is showing, or a click on
    /// `switch3` posts to whichever switch happens to be first in the list.
    #[test]
    fn the_script_names_the_switch_it_is_showing() {
        let s = fixture();
        let r = rows("switch3");
        let p = Page {
            name: "switch3",
            snap: &s,
            rows: &r,
            privilege: "Manager",
            is_manager: true,
            user: "cjdell",
            email: None,
            read_only: false,
            cache_age_secs: 0,
            switch_url: "http://10.3.1.4",
        };
        let html = dashboard(&p);
        assert!(html.contains("const SW = 'switch3'"));
        assert!(html.contains("'switch=' + encodeURIComponent(SW)"));
        assert!(html.contains("href=\"/switch/switch2\""));
        assert!(!html.contains("href=\"/switch/switch3\""), "the current switch is not a link");
    }

    /// The matrix is the only place VLAN membership is edited, and it is the
    /// only thing on the page that has to render a *state* rather than a button:
    /// the checkboxes and the dropdown have to come out pre-set to what the
    /// switch actually said, or the first Apply writes the page's guess.
    #[test]
    fn the_matrix_shows_what_the_switch_actually_said() {
        let s = fixture();
        let html = dashboard(&manager_page(&s));
        // One vertical column per VLAN, named — vertically, because ten VLANs
        // across forty-eight ports only fits if the header costs a checkbox.
        assert_eq!(count(&html, "class=\"v\""), 2);
        assert!(html.contains("1 · DEFAULT"));
        assert!(html.contains("232 · AD Lab"));
        // Port 2 is tagged in 232: its box is checked.  Port 1 is not: its is not.
        assert!(html.contains("data-port=\"2\" data-vlan=\"232\" checked"), "{html}");
        assert!(!html.contains("data-port=\"1\" data-vlan=\"232\" checked"));
        // Port 2 has no native VLAN, and the dropdown has to say that rather
        // than silently select VLAN 1 — which would be a change waiting to ship.
        assert!(html.contains("<select id=\"mu2\" onchange=\"syncRow('2')\"><option value=\"\" selected>"), "{html}");
        // Port 1 is in a trunk, so its dropdown is rendered but locked.
        assert!(html.contains("<select id=\"mu1\" disabled onchange=\"syncRow('1')\"><option value=\"1\" selected>"), "{html}");
    }

    /// `data-init` is the contract with `rowState()` in the page script: it is
    /// how a row knows it differs from the switch.  If the two formats drift, a
    /// row shows as changed when it is not, or (worse) unchanged when it is.
    #[test]
    fn each_row_carries_the_state_the_script_compares_against() {
        let s = fixture();
        let html = dashboard(&manager_page(&s));
        assert!(html.contains("id=\"mrow1\" data-init=\"1|\""), "{html}");
        assert!(html.contains("id=\"mrow2\" data-init=\"|232\""), "{html}");
    }

    /// A trunk member is the one membership the switch refuses to change, and a
    /// trunk group has never been exercised.  Neither gets a live control.
    #[test]
    fn a_trunk_row_is_locked_and_says_why() {
        let s = fixture();
        let html = dashboard(&manager_page(&s));
        let row = html
            .split("<tr id=\"mrow1\"")
            .nth(1)
            .and_then(|r| r.split("</tr>").next())
            .expect("port 1 has a row");
        assert!(row.contains("data-locked=\"1\""), "{row}");
        assert!(row.contains("in LACP group Trk3"), "{row}");
        assert!(!row.contains("Apply"), "a locked row has nothing to apply");
        // Port 2, which is not in a trunk, does.
        assert!(html.contains("saveMembers('2'"), "an ordinary row can be applied");
    }

    /// A trunk **group** row is not a member row.  `trunk_group` is set on a
    /// group's *members* (from `show trunks`), so a lock that only looks there
    /// leaves `Trk1` itself live — and Apply on that row sends
    /// `vlan 225 tagged Trk1`, a form this tool has never exercised.
    #[test]
    fn a_trunk_group_row_is_locked_too() {
        let mut s = fixture();
        let mut g = Port::new("Trk1");
        g.tagged_vlans = vec![232];
        s.ports.insert("Trk1".into(), g);
        let html = dashboard(&manager_page(&s));
        let row = html
            .split("<tr id=\"mrowTrk1\"")
            .nth(1)
            .and_then(|r| r.split("</tr>").next())
            .expect("the group has a row");
        assert!(row.contains("data-locked=\"1\""), "{row}");
        assert!(!row.contains("Apply"), "a group row has nothing to apply");
        assert!(row.contains("link-aggregation group"), "and it says why");
        // The ordinary port in the same fixture is still editable.
        assert!(html.contains("saveMembers('2'"));
    }

    #[test]
    fn a_read_only_session_locks_the_matrix() {
        let s = fixture();
        let html = dashboard(&read_only_page(&s));
        // The script always *defines* the helper; what must be absent is a call.
        assert!(!html.contains("return saveMembers("), "no writes to offer");
        assert!(html.contains("read-only session"));
    }

    /// There used to be a second, narrower VLAN dropdown in the port table.  Two
    /// controls for one setting, with different rules about tagged memberships,
    /// is one too many — the matrix is the one that knows the whole row.
    #[test]
    fn the_port_table_no_longer_offers_a_second_vlan_dropdown() {
        let s = fixture();
        let html = dashboard(&manager_page(&s));
        assert!(!html.contains("pv1"), "the duplicate dropdown is gone");
        assert!(!html.contains("Set VLAN"));
        // The name editor stays: it is a different setting, and a single field.
        assert!(html.contains("pn1"));
    }

    /// A switch that is down has to show up on the landing page as an error, not
    /// as a row of zeroes.
    #[test]
    fn the_index_page_shows_which_switch_is_down() {
        let mut r = rows("switch1");
        r[2].error = Some("timed out".to_string());
        let html = index(&r, "cjdell", None);
        assert!(html.contains("href=\"/switch/switch3\""));
        assert!(html.contains("timed out"));
    }
}
