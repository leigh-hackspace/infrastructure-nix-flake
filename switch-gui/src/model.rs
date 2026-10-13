//! The switch's configuration as the GUI sees it.
//!
//! Every field here comes out of a `show` command on the ProCurve CLI; the
//! parsers in [`crate::parse`] fill them in.  Everything is `String` rather
//! than an enum on purpose: the CLI truncates wide columns with `...`, and a
//! firmware revision that renames a state (`Disabled` → whatever) should make
//! the GUI show the unexpected word, not fail to load the page.
//!
//! Port identity is a string, not a number: the switch calls its physical
//! ports `1`..`48` and its trunk groups `Trk1`..`Trk8`, and both appear in the
//! same VLAN membership columns.

use std::collections::BTreeMap;

/// One row of `show vlan`.
#[derive(Debug, Clone)]
pub struct Vlan {
    pub id: u32,
    pub name: String,
    /// `Port-based` or `Protocol-based`.
    pub status: String,
    pub jumbo: String,
}

/// One row of `show vlan <id>` — a port's membership of one VLAN.
#[derive(Debug, Clone)]
pub struct Member {
    /// Port or trunk-group name, e.g. `12` or `Trk1`.
    pub port: String,
    /// `Tagged`, `Untagged` or `Forbidden`.
    pub mode: String,
    /// `Up` or `Down` — the link state, which `show vlan <id>` reports for
    /// every member and which is the only per-port liveness an operator wants.
    pub link: String,
}

/// One row of `show interfaces` (which is the *port counters* table).
///
/// Every field is the CLI's own column, in its own order, and the parser fills all
/// seven — including the port token exactly as printed, which is how a member of a
/// trunk group arrives (`5-Trk3`: the key the row is stored under is the physical
/// port, the group is only in the label).
///
/// Only two of them are stored and shown: `errors_rx` and `drops_rx` are what an
/// operator scans a bad cable for, and the rest are cumulative counters a snapshot
/// taken every few seconds cannot interpret without its predecessor. They are kept
/// rather than dropped at parse time so the row here is the row the switch printed
/// — a parser that silently discarded four columns could not be tested against the
/// live capture, which is what caught the 2824's extra `Voice` column.
#[allow(dead_code)] // parsed whole on purpose — see the note above
#[derive(Debug, Clone)]
pub struct Counters {
    /// The port as printed, which for a trunk member names the group too.
    pub port: String,
    pub total_bytes: String,
    pub total_frames: String,
    pub errors_rx: String,
    pub drops_rx: String,
    pub flow_ctrl: String,
    pub bcast_limit: String,
}

/// One row of `show trunks`.
#[derive(Debug, Clone)]
pub struct TrunkMember {
    pub port: String,
    /// The configured port name — the only place the Operator context exposes
    /// names at all, and only for ports in a trunk.
    pub name: String,
    /// Media type, e.g. `100/1000T`.
    pub media: String,
    /// `Trk1`..`Trk8`.
    pub group: String,
    /// `LACP`, `Trunk` or `Distributed`.
    pub group_type: String,
}

/// One row of the port table in `show spanning-tree`.
#[derive(Debug, Clone)]
pub struct StpPort {
    pub port: String,
    pub media: String,
    /// `Forwarding`, `Disabled`, `Learning`, ...
    pub state: String,
}

/// `show system`, the `Key : Value` lines we care about.
///
/// Parsed whole, stored partly. `contact`, `rom`, `mac` and `cpu` are read because
/// they are on the same lines and a parser that skipped them could not be checked
/// against the live capture; they are not stored because the page has nowhere to
/// put them and a shadow copy of a field nobody reads is a field nobody notices
/// going stale.
#[allow(dead_code)] // parsed whole on purpose — see the note above
#[derive(Debug, Clone, Default)]
pub struct SystemInfo {
    pub name: String,
    /// The `System Contact` string, which on this network is the admin's email —
    /// the switch's own idea of who to call.  Read, not stored: the GUI already
    /// knows who it is talking to.
    pub contact: String,
    pub location: String,
    pub software: String,
    /// The ROM image, which is what to quote to HP when `Software revision` is
    /// already current and the bug is still there.  Read, not stored.
    pub rom: String,
    /// The base MAC, the one thing on this page that identifies the silicon rather
    /// than the configuration — and so the only field that can prove two switches
    /// with the same `System Name` are two boxes.  Read, not stored.
    pub mac: String,
    pub serial: String,
    pub uptime: String,
    /// The busy percentage, which a snapshot taken every few seconds cannot make
    /// sense of on its own.  Read, not stored.
    pub cpu: String,
}

/// One physical port or trunk group, merged out of every table above.
///
/// The merge is what makes the GUI worth having: the switch spreads a port's
/// name, its link state and its VLAN membership over four different commands,
/// and none of them has all three.
#[derive(Debug, Clone)]
pub struct Port {
    pub id: String,
    /// Empty unless the port is in a trunk (`show trunks` is the only
    /// Operator-readable source of port names).
    pub name: String,
    pub media: String,
    pub stp_state: String,
    /// Set when `stp_state` was copied from this port's trunk group rather
    /// than measured for the port itself — `show spanning-tree` has a row for
    /// `Trk3` and none for its members.  The page says so in a tooltip.
    pub stp_state_from: String,
    /// Link state from the VLAN membership tables: `Some(true)` when the port
    /// is `Up` in every VLAN it belongs to.  This is what tells you a cable is
    /// out; `stp_state` says what STP is doing with the port, which is not the
    /// same question.
    pub link_up: Option<bool>,
    pub trunk_group: String,
    pub trunk_type: String,
    /// The VLAN the port is `Untagged` in — its PVID, the access VLAN.
    pub untagged_vlan: Option<u32>,
    /// Every VLAN the port is `Tagged` in.
    pub tagged_vlans: Vec<u32>,
    /// Whether the port is `no shutdown` in the running config.
    ///
    /// `show interfaces`, `show spanning-tree` and `show vlan <id>` all describe
    /// a shut port as if it were merely down, so this is not a link state: it is
    /// the configuration.  Only `show running-config` reports it, which is also
    /// the read that supplies port names (see [`crate::parse::port_settings`]).
    pub enabled: bool,
    pub counters: Option<Counters>,
}

impl Port {
    /// A port known only by name — the starting point before the tables are
    /// merged into it.
    pub fn new(id: &str) -> Port {
        Port {
            id: id.to_string(),
            name: String::new(),
            media: String::new(),
            stp_state: String::new(),
            stp_state_from: String::new(),
            link_up: None,
            trunk_group: String::new(),
            trunk_type: String::new(),
            untagged_vlan: None,
            tagged_vlans: Vec::new(),
            enabled: true,
            counters: None,
        }
    }

    /// `5` or `Trk1` — the shortest label that fits a table cell.
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            self.id.clone()
        } else {
            format!("{} · {}", self.id, self.name)
        }
    }
}
/// Everything the dashboard shows, plus how old it is.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Which switch this snapshot came from. With three switches a JSON snapshot
    /// that does not name its switch cannot be told apart from another one's.
    pub switch: String,
    pub system: SystemInfo,
    pub vlans: Vec<Vlan>,
    pub ports: BTreeMap<String, Port>,
    /// Commands that failed while building this snapshot.  A partial snapshot
    /// with a visible error beats a page that looks healthy and is stale.
    pub warnings: Vec<String>,
}

impl Snapshot {
    /// Ports sorted the way a switch operator reads a patch panel: `Trk1`
    /// after `48`, not interleaved by string comparison.
    pub fn sorted_ports(&self) -> Vec<&Port> {
        let mut v: Vec<&Port> = self.ports.values().collect();
        v.sort_by_key(|p| port_key(&p.id));
        v
    }
}

/// Sort key: physical ports first in numeric order, then trunk groups.
pub fn port_key(id: &str) -> (u8, u32) {
    match id.strip_prefix("Trk") {
        Some(n) => (1, n.parse().unwrap_or(0)),
        None => (0, id.parse().unwrap_or(0)),
    }
}

/// Whether a port id names a **trunk group** (`Trk1`) rather than a physical
/// port (`7`).
///
/// The switch uses the same token for both everywhere it talks: `show vlan 225`
/// lists `Trk1` among its members, and the CLI takes `interface Trk1` and
/// `vlan 225 tagged Trk1` as readily as the port forms.  So a group arrives
/// through the parsers as a port-shaped row, and anything that means "a physical
/// port" has to know which of the two it is holding.
///
/// This is **not** the same question as "is a trunk member": that is
/// [`Port::trunk_group`], and it is filled in on the group's *members* from
/// `show trunks` — never on the group's own row.  Checking one and assuming it
/// covers the other is how a group's row ends up looking like an ordinary port.
///
/// The rule itself lives in [`crate::dto::is_trunk_group`], because the SPA has to
/// lock the same rows it locks here and a second copy is a rule that can drift.
pub fn is_trunk_group(id: &str) -> bool {
    crate::dto::is_trunk_group(id)
}

/// The two CLI contexts the switch will put us in.
///
/// `Operator` is read-only: it has `show`, `ping`, `traceroute`, `enable` and
/// nothing else, so every write the GUI offers is refused.  `Manager` is the
/// context that actually accepts configuration.
///
/// `Unknown` is the state before a session has been seen — the value the shadow
/// copy starts a switch row with, and what the page shows for a switch that has
/// never been dialled. Without it the column would have to hold `operator`, which
/// reads as "we reached it and it refused writes" — the one thing the tool must
/// not claim about a switch it has not spoken to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Privilege {
    #[default]
    Unknown,
    Operator,
    Manager,
}

impl Privilege {
    /// The page's prose for the level.
    pub fn label(self) -> &'static str {
        match self {
            Privilege::Unknown => "unknown",
            Privilege::Operator => "Operator (read-only)",
            Privilege::Manager => "Manager",
        }
    }

    pub fn is_manager(self) -> bool {
        matches!(self, Privilege::Manager)
    }

    /// The lowercase name stored in the shadow copy's `privilege` column.
    ///
    /// Separate from [`Privilege::label`] because the label is prose for the page
    /// ("Operator (read-only)") and this is a value a query compares against; the
    /// two changing together would be a silent `WHERE privilege = …` that matches
    /// nothing.
    pub fn slug(self) -> &'static str {
        match self {
            Privilege::Unknown => "unknown",
            Privilege::Operator => "operator",
            Privilege::Manager => "manager",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trunks_sort_after_physical_ports() {
        let mut ids = vec!["48", "Trk1", "9", "Trk10", "1"];
        ids.sort_by_key(|i| port_key(i));
        assert_eq!(ids, vec!["1", "9", "48", "Trk1", "Trk10"]);
    }

    #[test]
    fn a_garbage_port_name_sorts_last_in_its_group() {
        // Never panics, and does not sort as 0 in front of everything.
        assert_eq!(port_key("TrkX"), (1, 0));
        assert_eq!(port_key("??"), (0, 0));
    }

    #[test]
    fn trunk_groups_are_told_apart_from_ports() {
        assert!(is_trunk_group("Trk1"));
        assert!(is_trunk_group("Trk10"));
        assert!(!is_trunk_group("1"));
        assert!(!is_trunk_group("48"));
        assert!(!is_trunk_group("Trk"), "a group always has a number");
        assert!(!is_trunk_group("TrkX"), "and it is always digits after it");
        assert!(!is_trunk_group(""));
    }
}
