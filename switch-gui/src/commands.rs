//! Turning a GUI action into ProCurve CLI lines, and refusing the ones that
//! should not be sent.
//!
//! The commands follow the older ProCurve CLI this firmware speaks.  They run
//! inside **config mode** (`Switch::apply` enters it first) — at the plain
//! Manager Exec prompt every one of them answers `Invalid input` — and the
//! per-port ones use the one-shot `interface <n> <setting>` form, because at
//! config level `port 5 name "x"` is refused and entering the interface level
//! properly wedges the read (see [`interface`]).
//!
//! `name` / `enable` / `disable` are confirmed against the switch's own `?`
//! output and by applying a no-op rename. The VLAN commands come from the
//! platform's command reference and have **not been executed**; anything the
//! switch dislikes comes back as its own `Invalid input: …` text, surfaced
//! verbatim, and nothing here writes NVRAM, so a reboot undoes the change.
//!
//! Two guard rails are enforced here rather than trusted to the browser:
//! VLAN 1 cannot be deleted, and a VLAN that still has ports cannot be deleted
//! (the switch would refuse, but "VLAN 225 still has 4 ports" beats a
//! half-applied change and a confusing error).

use crate::model::{Port, Snapshot};
use crate::R;
use std::collections::BTreeSet;

/// Longest port name this platform accepts (the switch enforces its own limit
/// too; failing here gives a readable message instead of `Invalid input`).
const MAX_PORT_NAME: usize = 15;
const MAX_VLAN_NAME: usize = 64;

/// A physical port (`1`..`54`) or trunk group (`Trk1`..`Trk8`).
pub fn check_port(port: &str) -> R<()> {
    if let Some(n) = port.strip_prefix("Trk") {
        let n: u32 = n
            .parse()
            .map_err(|_| format!("`{port}` is not a trunk group (expected Trk1..Trk8)"))?;
        return if (1..=8).contains(&n) {
            Ok(())
        } else {
            Err(format!("trunk group {n} is out of range (Trk1..Trk8)"))
        };
    }
    let n: u32 = port
        .parse()
        .map_err(|_| format!("`{port}` is not a port number"))?;
    if (1..=54).contains(&n) {
        Ok(())
    } else {
        Err(format!("port {n} is out of range (1..54)"))
    }
}

/// A comma-separated list of ports and ranges, e.g. `5,7-9,Trk1`.
pub fn check_port_list(spec: &str) -> R<Vec<String>> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err("empty item in the port list".to_string());
        }
        match part.split_once('-') {
            Some((a, b)) => {
                check_port(a)?;
                check_port(b)?;
                out.push(format!("{a}-{b}"));
            }
            None => {
                check_port(part)?;
                out.push(part.to_string());
            }
        }
    }
    if out.is_empty() {
        return Err("no ports given".to_string());
    }
    Ok(out)
}

pub fn check_vlan(id: u32) -> R<()> {
    if (1..=4094).contains(&id) {
        Ok(())
    } else {
        Err(format!("VLAN {id} is out of range (1..4094)"))
    }
}

/// A label that is safe to put inside double quotes on the CLI.
///
/// The CLI has no escaping mechanism inside a quoted name, so the characters
/// that could end the argument or inject another command are refused outright
/// rather than escaped.
fn check_label(value: &str, max: usize, what: &str) -> R<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{what} must not be empty"));
    }
    if value.chars().count() > max {
        return Err(format!(
            "{what} is {} characters; the limit is {max}",
            value.chars().count()
        ));
    }
    if value.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return Err(format!(
            "{what} may not contain a quote, a backslash or a control character"
        ));
    }
    Ok(())
}

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.trim())
}

/// `port 5 name "NAS2 Port 1"`
/// `interface <port> <setting>` — the **one-shot** form.
///
/// This firmware has no flat `port <n> …` form for these settings: at config
/// level `port 5 name "x"` answers `Invalid input: name` (measured on 10.3.1.2).
/// The CLI's help says `interface` enters the interface level *or executes one
/// command for that level*, and the one-shot form is the one to use: it leaves
/// the session at `switch1(config)#`, which the prompt matcher knows.
///
/// Entering the level was tried and is not workable here — at
/// `switch1(eth-5)#` the CLI repaints its bottom row with enough ANSI that the
/// read never saw a buffer ending cleanly on a prompt and timed out at 20 s.
fn interface(port: &str, setting: &str) -> Vec<String> {
    vec![format!("interface {port} {setting}")]
}

/// `interface 5 name "NAS2 Port 1"`.
pub fn port_name(port: &str, name: &str) -> R<Vec<String>> {
    check_port(port)?;
    check_label(name, MAX_PORT_NAME, "Port name")?;
    Ok(interface(port, &format!("name {}", quoted(name))))
}

/// Reconcile one port's VLAN membership with what the form says it should be.
///
/// The form submits the *wanted* state, not the click that changed it, and the
/// commands come from the difference against the snapshot the page was built
/// from.  Two reasons it has to work that way:
///
/// * `vlan <id> <untagged|tagged> <ports>` is a statement about the **VLAN**, not
///   about the port, so "make port 7 an access port on 225" is three commands
///   against three different VLANs, and the switch has no verb for the thing the
///   operator actually asked for.
/// * A command that is already true is not free.  It is a config-mode round trip
///   on a console that takes a second per line, so ticking two boxes should cost
///   two commands, not one per VLAN the port belongs to.
///
/// Ordering is not cosmetic.  The rule is **take away, then move the native
/// VLAN, then add**:
///
/// * removals go first, because a port cannot be tagged and untagged on the same
///   VLAN at once — moving port 7 from `untagged 225, tagged 226` to
///   `untagged 226` has to `no vlan 226 tagged 7` before it sets the untagged
///   side, or the port is left in a state the switch will not report honestly.
/// * the untagged move is `no`-then-set: a port has exactly one native VLAN, and
///   setting the new one without releasing the old leaves the port showing under
///   both in `show vlan`.
/// * additions come last, so no later removal can undo one.
pub fn port_members(port: &Port, tagged: &[u32], untagged: Option<u32>) -> R<Vec<String>> {
    check_port(&port.id)?;
    // Two different things are "a trunk" here, and the model tells them apart:
    // a **member** carries the group in `trunk_group` (set from `show trunks`),
    // while the **group itself** is a row whose id is the token (`Trk1`).  Both
    // are refused.  The switch refuses to change a member's membership while it
    // is aggregated, and a trunk group has never been exercised at all — so the
    // GUI locks both rather than sending a command form nobody has seen work.
    if !port.trunk_group.is_empty() {
        return Err(format!(
            "port {} is in LACP group {}; this tool will not change the VLAN membership of a trunk member or of a trunk group",
            port.id,
            port.trunk_group
        ));
    }
    if crate::model::is_trunk_group(&port.id) {
        return Err(format!(
            "{} is a link-aggregation group, not a port; its VLAN membership is managed as a unit with its members, which this tool does not attempt",
            port.id
        ));
    }
    let mut want: BTreeSet<u32> = BTreeSet::new();
    for id in tagged {
        check_vlan(*id)?;
        want.insert(*id);
    }
    if let Some(u) = untagged {
        check_vlan(u)?;
        if want.remove(&u) {
            return Err(format!(
                "VLAN {u} cannot be both the untagged VLAN and a tagged VLAN of port {}",
                port.id
            ));
        }
    }

    let have: BTreeSet<u32> = port.tagged_vlans.iter().copied().collect();
    let mut cmds = Vec::new();
    for id in have.difference(&want) {
        cmds.push(format!("no vlan {id} tagged {}", port.id));
    }
    if let Some(new) = untagged {
        if port.untagged_vlan != Some(new) {
            if let Some(old) = port.untagged_vlan {
                cmds.push(format!("no vlan {old} untagged {}", port.id));
            }
            cmds.push(format!("vlan {new} untagged {}", port.id));
        }
    }
    for id in want.difference(&have) {
        cmds.push(format!("vlan {id} tagged {}", port.id));
    }
    Ok(cmds)
}

/// `interface 9 enable` / `interface 9 disable`.
pub fn port_state(port: &str, enabled: bool) -> R<Vec<String>> {
    check_port(port)?;
    let verb = if enabled { "enable" } else { "disable" };
    Ok(interface(port, verb))
}

/// Create or rename a VLAN — the same command does both on this platform.
pub fn vlan_rename(id: u32, name: &str) -> R<Vec<String>> {
    check_vlan(id)?;
    check_label(name, MAX_VLAN_NAME, "VLAN name")?;
    Ok(vec![format!("vlan {id} name {}", quoted(name))])
}

/// Delete a VLAN, unless it is the default VLAN or still has ports.
pub fn vlan_delete(id: u32, snap: &Snapshot) -> R<Vec<String>> {
    check_vlan(id)?;
    if id == 1 {
        return Err("VLAN 1 (DEFAULT) cannot be deleted".to_string());
    }
    let mut members: Vec<&str> = Vec::new();
    for p in snap.ports.values() {
        let in_it = p.untagged_vlan == Some(id) || p.tagged_vlans.contains(&id);
        if in_it {
            members.push(&p.id);
        }
    }
    if !members.is_empty() {
        return Err(format!(
            "VLAN {id} still has {} port(s): {} — move them off first",
            members.len(),
            members.join(", ")
        ));
    }
    Ok(vec![format!("no vlan {id}")])
}

/// Add or remove tagged membership.  `no vlan 225 tagged 7` is the inverse of
/// `vlan 225 tagged 7`.
pub fn vlan_tagged(id: u32, ports: &str, add: bool) -> R<Vec<String>> {
    check_vlan(id)?;
    let list = check_port_list(ports)?;
    let verb = if add { "" } else { "no " };
    Ok(vec![format!(
        "{verb}vlan {id} tagged {}",
        list.join(",")
    )])
}

/// A command typed into the console box.  Only `show` commands are accepted:
/// everything else should come through a validated action so the audit log
/// says what changed.
pub fn read_command(cmd: &str) -> R<String> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Err("no command given".to_string());
    }
    if cmd.contains('\n') || cmd.contains('\r') {
        return Err("one command at a time".to_string());
    }
    if cmd.len() > 200 {
        return Err("command is too long".to_string());
    }
    let first = cmd.split_whitespace().next().unwrap_or("");
    if first != "show" && first != "display" {
        return Err(
            "only `show` commands may be typed into the console; use the buttons for \
             configuration so it is audited"
                .to_string(),
        );
    }
    Ok(cmd.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Port, Vlan};

    fn snap_with(ports: &[(&str, Option<u32>, &[u32])]) -> Snapshot {
        let mut s = Snapshot::default();
        for (id, untagged, tagged) in ports {
            let mut p = Port::new(id);
            p.untagged_vlan = *untagged;
            p.tagged_vlans = tagged.to_vec();
            s.ports.insert(id.to_string(), p);
        }
        s.vlans.push(Vlan {
            id: 225,
            name: "Shared".into(),
            status: "Port-based".into(),
            jumbo: "No".into(),
        });
        s
    }

    #[test]
    fn port_commands() {
        assert_eq!(
            port_name("5", "NAS2 Port 1").unwrap(),
            vec![r#"interface 5 name "NAS2 Port 1""#.to_string()]
        );
        assert_eq!(
            port_state("9", false).unwrap(),
            vec!["interface 9 disable".to_string()]
        );
        assert_eq!(
            port_state("9", true).unwrap(),
            vec!["interface 9 enable".to_string()]
        );
    }

    #[test]
    fn vlan_commands() {
        assert_eq!(
            vlan_rename(225, "Shared").unwrap(),
            vec![r#"vlan 225 name "Shared""#.to_string()]
        );
        assert_eq!(
            vlan_tagged(225, "7,9-11", true).unwrap(),
            vec!["vlan 225 tagged 7,9-11".to_string()]
        );
        assert_eq!(
            vlan_tagged(225, "7", false).unwrap(),
            vec!["no vlan 225 tagged 7".to_string()]
        );
        assert_eq!(
            vlan_delete(225, &snap_with(&[])).unwrap(),
            vec!["no vlan 225".to_string()]
        );
    }

    #[test]
    fn the_default_vlan_is_protected() {
        assert!(vlan_delete(1, &snap_with(&[]))
            .unwrap_err()
            .contains("cannot be deleted"));
    }

    #[test]
    fn a_vlan_with_ports_cannot_be_deleted() {
        let s = snap_with(&[("7", Some(225), &[]), ("9", None, &[225])]);
        let err = vlan_delete(225, &s).unwrap_err();
        assert!(err.contains("still has 2 port(s)"), "{err}");
        assert!(err.contains("7") && err.contains("9"));
    }

    #[test]
    fn port_numbers_are_checked() {
        assert!(check_port("1").is_ok());
        assert!(check_port("48").is_ok());
        assert!(check_port("Trk8").is_ok());
        assert!(check_port("0").is_err());
        assert!(check_port("55").is_err());
        assert!(check_port("Trk9").is_err());
        assert!(check_port("Trk").is_err());
        assert!(check_port("5; show vlan").is_err());
        assert!(check_port("").is_err());
    }

    #[test]
    fn port_lists_accept_ranges() {
        assert_eq!(check_port_list("5,7-9").unwrap(), vec!["5", "7-9"]);
        assert!(check_port_list("5,").is_err());
        assert!(check_port_list("7-").is_err());
        assert!(check_port_list("").is_err());
    }

    /// A name is quoted, so anything that could close the quote or add a
    /// second command must be refused before it reaches the CLI.
    #[test]
    fn labels_cannot_inject() {
        assert!(port_name("5", r#"x" ; no vlan 1 ; "#).is_err());
        assert!(port_name("5", "back\\slash").is_err());
        assert!(port_name("5", "line\nbreak").is_err());
        assert!(port_name("5", "control\u{7}char").is_err());
        assert!(port_name("5", "ok name").is_ok());
        assert!(port_name("5", "").is_err());
        assert!(port_name("5", &"x".repeat(16)).is_err());
        // The platform's own limit, and the length of the names already on the
        // switch (`Switch2 Port 47`).
        assert!(port_name("5", &"x".repeat(15)).is_ok());
    }

    #[test]
    fn vlan_numbers_are_checked() {
        assert!(check_vlan(1).is_ok());
        assert!(check_vlan(4094).is_ok());
        assert!(check_vlan(0).is_err());
        assert!(check_vlan(4095).is_err());
    }

    fn port(untagged: Option<u32>, tagged: &[u32]) -> Port {
        let mut p = Port::new("7");
        p.untagged_vlan = untagged;
        p.tagged_vlans = tagged.to_vec();
        p
    }

    /// The form sends the wanted state, so the commands are the difference.
    /// Ticking two boxes must be two commands, not one per VLAN the port is in.
    #[test]
    fn membership_is_a_diff_not_a_recital() {
        // 7 is untagged 225, tagged 226 and 227.  Want: same untagged, drop
        // 226, add 230.  227 is untouched and must not appear at all.
        let c = port_members(&port(Some(225), &[226, 227]), &[227, 230], Some(225)).unwrap();
        assert_eq!(
            c,
            vec![
                "no vlan 226 tagged 7".to_string(),
                "vlan 230 tagged 7".to_string()
            ]
        );
    }

    #[test]
    fn nothing_changed_is_no_commands() {
        assert!(port_members(&port(Some(225), &[226]), &[226], Some(225))
            .unwrap()
            .is_empty());
        // `untagged: None` means "leave the native VLAN alone", not "clear it".
        assert!(port_members(&port(Some(225), &[226]), &[226], None)
            .unwrap()
            .is_empty());
    }

    /// A port has exactly one native VLAN.  Setting the new one without
    /// releasing the old leaves it showing under both in `show vlan`, which is
    /// worse than the extra command.
    #[test]
    fn moving_the_untagged_vlan_releases_the_old_one() {
        let c = port_members(&port(Some(225), &[]), &[], Some(226)).unwrap();
        assert_eq!(
            c,
            vec![
                "no vlan 225 untagged 7".to_string(),
                "vlan 226 untagged 7".to_string()
            ]
        );
        // A port with no native VLAN yet just joins.
        let c = port_members(&port(None, &[]), &[], Some(226)).unwrap();
        assert_eq!(c, vec!["vlan 226 untagged 7".to_string()]);
    }

    /// The ordering rule, and the case it exists for: 7 is untagged 225 and
    /// tagged 226, and is being made an access port on 226.  The tagged 226 has
    /// to go **before** the untagged side is set, or the port is briefly both on
    /// the same VLAN and `show vlan` reports whichever one it likes.
    #[test]
    fn a_tagged_membership_is_dropped_before_the_same_vlan_goes_untagged() {
        let c = port_members(&port(Some(225), &[226]), &[], Some(226)).unwrap();
        assert_eq!(
            c,
            vec![
                "no vlan 226 tagged 7".to_string(),
                "no vlan 225 untagged 7".to_string(),
                "vlan 226 untagged 7".to_string()
            ]
        );
    }

    #[test]
    fn a_port_cannot_be_untagged_and_tagged_on_the_same_vlan() {
        let err = port_members(&port(Some(225), &[]), &[226, 227], Some(226)).unwrap_err();
        assert!(err.contains("both the untagged VLAN and a tagged VLAN"), "{err}");
        // The wanted state is what is checked: asking to *keep* 226 tagged while
        // moving the native VLAN off it is fine, that is an ordinary trunk.
        assert!(port_members(&port(Some(225), &[226]), &[226], Some(227))
            .is_ok());
    }

    /// A trunk member is the one thing the switch refuses here, and a trunk
    /// group has never been exercised: neither is attempted.
    #[test]
    fn trunk_members_are_left_alone() {
        let mut p = port(Some(225), &[]);
        p.trunk_group = "Trk3".into();
        p.trunk_type = "LACP".into();
        let err = port_members(&p, &[226], None).unwrap_err();
        assert!(err.contains("LACP group Trk3"), "{err}");
    }

    #[test]
    fn membership_validates_its_numbers() {
        assert!(port_members(&port(Some(225), &[]), &[0], None).is_err());
        assert!(port_members(&port(Some(225), &[]), &[4095], None).is_err());
        assert!(port_members(&port(Some(225), &[]), &[], Some(0)).is_err());
        let mut bad = port(Some(225), &[]);
        bad.id = "7; show vlan".into();
        assert!(port_members(&bad, &[226], None).is_err());
    }

    /// A trunk **group** is a port-shaped row in the model, and the CLI will
    /// syntactically accept `vlan 225 tagged Trk1` — but that form has never been
    /// exercised, so it is refused rather than sent.  (Checking `trunk_group`
    /// alone does not catch this: that field is set on a group's *members*, not
    /// on the group's own row.)
    #[test]
    fn a_trunk_group_is_refused_too() {
        let mut p = port(Some(225), &[]);
        p.id = "Trk1".into();
        let err = port_members(&p, &[226], None).unwrap_err();
        assert!(err.contains("link-aggregation group"), "{err}");
        // And it is the group token, not the number, that makes it one.
        let mut n = port(Some(225), &[]);
        n.id = "1".into();
        assert!(port_members(&n, &[226], None).is_ok());
    }

    #[test]
    fn the_console_only_runs_show() {
        assert_eq!(read_command("  show vlan  ").unwrap(), "show vlan");
        assert!(read_command("no vlan 1").is_err());
        assert!(read_command("show vlan\nconfigure").is_err());
        assert!(read_command("").is_err());
        assert!(read_command(&format!("show {}", "x".repeat(300))).is_err());
    }
}
