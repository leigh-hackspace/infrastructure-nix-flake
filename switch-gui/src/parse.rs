//! Parsers for the ProCurve CLI's fixed-width tables.
//!
//! Every fixture in the tests below is real output from the switch at
//! 10.3.1.2 (J9280A 2510G-48, Y.11.44), captured over Telnet — including the
//! two things that make naive splitting wrong: `AD Lab` (a VLAN name with a
//! space in it) and `5-Trk3` (a trunk member appearing in the counters table
//! under a compound name).
//!
//! The rule used throughout: **split from the right when a column can contain
//! spaces**, and never index by character offset.  Fixed-width slicing breaks
//! the moment the CLI widens a column for a longer value, whereas "the last
//! field is Jumbo, the one before is Status, the middle is the name" survives.

use crate::model::{Counters, Member, StpPort, SystemInfo, TrunkMember, Vlan};
use std::collections::BTreeMap;

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

/// The table of `show vlan`:
///
/// ```text
///   VLAN ID Name                 Status       Jumbo
///   ------- -------------------- ------------ -----
///   1       DEFAULT              Port-based   No
///   232     AD Lab               Port-based   No
/// ```
pub fn vlans(out: &str) -> Vec<Vlan> {
    let mut rows = Vec::new();
    for line in out.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() || t.starts_with("Status and Counters") {
            continue;
        }
        let fields: Vec<&str> = t.split_whitespace().collect();
        // id, name (>=1 word), status, jumbo
        if fields.len() < 4 {
            continue;
        }
        let id: u32 = match fields[0].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let jumbo = fields[fields.len() - 1].to_string();
        let status = fields[fields.len() - 2].to_string();
        if !jumbo.eq_ignore_ascii_case("yes") && !jumbo.eq_ignore_ascii_case("no") {
            continue;
        }
        if !status.contains("based") && !status.contains("Protocol") {
            continue;
        }
        let name = fields[1..fields.len() - 2].join(" ");
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
        if !f[1].chars().all(|c| c.is_ascii_digit() || c == ',' || c == '.') {
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
pub fn stp_ports(out: &str) -> Vec<StpPort> {
    let mut rows = Vec::new();
    for line in out.lines() {
        if !line.contains('|') {
            continue;
        }
        let parts: Vec<&str> = line.split('|').map(|p| p.trim()).collect();
        if parts.len() < 2 {
            continue;
        }
        let left: Vec<&str> = parts[0].split_whitespace().collect();
        if left.is_empty() || left.len() > 2 || !is_port_token(left[0]) {
            continue;
        }
        let mid: Vec<&str> = parts[1].split_whitespace().collect();
        if mid.len() < 3 {
            continue;
        }
        rows.push(StpPort {
            port: left[0].to_string(),
            media: left.get(1).copied().unwrap_or_default().to_string(),
            state: mid[2].to_string(),
        });
    }
    rows
}

/// A port or trunk-group token as the CLI prints it: `1`, `48`, `Trk3`,
/// `Trk10`.  Used to tell a table row from a header or a stray line.
fn is_port_token(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix("Trk") {
        return !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit());
    }
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    #[test]
    fn the_right_hand_column_is_read_too() {
        let kv = key_values(SHOW_SYSTEM);
        assert_eq!(kv.get("Base MAC Addr").map(String::as_str), Some("643150-bf8c40"));
        assert_eq!(kv.get("Serial Number").map(String::as_str), Some("CN117DG07S"));
        assert_eq!(kv.get("Memory   - Total").map(String::as_str), Some("23,556,880"));
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
}
