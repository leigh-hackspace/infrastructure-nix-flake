//! Parsers for the ProCurve CLI's fixed-width tables.
//!
//! Every fixture in the tests below is real output captured over Telnet from a
//! switch in this network, and there are two firmwares to satisfy:
//!
//! * `10.3.1.2` / `10.3.1.3` — J9280A 2510G-48, **Y.11.44**
//! * `10.3.1.4` — J4903A 2824, **I.10.107**
//!
//! They are not the same tables. The 2824's `show vlan` has a `Voice` column
//! the 2510 does not, and its `show spanning-tree` puts `Cost`, `Priority` and
//! `State` in the same `|`-block as the port instead of the second one. Both
//! are caught by fixtures. What makes naive splitting wrong either way: `AD Lab`
//! (a VLAN name with a space in it) and `5-Trk3` (a trunk member appearing in
//! the counters table under a compound name).
//!
//! The rule used throughout: **split from the right when a column can contain
//! spaces**, and never index by character offset.  Fixed-width slicing breaks
//! the moment the CLI widens a column for a longer value, whereas "the last
//! field is Jumbo, the one before is Status, the middle is the name" survives.
//!
//! One byte to know about before writing a parser that cares about leading
//! whitespace: the CLI ends every line with **LF** and then CR-paints to the
//! next row, so `str::lines()` yields `"\r   name \"AP3\""` — every line begins
//! with a carriage return, not a character.  Trailing-`trim()`ing hides this,
//! which is why the column parsers are fine and [`port_names`], which asks
//! whether a line is indented, was not.

use crate::model::{Counters, Member, StpPort, SystemInfo, TrunkMember, Vlan};
use std::collections::BTreeMap;

// The whole module reads the CLI's own text, and every parser here is `pub` only
// so `switch.rs` and the tests can reach it.  Nothing outside this crate consumes
// it, so an unused parser is dead code rather than an API, and rustc is right to
// say so: `#![allow(dead_code)]` here would hide a parser that stopped being
// wired up, which is exactly the bug class this module has fixtures for.

/// The `Key : Value` lines from `show system`.
///
/// The key names are the switch's own, verified against its output — several of
/// them are not what you would guess (`Up Time`, not `Uptime`; `Base MAC Addr`,
/// not `MAC address`), and half of them only exist in the right-hand column.
pub fn system_info(out: &str) -> SystemInfo {
    let kv = key_values(out);
    let get = |k: &str| kv.get(k).cloned().unwrap_or_default();
    SystemInfo {
        name: get("System Name"),
        contact: get("System Contact"),
        location: get("System Location"),
        software: get("Software revision"),
        rom: get("ROM Version"),
        mac: get("Base MAC Addr"),
        serial: get("Serial Number"),
        uptime: get("Up Time"),
        cpu: get("CPU Util (%)"),
    }
}

/// `Key : Value` pairs from `show system`.
///
/// That command is **two columns wide**, and after the ANSI is stripped the two
/// halves arrive on one line:
///
/// ```text
///   Software revision  : Y.11.44          Base MAC Addr      : 643150-bf8c40
///   ROM Version        : N.10.02          Serial Number      : CN117DG07S
///   Up Time            : 4 days           Memory   - Total   : 23,556,880
/// ```
///
/// So a line is split on every ` : ` and each middle segment is cut at its first
/// run of two or more spaces: that run is the gutter between the value and the
/// next column's key.  A value that legitimately contained two spaces in a row
/// would be truncated here, which is the trade for reading `Base MAC Addr` and
/// `Serial Number` at all — they are only ever in the right-hand column.
pub fn key_values(out: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in out.lines() {
        let line = line.trim_end();
        if !line.contains(" : ") {
            continue;
        }
        let segments: Vec<&str> = line.split(" : ").collect();
        for i in 0..segments.len() - 1 {
            // The key of the first pair is the whole leading segment; the key
            // of every later pair is what followed the previous gutter.
            let key = if i == 0 {
                segments[i].trim()
            } else {
                match gutter(segments[i]) {
                    Some(g) => segments[i][g..].trim(),
                    // No gutter means the previous value was simply wide, and
                    // there is no further key on this line.
                    None => continue,
                }
            };
            // A table rule (`---- + ----`) or a heading, not a key.
            if key.is_empty() || key.chars().all(|c| c == '-' || c == '+' || c == ' ') {
                continue;
            }
            let value = match gutter(segments[i + 1]) {
                Some(g) => segments[i + 1][..g].trim(),
                None => segments[i + 1].trim(),
            };
            map.insert(key.to_string(), value.to_string());
        }
    }
    map
}

/// The offset of the column gutter — the first run of two or more spaces.  It
/// separates a value from the next column's key.
fn gutter(s: &str) -> Option<usize> {
    s.find("  ")
}

/// How many columns come after the `Name` column in the `show vlan` header.
///
/// The two firmwares disagree:
///
/// ```text
///   VLAN ID Name                 Status       Jumbo            (Y.11.44)
///   802.1Q VLAN ID Name         Status       Voice Jumbo       (I.10.107)
/// ```
///
/// `Name` is the only column that can hold a space (`AD Lab`), so the row is
/// read from the right and the count has to come from the header: assuming two
/// trailing columns makes the I.10.107 row's `Status` read as `No` and drops
/// every VLAN on that switch.
fn name_trailing(out: &str) -> usize {
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        // A header, not a row: `Name : DEFAULT` from `show vlan <id>` has the
        // word but is not the table.
        if !(line.contains("Name") && line.contains("Jumbo")) {
            continue;
        }
        if let Some(i) = f.iter().position(|x| *x == "Name") {
            if f.len() > i + 1 {
                return f.len() - i - 1;
            }
        }
    }
    2
}

/// The table of `show vlan`:
///
/// ```text
///   VLAN ID Name                 Status       Jumbo
///   ------- -------------------- ------------ -----
///   1       DEFAULT              Port-based   No
///   232     AD Lab               Port-based   No
/// ```
pub fn vlans(out: &str) -> Vec<Vlan> {
    let trailing = name_trailing(out);
    let mut rows = Vec::new();
    for line in out.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() || t.starts_with("Status and Counters") {
            continue;
        }
        let fields: Vec<&str> = t.split_whitespace().collect();
        // id, name (>=1 word), then the trailing columns
        if fields.len() < trailing + 2 {
            continue;
        }
        let id: u32 = match fields[0].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let jumbo = fields[fields.len() - 1].to_string();
        let status = fields[fields.len() - trailing].to_string();
        if !jumbo.eq_ignore_ascii_case("yes") && !jumbo.eq_ignore_ascii_case("no") {
            continue;
        }
        if !status.contains("based") && !status.contains("Protocol") {
            continue;
        }
        let name = fields[1..fields.len() - trailing].join(" ");
        rows.push(Vlan {
            id,
            name,
            status,
            jumbo,
        });
    }
    rows
}

/// The port table of `show vlan <id>`:
///
/// ```text
///   Port Information Mode     Unknown VLAN Status
///   ---------------- -------- ------------ ----------
///   1                Untagged Learn        Up
///   Trk1             Untagged Learn        Up
/// ```
pub fn members(out: &str) -> Vec<Member> {
    let mut rows = Vec::new();
    let mut in_table = false;
    for line in out.lines() {
        let t = line.trim();
        if t.starts_with("Port Information") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if t.starts_with("----") || t.is_empty() {
            continue;
        }
        // The table is the last thing the command prints; a trailing blank
        // line and the prompt follow, and the prompt has already been removed
        // by the caller.  Anything that is not a port name ends the table.
        let f: Vec<&str> = t.split_whitespace().collect();
        if f.len() < 4 || !is_port_token(f[0]) {
            if f.len() > 1 {
                break;
            }
            continue;
        }
        rows.push(Member {
            port: f[0].to_string(),
            mode: f[1].to_string(),
            link: f[3].to_string(),
        });
    }
    rows
}

/// The table of `show interfaces` (which is *Port Counters*):
///
/// ```text
///   Port    Total Bytes  Total Frames Errors Rx    Drops Rx     Flow  Bcast
///   1       2,993,652... 1,219,890... 0            0            off   0
///   5-Trk3  2,995,318... 183,610,709  0            0            off   0
/// ```
///
/// A trunk member is listed under `5-Trk3`; it is reported under the physical
/// port, because that is the row an operator is looking for when a cable is
/// bad.  The trunk itself is also a member of VLANs under its own name.
pub fn counters(out: &str) -> Vec<(String, Counters)> {
    let mut rows = Vec::new();
    for line in out.lines() {
        let t = line.trim_end();
        let f: Vec<&str> = t.split_whitespace().collect();
        if f.len() < 7 {
            continue;
        }
        let (port, trunk) = match f[0].split_once('-') {
            Some((p, g)) => (p.to_string(), Some(g.to_string())),
            None => (f[0].to_string(), None),
        };
        if !is_port_token(&port) {
            continue;
        }
        // Numeric columns: reject the header row (whose fields are words).
        if !f[1]
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
        {
            continue;
        }
        rows.push((
            port,
            Counters {
                port: f[0].to_string(),
                total_bytes: f[1].to_string(),
                total_frames: f[2].to_string(),
                errors_rx: f[3].to_string(),
                drops_rx: f[4].to_string(),
                flow_ctrl: f[5].to_string(),
                bcast_limit: f[6].to_string(),
            },
        ));
        let _ = trunk;
    }
    rows
}

/// The table of `show trunks`:
///
/// ```text
///   Port | Name                             Type      | Group Type
///   5    | NAS2 Port 1                      100/1000T | Trk3  LACP
/// ```
pub fn trunks(out: &str) -> Vec<TrunkMember> {
    let mut rows = Vec::new();
    for line in out.lines() {
        if !line.contains('|') {
            continue;
        }
        let parts: Vec<&str> = line.split('|').map(|p| p.trim()).collect();
        if parts.len() != 3 {
            continue;
        }
        if !is_port_token(parts[0]) {
            continue;
        }
        let mid: Vec<&str> = parts[1].split_whitespace().collect();
        if mid.is_empty() {
            continue;
        }
        let media = mid[mid.len() - 1].to_string();
        let name = mid[..mid.len() - 1].join(" ");
        let right: Vec<&str> = parts[2].split_whitespace().collect();
        rows.push(TrunkMember {
            port: parts[0].to_string(),
            name,
            media,
            group: right.first().copied().unwrap_or("").to_string(),
            group_type: right.get(1).copied().unwrap_or("").to_string(),
        });
    }
    rows
}

/// The port table of `show spanning-tree`:
///
/// ```text
///   Port  Type      | Cost      rity  State      | Bridge        Time  PtP Edge
///   1     100/1000T | 20000     128   Forwarding | 643150-bf8c40 2     Yes Yes
///   2     100/1000T | Auto      128   Disabled   |
///   Trk1            | 20000     64    Forwarding | 643150-bf8c40 2     Yes No
/// ```
///
/// This is the only Operator-readable source of a per-port link state for
/// *every* port (`Forwarding`/`Disabled`/`Learning`/…), which is why the GUI
/// reads it despite being about STP.
///
/// A trunk group's row has **no Type column** — the left side is just `Trk1` —
/// and its member ports are not listed at all, so `show trunks` is what puts
/// ports 5, 6, 47 and 48 on the page.
///
/// Which `|`-block holds `State` is firmware-dependent, so it is read off the
/// header rather than assumed: Y.11.44 puts `Cost/Priority/State` in the second
/// block, I.10.107 puts `Cost/Priority/State` in the first one with the port.
/// Assuming the second block reads every 2824 port's state as `128` (its
/// priority) — see the fixture below.
///
/// The state is always the **last** column of its block, which is what survives
/// a row whose later columns are empty (`2 ... Disabled   |`).
pub fn stp_ports(out: &str) -> Vec<StpPort> {
    let block = stp_state_block(out);
    let mut rows = Vec::new();
    for line in out.lines() {
        if !line.contains('|') {
            continue;
        }
        let parts: Vec<&str> = line.split('|').map(|p| p.trim()).collect();
        let left: Vec<&str> = parts[0].split_whitespace().collect();
        if left.is_empty() || !is_port_token(left[0]) {
            continue;
        }
        let cols: Vec<&str> = parts
            .get(block)
            .map(|b| b.split_whitespace().collect())
            .unwrap_or_default();
        if cols.is_empty() {
            continue;
        }
        rows.push(StpPort {
            port: left[0].to_string(),
            media: left.get(1).copied().unwrap_or_default().to_string(),
            state: cols[cols.len() - 1].to_string(),
        });
    }
    rows
}

/// The `|`-block index containing the `State` column, from the header row.
fn stp_state_block(out: &str) -> usize {
    for line in out.lines() {
        if !line.contains('|') || !line.contains("State") {
            continue;
        }
        for (i, block) in line.split('|').enumerate() {
            if block.split_whitespace().any(|f| f == "State") {
                return i;
            }
        }
    }
    1
}

/// A port or trunk-group token as the CLI prints it: `1`, `48`, `Trk3`,
/// `Trk10`.  Used to tell a table row from a header or a stray line.
fn is_port_token(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix("Trk") {
        return !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit());
    }
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Port names out of `show running-config`.
///
/// On Y.11.44 this is the **only** place a port name is reported.  `show vlan
/// <id>` has a `Name` column and it is blank for every port whose membership is
/// dynamic (`Learn`) — which on 10.3.1.2 is every port — and there is no
/// `show port name` to ask instead: it answers `Ambiguous input: port`.  The
/// names live in the running config, one block per configured port:
///
/// ```text
/// interface 22
///    name "AP3"
/// exit
/// ```
///
/// Only `interface` blocks count.  A `vlan` block has a `name` line of its own
/// (`vlan 225` / `name "Shared"`), and reading that as a port name is the
/// obvious mistake with this input, so a block is open only between
/// `interface <id>` and the line that ends it, and an indented `name` outside a
/// block is ignored.  A block ends at `exit` or at any other unindented line.
///
/// And the line has to be stripped of its leading CR *before* asking whether it
/// is indented.  This CLI ends every line with LF and then CR-paints its way to
/// the next row, so `lines()` hands back `"\r   name \"AP3\""`: the indentation
/// is real, but it is not at the start of the string.  Testing
/// `starts_with(' ')` on the raw line made every line look top-level, every
/// block close immediately, and the parser return no names at all while the
/// command itself succeeded — which is why this is the one parser here with a
/// CR-shaped test.
///
/// The rest of the input is deliberately not returned: `show running-config`
/// carries the RADIUS shared secret and the local user hashes.  This hands back
/// names and nothing else, and [`crate::switch::Switch::secret_command`] keeps
/// the raw text out of the error path too.
/// The port names in `show running-config`.
///
/// Not a live path: [`port_settings`] reads the same blocks and `merge_settings`
/// needs `enabled` from them too, so the session reads the running config once and
/// takes both facts from that.  It stays because it is the tightest statement of
/// what this command is the only source *for* — the name of a dynamically-learned
/// port, which `show vlan <id>` leaves blank for all fifty ports on switch1 — and
/// the tests below read through it so they cannot drift from that.
#[allow(dead_code)] // the tests read through it; `port_settings` is the live path
pub fn port_names(out: &str) -> BTreeMap<String, String> {
    port_settings(out)
        .into_iter()
        .map(|(id, s)| (id, s.name))
        .filter(|(_, name)| !name.is_empty())
        .collect()
}

/// What an `interface <id>` block in `show running-config` says about a port.
///
/// Split out of [`port_names`] because the same block is the only source for two
/// facts the GUI needs, and reading the running config twice against a 1990s CPU
/// is not free: the port's name, and whether it is administratively shut.
///
/// The `shutdown` line is why this exists.  Every other read treats a shut port
/// as merely a down one — `show interfaces` counts it, `show spanning-tree` has
/// no row or a `Disabled` one that a port in a blocked state also produces, and
/// `show vlan <id>` reports `Down` for a missing cable identically — so without
/// the running config the GUI cannot tell "nobody is plugged in" from "an
/// operator turned this port off", and cannot show the state its own
/// `disable`/`enable` writes produce (the CLI spells the setting `shutdown` in the
/// running config and `disable` on the command line).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortSettings {
    pub name: String,
    /// Absent `shutdown` means up: that is the switch's default, and the
    /// running config only writes what differs from it.
    pub enabled: bool,
}

pub fn port_settings(out: &str) -> BTreeMap<String, PortSettings> {
    let mut ports: BTreeMap<String, PortSettings> = BTreeMap::new();
    let mut port: Option<&str> = None;
    for line in out.lines() {
        let line = line.trim_start_matches('\r');
        // The CLI indents a block's settings; column 0 is a new block.
        if !(line.starts_with(' ') || line.starts_with('\t')) {
            port = match line.trim().strip_prefix("interface") {
                Some(rest) if is_port_token(rest.trim()) => Some(rest.trim()),
                _ => None,
            };
            continue;
        }
        let Some(id) = port else { continue };
        // Every port the running config has a block for is a port that has some
        // non-default setting; `enabled` starts true and only a `shutdown` line
        // turns it off, so a block with no `shutdown` still has to be recorded.
        let entry = ports.entry(id.to_string()).or_insert(PortSettings {
            name: String::new(),
            enabled: true,
        });
        let body = line.trim();
        if let Some(rest) = body.strip_prefix("name ") {
            if let Some(name) = unquoted(rest) {
                entry.name = name;
            }
        } else if body == "shutdown" {
            entry.enabled = false;
        }
    }
    ports
}

/// The inside of a double-quoted CLI argument, or `None` if it is not quoted.
///
/// The CLI has no escape mechanism inside a name (which is why
/// `commands::check_label` refuses a quote rather than escaping it), so the
/// first and last quote in the value are the delimiters.
fn unquoted(s: &str) -> Option<String> {
    let s = s.trim();
    let rest = s.strip_prefix('"')?;
    let end = rest.rfind('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Every fixture in this module, in one string.
    ///
    /// `switch::is_prompt` has to read a prompt as a *suffix* of a line — that
    /// is the only way it sees the prompt the CLI paints on the end of an echoed
    /// command (`interface 23 name "x"switch1(config)#`), which is what a write
    /// answers. The cost is that a response line ending in `word#` would read as
    /// a prompt and cut a response short, so the switch's own captured output is
    /// the thing to check that against.
    pub fn corpus() -> String {
        [
            SHOW_VLAN,
            SHOW_VLAN_1,
            SHOW_INTERFACES,
            SHOW_TRUNKS,
            SHOW_STP,
            SHOW_SYSTEM,
            SHOW_VLAN_2824,
            SHOW_STP_2824,
        ]
        .join("\n")
    }

    /// Real `show vlan` output from 10.3.1.2 (ANSI stripped, MORE markers
    /// removed) — including a VLAN whose name contains a space.
    const SHOW_VLAN: &str = r#" Status and Counters - VLAN Information

  Maximum VLANs to support : 64
  Primary VLAN : DEFAULT
  Management VLAN :

  VLAN ID Name                 Status       Jumbo
  ------- -------------------- ------------ -----
  1       DEFAULT              Port-based   No
  50      Mill Network         Port-based   No
  228     VLAN228              Port-based   No
  232     AD Lab               Port-based   No
  1337    CS_VLAN_Paul         Port-based   No
"#;

    #[test]
    fn vlan_table_with_a_spaced_name() {
        let v = vlans(SHOW_VLAN);
        assert_eq!(v.len(), 5);
        assert_eq!((v[0].id, v[0].name.as_str()), (1, "DEFAULT"));
        assert_eq!((v[3].id, v[3].name.as_str()), (232, "AD Lab"));
        assert_eq!((v[4].id, v[4].name.as_str()), (1337, "CS_VLAN_Paul"));
        assert_eq!(v[0].status, "Port-based");
        assert_eq!(v[0].jumbo, "No");
    }

    #[test]
    fn vlan_table_ignores_the_preamble() {
        // "Primary VLAN : DEFAULT" has words in it but must not become a row.
        assert!(vlans(SHOW_VLAN).iter().all(|v| v.id != 0));
        assert_eq!(vlans("no table here").len(), 0);
    }

    /// Real `show vlan 1` output.
    const SHOW_VLAN_1: &str = r#" Status and Counters - VLAN Information - Ports - VLAN 1

  VLAN ID : 1
  Name : DEFAULT
  Status : Port-based
  Jumbo : No

  Port Information Mode     Unknown VLAN Status
  ---------------- -------- ------------ ----------
  1                Untagged Learn        Up
  15               Untagged Learn        Down
  46               Untagged Learn        Up
  Trk1             Untagged Learn        Up
  Trk3             Untagged Learn        Up
"#;

    #[test]
    fn vlan_membership() {
        let m = members(SHOW_VLAN_1);
        assert_eq!(m.len(), 5);
        assert_eq!(m[0].port, "1");
        assert_eq!(m[0].mode, "Untagged");
        assert_eq!(m[1].link, "Down");
        assert_eq!(m[4].port, "Trk3");
        let kv = key_values(SHOW_VLAN_1);
        assert_eq!(kv.get("Name").map(String::as_str), Some("DEFAULT"));
        assert_eq!(kv.get("VLAN ID").map(String::as_str), Some("1"));
    }

    #[test]
    fn membership_stops_at_the_end_of_the_table() {
        let with_tail = format!("{SHOW_VLAN_1}\n  Some other section\n    nonsense\n");
        assert_eq!(members(&with_tail).len(), 5);
    }

    /// Real `show interfaces` output, including the compound trunk row and the
    /// CLI's truncation of wide numbers with `...`.
    const SHOW_INTERFACES: &str = r#" Status and Counters - Port Counters

                                                              Flow  Bcast
  Port    Total Bytes  Total Frames Errors Rx    Drops Rx     Ctrl  Limit
  ------- ------------ ------------ ------------ ------------ ----- ------
  1       2,993,652... 1,219,890... 0            0            off   0
  2       688,085      1700         0            0            off   0
  5-Trk3  2,995,318... 183,610,709  0            0            off   0
  47-Trk1 1,307,972... 1,307,972... 0            0            off   0
"#;

    #[test]
    fn counters_keyed_by_physical_port() {
        let c = counters(SHOW_INTERFACES);
        assert_eq!(c.len(), 4);
        let (port, row) = &c[2];
        assert_eq!(port, "5");
        assert_eq!(row.port, "5-Trk3");
        assert_eq!(row.total_bytes, "2,995,318...");
        assert_eq!(row.errors_rx, "0");
        // The header row must not become a data row.
        assert!(c.iter().all(|(p, _)| p != "Port"));
    }

    /// Real `show trunks` output.
    const SHOW_TRUNKS: &str = r#" Load Balancing

  Port | Name                             Type      | Group Type
  ---- + -------------------------------- --------- + ----- -----
  5    | NAS2 Port 1                      100/1000T | Trk3  LACP
  6    | NAS2 Port 2                      100/1000T | Trk3  LACP
  47   | Switch2 Port 47                  100/1000T | Trk1  LACP
  48   | Switch2 Port 48                  100/1000T | Trk1  LACP
"#;

    #[test]
    fn trunk_rows_split_name_from_media() {
        let t = trunks(SHOW_TRUNKS);
        assert_eq!(t.len(), 4);
        assert_eq!(t[0].port, "5");
        assert_eq!(t[0].name, "NAS2 Port 1");
        assert_eq!(t[0].media, "100/1000T");
        assert_eq!(t[0].group, "Trk3");
        assert_eq!(t[0].group_type, "LACP");
        assert_eq!(t[3].name, "Switch2 Port 48");
    }

    /// Real `show spanning-tree` port rows, including the two trunk rows whose
    /// Type column is empty.
    const SHOW_STP: &str = r#"                  |           Prio             | Designated    Hello
  Port  Type      | Cost      rity  State      | Bridge        Time  PtP Edge
  ----- --------- + --------- ----- ---------- + ------------- ----- --- ----
  1     100/1000T | 20000     128   Forwarding | 643150-bf8c40 2     Yes Yes
  2     100/1000T | Auto      128   Disabled   |
  46    100/1000T | 20000     128   Forwarding | 001635-bb31e0 2     Yes No
  Trk1            | 20000     64    Forwarding | 643150-bf8c40 2     Yes No
  Trk3            | 20000     64    Forwarding | 643150-bf8c40 2     Yes Yes
"#;

    #[test]
    fn stp_rows() {
        let s = stp_ports(SHOW_STP);
        assert_eq!(s.len(), 5);
        assert_eq!(s[0].port, "1");
        assert_eq!(s[0].state, "Forwarding");
        assert_eq!(s[1].state, "Disabled");
        assert_eq!(s[2].media, "100/1000T");
    }

    /// A trunk row has no Type column; dropping it would leave the uplink with
    /// no state on the page.
    #[test]
    fn stp_trunk_rows_have_no_media() {
        let s = stp_ports(SHOW_STP);
        let trk1 = s.iter().find(|p| p.port == "Trk1").expect("Trk1 row");
        assert_eq!(trk1.media, "");
        assert_eq!(trk1.state, "Forwarding");
        assert!(s.iter().any(|p| p.port == "Trk3"));
    }

    /// Real `show system` output (ANSI stripped, page markers removed), which
    /// is two columns wide — the case the parser exists for.
    const SHOW_SYSTEM: &str = r#" Status and Counters - System Information

  System Name        : switch1
  System Contact     :
  System Location    : Rack1

  MAC Age Time (sec) : 300
  Time Zone          : 0
  Daylight Time Rule : None

  Software revision  : Y.11.44          Base MAC Addr      : 643150-bf8c40
  ROM Version        : N.10.02          Serial Number      : CN117DG07S

  Up Time            : 4 days           Memory   - Total   : 23,556,880
  CPU Util (%)       : 9                           Free    : 16,081,440
"#;

    #[test]
    fn system_info_reads_the_real_keys() {
        let s = system_info(SHOW_SYSTEM);
        assert_eq!(s.name, "switch1");
        assert_eq!(s.location, "Rack1");
        assert_eq!(s.contact, "");
        // The whole point: without cutting the gutter this value is
        // "Y.11.44          Base MAC Addr      : 643150-bf8c40".
        assert_eq!(s.software, "Y.11.44");
        assert_eq!(s.rom, "N.10.02");
        assert_eq!(s.uptime, "4 days");
        assert_eq!(s.cpu, "9");
    }

    /// `Base MAC Addr` and `Serial Number` only ever appear in the right-hand
    /// column, so a single-column parser silently reports them empty.
    /// Real `show vlan` output from the 2824 at 10.3.1.4 (I.10.107), which has
    /// a `Voice` column the 2510 does not.
    const SHOW_VLAN_2824: &str = r#" Status and Counters - VLAN Information

  Maximum VLANs to support : 256
  Primary VLAN : DEFAULT
  Management VLAN :

  802.1Q VLAN ID Name         Status       Voice Jumbo
  -------------- ------------ ------------ ----- -----
  1              DEFAULT      Port-based   No    No
  225            Shared       Port-based   No    No
  232            AD Lab       Port-based   No    No
"#;

    #[test]
    fn the_voice_column_does_not_shift_the_name() {
        let v = vlans(SHOW_VLAN_2824);
        assert_eq!(v.len(), 3, "the Voice column made every row fail");
        assert_eq!((v[0].id, v[0].name.as_str()), (1, "DEFAULT"));
        assert_eq!((v[2].id, v[2].name.as_str()), (232, "AD Lab"));
        assert_eq!(v[1].status, "Port-based");
        assert_eq!(v[1].jumbo, "No");
    }

    /// Real `show spanning-tree` port rows from the 2824: `Cost`, `Priority`
    /// and `State` are in the same `|`-block as the port, so the state is the
    /// last field of the first block, not the third field of the second.
    const SHOW_STP_2824: &str = r#"  Port  Type      Cost      Priority State      | Designated Bridge
  ----- --------- --------- -------- ---------- + -----------------
  1     100/1000T 200000    128      Forwarding | 001635-bb31e0
  5     100/1000T 20000     128      Disabled   |
"#;

    #[test]
    fn stp_rows_on_the_2824_layout() {
        let s = stp_ports(SHOW_STP_2824);
        assert_eq!(s.len(), 2);
        assert_eq!(
            (s[0].port.as_str(), s[0].state.as_str()),
            ("1", "Forwarding")
        );
        assert_eq!(s[0].media, "100/1000T");
        assert_eq!((s[1].port.as_str(), s[1].state.as_str()), ("5", "Disabled"));
    }

    /// Without the header the parser falls back to the Y.11.44 layout, which is
    /// what the fixture above is.
    #[test]
    fn stp_state_block_comes_from_the_header() {
        assert_eq!(stp_state_block(SHOW_STP), 1);
        assert_eq!(stp_state_block(SHOW_STP_2824), 0);
        assert_eq!(stp_state_block("no header"), 1);
    }

    #[test]
    fn the_right_hand_column_is_read_too() {
        let kv = key_values(SHOW_SYSTEM);
        assert_eq!(
            kv.get("Base MAC Addr").map(String::as_str),
            Some("643150-bf8c40")
        );
        assert_eq!(
            kv.get("Serial Number").map(String::as_str),
            Some("CN117DG07S")
        );
        assert_eq!(
            kv.get("Memory   - Total").map(String::as_str),
            Some("23,556,880")
        );
        let s = system_info(SHOW_SYSTEM);
        assert_eq!(s.mac, "643150-bf8c40");
        assert_eq!(s.serial, "CN117DG07S");
    }

    #[test]
    fn key_values_skips_rules_and_headings() {
        let kv = key_values("  ---- + ----\n Not a key\n  A : b\n");
        assert_eq!(kv.len(), 1);
        assert_eq!(kv.get("A").map(String::as_str), Some("b"));
    }

    #[test]
    fn port_tokens() {
        assert!(is_port_token("1"));
        assert!(is_port_token("48"));
        assert!(is_port_token("Trk10"));
        assert!(!is_port_token("Trk"));
        assert!(!is_port_token("Port"));
        assert!(!is_port_token(""));
    }

    /// Real `show running-config` from 10.3.1.2 (Y.11.44), trimmed to the parts
    /// that matter and with **every secret replaced** — the shape of the lines is
    /// what is pinned here, never their values.  Note the trailing spaces the CLI
    /// emits on `interface 1 ` and `name "GW Port 1" `.
    const SHOW_RUNNING_CONFIG: &str = r#"interface 1 
   name "GW Port 1" 
exit
interface 22 
   name "AP3" 
exit
interface 48 
   name "Switch2 Port 48" 
exit
interface 23 
   shutdown 
exit
interface 9 
   name "Camera 9" 
   shutdown 
exit
trunk 47-48 Trk1 LACP 
trunk 5-6 Trk3 LACP 
ip default-gateway 10.3.1.1 
snmp-server community "public" Unrestricted 
vlan 1 
   name "DEFAULT"
exit
vlan 225 
   name "Shared" 
   untagged 2,4,7,23,Trk1 
   tagged 1,3,8-18,22,45-46,Trk1 
exit
radius-server host 10.3.1.36
radius-server key <REDACTED-IN-THIS-REPO>
username operator password <REDACTED-IN-THIS-REPO>
"#;

    /// The other half of what an `interface` block is the only source for:
    /// whether the port is administratively shut.  Note the two shapes — a shut
    /// port with no name (23) must still be reported as shut, and a named port
    /// (9) must keep both facts.
    #[test]
    fn shutdown_comes_from_the_interface_blocks() {
        let s = port_settings(SHOW_RUNNING_CONFIG);
        assert_eq!(s.len(), 5, "{s:?}");
        assert!(
            s.get("1").map(|p| p.enabled).unwrap_or(false),
            "no shutdown line"
        );
        assert_eq!(s.get("1").map(|p| p.name.as_str()), Some("GW Port 1"));
        assert!(
            !s.get("23").map(|p| p.enabled).unwrap_or(true),
            "port 23 has a shutdown line"
        );
        assert_eq!(s.get("23").map(|p| p.name.as_str()), Some(""));
        assert_eq!(s.get("9").map(|p| p.name.as_str()), Some("Camera 9"));
        assert!(!s.get("9").map(|p| p.enabled).unwrap_or(true));
        // A VLAN block is not a port block, and its `name` is not a port name.
        assert!(!s.contains_key("225"), "the VLAN block leaked: {s:?}");
    }

    /// A port the running config never mentions is not in the map at all, which
    /// is what lets [`crate::switch`] keep the default (`enabled: true`) for it
    /// rather than inventing a state.
    #[test]
    fn an_unconfigured_port_has_no_settings_row() {
        let s = port_settings(SHOW_RUNNING_CONFIG);
        assert!(!s.contains_key("2"), "{s:?}");
        assert!(
            !s.contains_key("Trk1"),
            "a trunk line is not a block: {s:?}"
        );
    }

    #[test]
    fn names_come_from_the_interface_blocks() {
        let n = port_names(SHOW_RUNNING_CONFIG);
        // Four, not three: `interface 9` is a shut port that still has a name,
        // and `interface 23` is shut with no name at all (so it is not here, but
        // is in `port_settings`).
        assert_eq!(n.len(), 4, "{n:?}");
        assert_eq!(n.get("1").map(String::as_str), Some("GW Port 1"));
        assert_eq!(n.get("22").map(String::as_str), Some("AP3"));
        assert_eq!(n.get("48").map(String::as_str), Some("Switch2 Port 48"));
        assert_eq!(n.get("9").map(String::as_str), Some("Camera 9"));
        assert!(
            !n.contains_key("23"),
            "a shut port with no name has no name"
        );
    }

    /// The trap this parser exists to avoid: a `vlan` block has a `name` line
    /// exactly like an `interface` block's, and the running config is full of
    /// them.  Reading one as a port name would invent a port called `225`.
    #[test]
    fn a_vlan_blocks_name_is_not_a_port_name() {
        let n = port_names(SHOW_RUNNING_CONFIG);
        assert!(!n.contains_key("225"), "the VLAN name leaked: {n:?}");
        assert!(!n.contains_key("Shared"), "a name key, not a port: {n:?}");
        // `trunk 5-6 Trk3 LACP` is a top-level setting, not a block.
        assert!(!n.contains_key("5-6"));
    }

    /// `show running-config` is the only command this tool reads that carries
    /// secrets.  The parser returns names, so a secret has nowhere to go — but
    /// this is the line that would catch it if that ever changed.
    #[test]
    fn the_running_configs_secrets_are_not_returned() {
        for (port, name) in port_names(SHOW_RUNNING_CONFIG) {
            assert!(!name.contains("REDACTED"), "a secret leaked into a name");
            assert!(!port.contains("REDACTED"), "a secret leaked into a port id");
        }
    }

    #[test]
    fn running_config_quirks() {
        // A trailing space on the interface line, and on the name.
        let n = port_names("interface 7 \n   name \"NAS2 iLO\" \nexit\n");
        assert_eq!(n.get("7").map(String::as_str), Some("NAS2 iLO"));
        // A trunk group can be named too, even though this switch puts trunks
        // in a top-level `trunk 5-6 Trk3 LACP` line instead.
        let t = port_names("interface Trk1\n   name \"ESX1 LACP\"\nexit\n");
        assert_eq!(t.get("Trk1").map(String::as_str), Some("ESX1 LACP"));
        // An unquoted or malformed value is skipped rather than half-parsed.
        assert!(port_names("interface 7\n   name nope\nexit\n").is_empty());
        // A block with no name line contributes nothing.
        assert!(port_names("interface 7\n   speed-duplex auto\nexit\n").is_empty());
        // An indented name with no open block is not a port's.
        assert!(port_names("   name \"Orphan\"\n").is_empty());
        // `interface` with no port number opens nothing.
        assert!(port_names("interface\n   name \"Nope\"\n").is_empty());
        // The last `name` in a block wins, and a block ends at the next block.
        let two = port_names("interface 1\n   name \"A\"\ninterface 2\n   name \"B\"\n");
        assert_eq!(two.len(), 2);
        assert_eq!(two.get("1").map(String::as_str), Some("A"));
    }

    #[test]
    fn unquoted_needs_its_quotes() {
        assert_eq!(unquoted("\"AP3\" ").as_deref(), Some("AP3"));
        assert_eq!(unquoted("AP3"), None);
        assert_eq!(unquoted("\"open"), None);
    }

    /// The regression this exists for.  The CLI ends every line with LF and then
    /// CR-paints to the next row, so what `lines()` splits into is `"\r   name
    /// \"AP3\""`: asking whether the line is indented, on the raw line, answers
    /// "no" for every line, every `interface` block closes before its `name`,
    /// and the parser returns nothing — while the command succeeds and the page
    /// quietly shows no names at all.  Nothing else in the module notices, so
    /// nothing else would have caught it.
    #[test]
    fn every_line_arrives_behind_a_carriage_return() {
        let crlf = SHOW_RUNNING_CONFIG.replace('\n', "\n\r");
        assert!(
            crlf.contains("\r   name"),
            "the fixture is not shaped like the wire"
        );
        let n = port_names(&crlf);
        assert_eq!(n.len(), 4, "{n:?}");
        assert_eq!(n.get("22").map(String::as_str), Some("AP3"));
        // And still no VLAN name pretending to be a port.
        assert!(!n.contains_key("225"));
    }
}
