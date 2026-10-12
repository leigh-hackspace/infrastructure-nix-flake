//! Turning a GUI action into ProCurve CLI lines, and refusing the ones that
//! should not be sent.
//!
//! The commands follow the older ProCurve CLI this firmware speaks.  They run
//! inside **config mode** (`Switch::apply` enters it first) — at the plain
//! Manager Exec prompt every one of them answers `Invalid input`.  **They are
//! written from the platform's command reference, not from an executed
//! session**, so the first real use is the test; anything the switch rejects
//! comes back as its own `Invalid input: …` text, surfaced verbatim, and the
//! change is never saved to NVRAM, so a reboot undoes it.
//!
//! Two guard rails are enforced here rather than trusted to the browser:
//! VLAN 1 cannot be deleted, and a VLAN that still has ports cannot be deleted
//! (the switch would refuse, but "VLAN 225 still has 4 ports" beats a
//! half-applied change and a confusing error).

use crate::model::Snapshot;
use crate::R;

/// Longest port name this platform accepts (the switch enforces its own limit
/// too; failing here gives a readable message instead of `Invalid input`).
const MAX_PORT_NAME: usize = 15;
const MAX_DESCRIPTION: usize = 70;
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
pub fn port_name(port: &str, name: &str) -> R<Vec<String>> {
    check_port(port)?;
    check_label(name, MAX_PORT_NAME, "Port name")?;
    Ok(vec![format!("port {port} name {}", quoted(name))])
}

/// `port 5 description "uplink to services1"`
pub fn port_description(port: &str, text: &str) -> R<Vec<String>> {
    check_port(port)?;
    check_label(text, MAX_DESCRIPTION, "Description")?;
    Ok(vec![format!(
        "port {port} description {}",
        quoted(text)
    )])
}

/// Move an access port to another VLAN: sets the PVID and makes the port
/// untagged in that VLAN.
pub fn port_vlan(port: &str, vlan: u32) -> R<Vec<String>> {
    check_port(port)?;
    check_vlan(vlan)?;
    Ok(vec![format!("port {port} vlan {vlan}")])
}

/// `port 5 enabled` / `port 5 disabled`.
pub fn port_state(port: &str, enabled: bool) -> R<Vec<String>> {
    check_port(port)?;
    let state = if enabled { "enabled" } else { "disabled" };
    Ok(vec![format!("port {port} {state}")])
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
            vec![r#"port 5 name "NAS2 Port 1""#.to_string()]
        );
        assert_eq!(
            port_vlan("Trk1", 225).unwrap(),
            vec!["port Trk1 vlan 225".to_string()]
        );
        assert_eq!(
            port_state("9", false).unwrap(),
            vec!["port 9 disabled".to_string()]
        );
        assert_eq!(
            port_description("1", "uplink to services1").unwrap(),
            vec![r#"port 1 description "uplink to services1""#.to_string()]
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
        assert!(port_description("5", &"x".repeat(70)).is_ok());
        assert!(port_description("5", &"x".repeat(71)).is_err());
    }

    #[test]
    fn vlan_numbers_are_checked() {
        assert!(check_vlan(1).is_ok());
        assert!(check_vlan(4094).is_ok());
        assert!(check_vlan(0).is_err());
        assert!(check_vlan(4095).is_err());
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
