//! Turning what the operator asked for into what the switch will be told.
//!
//! This is the whole write path except the Telnet bytes themselves, and it runs
//! **before** anything is queued: an intent is validated against the shadow copy,
//! diffed against it, and either refused with a message the operator can act on
//! or turned into a [`Planned`] — the CLI lines, the ports they touch, the state
//! they replace, and one line of prose for the audit log.  Only then does
//! [`crate::changes::queue`] insert it, and only then does the request return.
//!
//! Three properties this file is responsible for:
//!
//! * **Nothing is queued that the switch is known to refuse.**  A trunk member's
//!   VLAN membership, a VLAN still carrying ports, a name with a quote in it, a
//!   VLAN id outside 1..4094 — all refused here, in the request's own time, so
//!   the operator sees the reason immediately instead of after five retries in a
//!   log they were not watching.  The checks are the same ones the pre-database
//!   GUI made inline, moved here so there is one copy.
//! * **A membership submit is a wanted state, not a delta.**  The editor posts
//!   the whole row it is showing; [`commands::port_members`] diffs it against the
//!   shadow copy.  That is what makes a double-submit harmless (the second one
//!   diffs to nothing) and what stops two operators' clicks from compounding.
//! * **The before-state is captured at queue time.**  Revert and the audit log
//!   both read it, and by the time either is used the switch may have moved again
//!   — "undo" has to mean the state *this* change replaced.
//!
//! The CLI lines themselves come from [`crate::commands`], which owns the
//! firmware's quoting rules and its ordering constraints (a port cannot be
//! tagged and untagged on the same VLAN in one go, so the removals, then the
//! native move, then the additions).

use crate::commands;
use crate::dto::Intent;
use crate::model::{Port, Snapshot};
use crate::shadow;
use crate::R;
use std::collections::{BTreeMap, BTreeSet};

/// A change that has been checked and turned into CLI, but not yet queued.
///
/// The audit row is written from this, so every field here is either stored or
/// used to decide what to store — with one deliberate exception, [`Planned::vlan`],
/// which is written to neither and exists to keep a VLAN id from being re-derived
/// from the summary text.
#[derive(Debug, Clone)]
pub struct Planned {
    /// The audit log's `kind` column.
    pub kind: &'static str,
    /// Ports to mark in the table while this is pending.  Empty for a VLAN-only
    /// change, which is marked by the VLAN instead.
    pub ports: Vec<String>,
    /// One line for a human, naming the ports and the VLANs involved.
    pub summary: String,
    /// The plans, one entry per port, each applied in config mode in order (see
    /// [`crate::switch::Switch::apply_groups`]).
    pub commands: Vec<Vec<String>>,
    /// What the shadow copy said first (see [`crate::dto::Change::before`]).
    pub before: BTreeMap<String, String>,
    /// The VLAN this change is about, when it has one.  Not stored separately —
    /// it is in `summary` and in the commands — but the caller uses it to decide
    /// which rows to mark, and a test asserts it.
    pub vlan: Option<u32>,
    /// The operator's own words, from [`Intent::note`], already trimmed to
    /// `None` when they typed nothing.  Stored on the audit row and never sent to
    /// the console: the CLI has no comment syntax, and a note that reached a
    /// switch would be a write nobody planned.
    pub note: Option<String>,
}

impl Planned {
    /// A change that would do nothing.  Not an error: the membership editor sends
    /// the whole wanted row, so "already like that" is an ordinary answer, and
    /// queueing it would put a no-op write in front of a real one.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() || self.commands.iter().all(|g| g.is_empty())
    }

    /// The ports this change marks, spelled the way a patch panel is read:
    /// `7 · GW Port 1`, or just `7` for a port nobody named.
    ///
    /// The queue page has the port names in its own rows and joins them there; the
    /// places that have only a [`Planned`] — the journal line, and anything written
    /// before the row exists — need the same reading, and a second format for the
    /// same port is how a search for "GW Port 1" starts missing lines.
    pub fn labelled_ports(&self, snap: &Snapshot) -> Vec<String> {
        self.ports
            .iter()
            .map(|id| match snap.ports.get(id) {
                Some(p) => p.label(),
                None => id.clone(),
            })
            .collect()
    }
}

/// Validate an intent against the shadow copy and plan it.
///
/// The `Err` of every arm is written to be shown to the operator verbatim, which
/// is a constraint on the wording: it names the thing they can see on screen
/// ("port 7", "VLAN 20") and says what to do instead, because there is no second
/// place this message can be made clearer.
pub async fn plan(pool: &sqlx::PgPool, intent: &Intent) -> R<Planned> {
    let name = intent.switch_name.as_str();
    // Every kind needs a switch the sync knows about.  Checking it once here is
    // what makes the per-kind arms below able to assume it.
    if shadow::page(pool, name).await?.is_none() {
        return Err(format!(
            "no such switch: {name} (the switch list comes from common/hosts.nix)"
        ));
    }

    match intent.kind.as_str() {
        "port-name" => port_name(pool, intent).await,
        "port-state" => port_state(pool, intent).await,
        "port-members" => port_members(pool, intent).await,
        "vlan-name" => vlan_name(pool, intent).await,
        "vlan-members" => vlan_members(pool, intent).await,
        "vlan-delete" => vlan_delete(pool, intent).await,
        "refresh" => Ok(refresh(name)),
        other => Err(format!(
            "unknown change kind `{other}` (expected port-name, port-state, \
             port-members, vlan-name, vlan-members, vlan-delete or refresh)"
        )),
    }
}

/// A forced re-read, queued rather than performed.
///
/// The page's Refresh button used to block on the console; now it queues a
/// `refresh`, which the sync treats as "read this switch now".  The row is the
/// audit entry for the click, and the pending notice is what tells the operator
/// the numbers on screen are about to change.
fn refresh(name: &str) -> Planned {
    Planned {
        kind: "refresh",
        ports: Vec::new(),
        summary: format!("re-read {name} now"),
        // A change with no CLI lines: the sync recognises the kind and reads
        // instead of writing.  Keeping it in the same table is what makes the
        // audit log complete — every click that touched a switch is in it.
        commands: Vec::new(),
        before: BTreeMap::new(),
        vlan: None,
        note: None,
    }
}

async fn port_name(pool: &sqlx::PgPool, intent: &Intent) -> R<Planned> {
    let port = current(pool, intent).await?;
    let wanted = intent
        .name
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or("a new name is required")?;
    let cmds = commands::port_name(&port.id, wanted)?;
    let mut before = BTreeMap::new();
    before.insert("name".into(), port.name.clone());

    if port.name == wanted {
        // Refusing a no-op rather than queueing it keeps the audit log free of
        // rows that changed nothing, which matters because the log is also the
        // thing an operator scans to work out what happened today.
        return Ok(Planned {
            kind: "port-name",
            ports: vec![port.id.clone()],
            summary: format!("Port {}: already named `{wanted}`", port.id),
            commands: Vec::new(),
            before,
            vlan: None,
            note: None,
        });
    }

    Ok(Planned {
        kind: "port-name",
        ports: vec![port.id.clone()],
        summary: format!("Port {}: name `{}` → `{}`", port.id, port.name, wanted),
        commands: vec![cmds], // The operator's own words on the request, if they typed any: the audit row is
        // the only place a note like this survives, and it never reaches the CLI.
        before,
        vlan: None,
        note: note_of(intent),
    })
}

async fn port_state(pool: &sqlx::PgPool, intent: &Intent) -> R<Planned> {
    let port = current(pool, intent).await?;
    let wanted = intent
        .enabled
        .ok_or("expected enabled=true or enabled=false")?;
    let cmds = commands::port_state(&port.id, wanted)?;
    let mut before = BTreeMap::new();
    before.insert("enabled".into(), port.enabled.to_string());

    if port.enabled == wanted {
        return Ok(Planned {
            kind: "port-state",
            ports: vec![port.id.clone()],
            summary: format!(
                "Port {}: already {}",
                port.id,
                if wanted { "up" } else { "shut" }
            ),
            commands: Vec::new(),
            before,
            vlan: None,
            note: None,
        });
    }

    Ok(Planned {
        kind: "port-state",
        ports: vec![port.id.clone()],
        summary: format!(
            "Port {}: {} → {}",
            port.id,
            if port.enabled { "up" } else { "shut" },
            if wanted { "up" } else { "shut" }
        ),
        commands: vec![cmds], // The operator's own words on the request, if they typed any: the audit row is
        // the only place a note like this survives, and it never reaches the CLI.
        before,
        vlan: None,
        note: note_of(intent),
    })
}

async fn port_members(pool: &sqlx::PgPool, intent: &Intent) -> R<Planned> {
    let port = current(pool, intent).await?;

    // The wanted tagged set.  Absent means "leave it alone"; an empty list means
    // "no tagged VLANs", and the two are different questions (see
    // `dto::Intent::tagged`).
    let wanted_tagged: Option<BTreeSet<u32>> = intent
        .tagged
        .as_ref()
        .map(|list| list.iter().map(|p| p.trim()).filter(|p| !p.is_empty()))
        .map(|iter| iter.map(|p| p.parse::<u32>()).collect::<Result<_, _>>())
        .transpose()
        .map_err(|_| "tagged must be a list of VLAN numbers".to_string())?;

    // The wanted native VLAN.  Empty means "leave it alone" — the switch has no
    // way to express "no native VLAN", so there is nothing else it could mean.
    let wanted_native: Option<u32> = match intent.untagged.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(v) => Some(
            v.parse::<u32>()
                .map_err(|_| format!("untagged VLAN `{v}` is not a number"))?,
        ),
    };

    // What the form did not send is what the port already has: the diff needs a
    // complete wanted row, and half a row would delete the half not mentioned.
    // `commands::port_members` takes the wanted tagged set as VLAN numbers: the
    // form sends strings because that is what a form field is.
    let tagged: Vec<u32> = match &wanted_tagged {
        Some(set) => set.iter().copied().collect(),
        None => port.tagged_vlans.clone(),
    };
    let native = wanted_native.or(port.untagged_vlan);

    let cmds = commands::port_members(&port, &tagged, native)?;
    let mut before = BTreeMap::new();
    before.insert(
        "native".into(),
        port.untagged_vlan
            .map(|v| v.to_string())
            .unwrap_or_default(),
    );
    before.insert(
        "tagged".into(),
        port.tagged_vlans
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );

    Ok(Planned {
        kind: "port-members",
        ports: vec![port.id.clone()],
        summary: format!(
            "Port {}: untagged {}, tagged {} → untagged {}, tagged {}",
            port.id,
            port.untagged_vlan
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into()),
            port.tagged_vlans
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("+"),
            native.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            tagged
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("+"),
        ),
        commands: vec![cmds], // The operator's own words on the request, if they typed any: the audit row is
        // the only place a note like this survives, and it never reaches the CLI.
        before,
        vlan: None,
        note: note_of(intent),
    })
}

async fn vlan_name(pool: &sqlx::PgPool, intent: &Intent) -> R<Planned> {
    let id = vlan_id(intent)?;
    let name = intent
        .name
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or("a new VLAN name is required")?;
    let existing = shadow::vlan(pool, &intent.switch_name, id)
        .await?
        .ok_or_else(|| format!("VLAN {id} is not on this switch — create it on the CLI first"))?;
    let cmds = commands::vlan_rename(id, name)?;
    let mut before = BTreeMap::new();
    before.insert("vlan-name".into(), existing.name.clone());

    if existing.name == name {
        return Ok(Planned {
            kind: "vlan-name",
            ports: Vec::new(),
            summary: format!("VLAN {id}: already named `{name}`"),
            commands: Vec::new(),
            before,
            vlan: Some(id),
            note: None,
        });
    }

    Ok(Planned {
        kind: "vlan-name",
        ports: Vec::new(),
        summary: format!("VLAN {id}: `{}` → `{name}`", existing.name),
        commands: vec![cmds], // The operator's own words on the request, if they typed any: the audit row is
        // the only place a note like this survives, and it never reaches the CLI.
        before,
        vlan: Some(id),
        note: note_of(intent),
    })
}

/// Tag or untag a list of ports on one VLAN — the VLAN page's editor, which works
/// from the VLAN rather than from a port.
async fn vlan_members(pool: &sqlx::PgPool, intent: &Intent) -> R<Planned> {
    let id = vlan_id(intent)?;
    let ports = intent
        .port
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or("a port or port list is required (e.g. `7,9,22-24`)")?;
    let add = intent.enabled.unwrap_or(true);
    // `check_port_list` expands `22-24` and rejects anything the CLI would not
    // take, so the plan below is built from real port ids.
    let list = commands::check_port_list(ports)?;
    let cmds = commands::vlan_tagged(id, ports, add)?;

    // The before-state is each listed port's tagged set: the revert of "add these
    // to VLAN 20" is "remove them from VLAN 20", which needs no per-port state,
    // but the audit log is better with what was true at the time.
    let mut before = BTreeMap::new();
    for p in &list {
        if let Some(port) = shadow::port(pool, &intent.switch_name, p).await? {
            before.insert(
                format!("tagged:{p}"),
                port.tagged_vlans
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
    }

    Ok(Planned {
        kind: "vlan-members",
        ports: list,
        summary: format!(
            "VLAN {id}: {} ports {}",
            if add { "tag" } else { "untag" },
            intent.port.as_deref().unwrap_or("").trim()
        ),
        commands: vec![cmds], // The operator's own words on the request, if they typed any: the audit row is
        // the only place a note like this survives, and it never reaches the CLI.
        before,
        vlan: Some(id),
        note: note_of(intent),
    })
}

async fn vlan_delete(pool: &sqlx::PgPool, intent: &Intent) -> R<Planned> {
    let id = vlan_id(intent)?;
    let snap = shadow::snapshot(pool, &intent.switch_name)
        .await?
        .ok_or("the switch has no shadow copy yet")?;
    // The refusal lives in `commands::vlan_delete` (it needs the whole snapshot
    // to know if anything is still on the VLAN), so the message the operator sees
    // is the one the tool has always given.
    let cmds = commands::vlan_delete(id, &snap)?;

    let mut before = BTreeMap::new();
    if let Some(v) = shadow::vlan(pool, &intent.switch_name, id).await? {
        before.insert("vlan-name".into(), v.name);
    }
    // The members, so a Revert can rebuild the VLAN rather than just re-creating
    // an empty one: a delete that cannot be undone is a delete you have to be
    // careful with, and the point of a GUI is that you do not have to be.
    let mut untagged = Vec::new();
    let mut tagged = Vec::new();
    for (pid, port) in &snap.ports {
        if port.untagged_vlan == Some(id) {
            untagged.push(pid.clone());
        }
        if port.tagged_vlans.contains(&id) {
            tagged.push(pid.clone());
        }
    }
    before.insert("untagged-members".into(), untagged.join(","));
    before.insert("tagged-members".into(), tagged.join(","));

    Ok(Planned {
        kind: "vlan-delete",
        ports: Vec::new(),
        summary: format!(
            "VLAN {id}: delete ({})",
            before
                .get("vlan-name")
                .cloned()
                .unwrap_or_else(|| "unnamed".into())
        ),
        commands: vec![cmds], // The operator's own words on the request, if they typed any: the audit row is
        // the only place a note like this survives, and it never reaches the CLI.
        before,
        vlan: Some(id),
        note: note_of(intent),
    })
}

// ---------------------------------------------------------------- helpers

/// The port the intent names, as the shadow copy has it.
///
/// The error is the important half.  "No such port" and "the sync has not read
/// this switch yet" need different answers from the operator, and the second one
/// must not read like a typo — it is a race with the sync, and reloading fixes it.
async fn current(pool: &sqlx::PgPool, intent: &Intent) -> R<Port> {
    let id = intent
        .port
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or("which port? (the request named none)")?
        .trim()
        .to_string();
    commands::check_port(&id)?;
    shadow::port(pool, &intent.switch_name, &id)
        .await?
        .ok_or_else(|| {
            format!(
                "port {id} is not in this switch's shadow copy — either it is not a \
                 port of this switch or the sync has not read it yet; reload the page"
            )
        })
}

/// The VLAN the intent names.
fn vlan_id(intent: &Intent) -> R<u32> {
    let id = intent.vlan.ok_or("which VLAN? (the request named none)")?;
    let id = u32::try_from(id).map_err(|_| format!("VLAN id {id} is out of range"))?;
    commands::check_vlan(id)?;
    Ok(id)
}

/// The revert of a change that has already been through the queue.
///
/// Built from the stored `before` map, never from the current shadow copy: by now
/// the switch may have been changed again by someone else, and "undo" that reads
/// the current state would undo *their* change too.  Where `before` does not hold
/// enough to reverse something, this says so rather than guessing.
/// Turn a stored change back into a plan, using the pool for the one thing the
/// row cannot say on its own.
///
/// [`revert_plan`] is pure and handles every kind whose `before` map records its
/// own undo. `vlan-delete` is the exception: its map stores the members as two
/// comma-joined strings, which is enough to re-issue the CLI lines but not enough
/// to *describe* the undo — and a revert whose summary says "its members" where
/// the original change named them is an audit log that gets vaguer with age. So
/// this reads the VLAN back and hands the row to the pure path unchanged.
pub async fn revert(
    pool: &sqlx::PgPool,
    change: &crate::dto::Change,
) -> R<Planned> {
    if change.kind != "vlan-delete" {
        return revert_plan(change);
    }
    let id = change_vlan_id(change)?;
    let (untagged, tagged) = shadow::vlan_membership(pool, &change.switch_name, id).await?;
    let mut change = change.clone();
    let join = |v: &Vec<String>| v.join(",");
    change
        .before
        .insert("untagged-members".into(), join(&untagged));
    change.before.insert("tagged-members".into(), join(&tagged));
    revert_plan(&change)
}

/// Turn a stored change back into a plan: the commands, the summary and the
/// `before` map for the new row.
pub fn revert_plan(change: &crate::dto::Change) -> R<Planned> {
    let b = &change.before;
    let port = change
        .ports
        .first()
        .cloned()
        .unwrap_or_default();
    let get = |k: &str| b.get(k).cloned().unwrap_or_default();

    let (kind, summary, commands, before, vlan) = match change.kind.as_str() {
        "port-name" => (
            "port-name",
            format!("Port {}: revert name to `{}`", port, get("name")),
            vec![commands::port_name(&port, &get("name"))?],
            BTreeMap::new(),
            None,
        ),
        "port-state" => {
            let wanted = get("enabled") != "false";
            (
                "port-state",
                format!(
                    "Port {}: revert to {}",
                    port,
                    if wanted { "up" } else { "shut" }
                ),
                vec![commands::port_state(&port, wanted)?],
                BTreeMap::new(),
                None,
            )
        }
        "port-members" => {
            let native = get("native");
            let tagged = get("tagged");
            (
                "port-members",
                format!(
                    "Port {}: revert to untagged {}, tagged {}",
                    port,
                    if native.is_empty() {
                        "—"
                    } else {
                        native.as_str()
                    },
                    if tagged.is_empty() {
                        "—".to_string()
                    } else {
                        tagged.replace(',', "+")
                    }
                ),
                vec![commands::port_members(
                    // The diff needs *a* port to compare against; the shadow
                    // copy's current row is right for that (what is on the switch
                    // now), and the wanted values come from `before`.  Only the
                    // trunk-group check reads the port, and a port that has since
                    // been added to a trunk is exactly the case to refuse on.
                    &crate::model::Port::new(&port),
                    &tagged
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| s.trim().parse::<u32>().ok())
                        .collect::<Vec<u32>>(),
                    native.parse::<u32>().ok(),
                )?],
                BTreeMap::new(),
                None,
            )
        }
        "vlan-name" => {
            let id = change_vlan_id(change)?;
            (
                "vlan-name",
                format!("VLAN {id}: revert name to `{}`", get("vlan-name")),
                vec![commands::vlan_rename(id, &get("vlan-name"))?],
                BTreeMap::new(),
                Some(id),
            )
        }
        "vlan-delete" => {
            let id = change_vlan_id(change)?;
            let name = get("vlan-name");
            // Re-create the VLAN and put its members back.  `vlan <id> name "x"`
            // creates it on this firmware; the members are then re-added one
            // command per port, which is how the CLI takes them anyway.
            let mut cmds = vec![format!("vlan {id} name \"{}\"", name.replace('"', ""))];
            for p in get("untagged-members").split(',').filter(|s| !s.is_empty()) {
                cmds.push(format!("vlan {id} untagged {p}"));
            }
            for p in get("tagged-members").split(',').filter(|s| !s.is_empty()) {
                cmds.push(format!("vlan {id} tagged {p}"));
            }
            // Named with the same words the membership editor uses, because the
            // two rows are read side by side in the log: "delete (Office wifi)"
            // above "re-create as `Office wifi` with 3 untagged, 1 tagged" is the
            // pair that says whether everyone came back.
            let count = |v: &str| v.split(',').filter(|s| !s.is_empty()).count();
            let n_untagged = count(&get("untagged-members"));
            let n_tagged = count(&get("tagged-members"));
            (
                "vlan-name",
                format!(
                    "VLAN {id}: re-create as `{name}` with {n_untagged} untagged, \
                     {n_tagged} tagged"
                ),
                vec![cmds],
                BTreeMap::new(),
                Some(id),
            )
        }
        other => {
            return Err(format!(
                "a `{other}` change cannot be reverted ({}). The switch may have \
                 been changed by this one — check the port against the shadow copy \
                 after the next sync.",
                match other {
                    "vlan-members" => "the ports it lists may have been edited since",
                    "refresh" => "a re-read changed nothing to undo",
                    _ => "nothing was recorded to revert to",
                }
            ))
        }
    };

    Ok(Planned {
        kind,
        ports: change.ports.clone(),
        summary,
        commands,
        before,
        vlan,
        // A revert is generated from the stored `before` map; there is no request
        // behind it to have a note.  What it reverts is in `reverted_from`.
        note: None,
    })
}

/// The VLAN a stored change is about, taken out of its own commands.
///
/// There is no `vlan` column: the id is in the CLI lines and in the summary, and
/// re-deriving it here rather than storing it again is the difference between one
/// source of truth and two that can disagree.  A change whose commands do not
/// name a VLAN cannot be reverted, which is what this returns an error for.
fn change_vlan_id(change: &crate::dto::Change) -> R<u32> {
    for group in &change.commands {
        for cmd in group {
            // `vlan 225 name "x"`, `no vlan 225`, `vlan 225 tagged 7`.
            let mut words = cmd.split_whitespace();
            let first = words.next();
            // `no vlan 225` is the delete form and names its VLAN as plainly as
            // `vlan 225 tagged 7` does, so the `no` is dropped and the word after
            // it is the subject either way.
            let first = if first == Some("no") {
                words.next()
            } else {
                first
            };
            if first == Some("vlan") {
                if let Some(id) = words.next().and_then(|v| v.parse::<u32>().ok()) {
                    return Ok(id);
                }
            }
        }
    }
    Err("this change does not name a VLAN, so it cannot be reverted".to_string())
}

/// The operator's note, trimmed: an all-whitespace note is no note.
///
/// `None` rather than `Some("")` so the audit row stores a NULL and the page can
/// tell "nobody said anything" from "they said nothing", which is the difference
/// between an empty field and a deliberate one.
fn note_of(intent: &Intent) -> Option<String> {
    intent
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

/// Whether a planned change may be queued at all.
///
/// The one rule that cannot be checked while building the plan, because it needs
/// the plan and the queue together: a change that would write nothing must not
/// take a place in front of one that would.
pub fn check_queueable(planned: &Planned) -> R<()> {
    if planned.is_empty() && planned.kind != "refresh" {
        return Err("nothing to do: the switch is already in that state".to_string());
    }
    // A VLAN change that names no VLAN is a planner bug, not an empty form.  It
    // would reach the queue as a row nothing can revert: `revert_plan` reads the id
    // back out of the stored commands, and a `vlan-members` change has no commands
    // that name one.
    if planned.kind.starts_with("vlan-") && planned.vlan.is_none() {
        return Err(format!(
            "internal: a `{}` change was planned without a VLAN",
            planned.kind
        ));
    }
    Ok(())
}

/// The audit line for one queued change: what it says it will do, plus the
/// operator's note and the snapshot it was diffed against.
///
/// Built here rather than at the call site because the log line and the row must
/// not be able to disagree: both come from the same [`Planned`], and a note that
/// only ever appeared in the journal would be a claim nothing could check later.
pub fn log_line(planned: &Planned, snap: &Snapshot, user: &str) -> String {
    let note = planned
        .note
        .as_deref()
        .map(|n| format!(" — {n}"))
        .unwrap_or_default();
    // The ports by their labels rather than the ids in `planned.ports`: a journal
    // read with grep is searched for "GW Port 1", and a line that only has `7` is
    // found by that search only by accident.
    let ports = planned.labelled_ports(snap).join(", ");
    format!(
        "queued {} for {}[{}]: {}{note} ({}) by {user}",
        planned.kind,
        snap.switch,
        ports,
        planned.summary,
        snap_note(snap),
    )
}

/// The snapshot the plan was diffed against, for [`log_line`].
fn snap_note(snap: &Snapshot) -> String {
    format!("shadow copy of {} ports", snap.ports.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(id: &str) -> Port {
        let mut p = Port::new(id);
        p.name = format!("Port {id}");
        p.untagged_vlan = Some(1);
        p.tagged_vlans = vec![7];
        p.enabled = true;
        p
    }

    /// A revert is built from the row, so it needs no database — and the four
    /// revertible kinds must produce the commands that put the old state back.
    #[test]
    fn a_revert_rebuilds_the_state_it_replaced() {
        let mut before = BTreeMap::new();
        before.insert("name".into(), "GW Port 1".into());
        let c = crate::dto::Change {
            kind: "port-name".into(),
            ports: vec!["7".into()],
            before,
            ..Default::default()
        };
        let p = revert_plan(&c).unwrap();
        assert_eq!(p.kind, "port-name");
        assert_eq!(
            p.commands,
            vec![vec!["interface 7 name \"GW Port 1\"".to_string()]]
        );
        assert!(p.summary.contains("GW Port 1"), "{}", p.summary);

        let mut before = BTreeMap::new();
        before.insert("enabled".into(), "true".into());
        let c = crate::dto::Change {
            kind: "port-state".into(),
            ports: vec!["9".into()],
            before,
            ..Default::default()
        };
        let p = revert_plan(&c).unwrap();
        assert_eq!(
            p.commands,
            vec![vec!["interface 9 enable".to_string()]]
        );

        let mut before = BTreeMap::new();
        before.insert("native".into(), "9".into());
        before.insert("tagged".into(), "7,20".into());
        let c = crate::dto::Change {
            kind: "port-members".into(),
            ports: vec!["7".into()],
            before,
            ..Default::default()
        };
        let p = revert_plan(&c).unwrap();
        let joined = p.commands.concat().join(" ");
        assert!(joined.contains("vlan 9 untagged 7"), "{joined}");
        assert!(joined.contains("vlan 7 tagged 7"), "{joined}");
        assert!(joined.contains("vlan 20 tagged 7"), "{joined}");
    }

    /// A deleted VLAN comes back with the members it had, because `before` holds
    /// them.  An empty re-create would be a silent second data loss.
    #[test]
    fn a_reverted_delete_replaces_the_members() {
        let mut before = BTreeMap::new();
        before.insert("vlan-name".into(), "Cameras".into());
        before.insert("untagged-members".into(), "5,6".into());
        before.insert("tagged-members".into(), "47".into());
        let c = crate::dto::Change {
            kind: "vlan-delete".into(),
            commands: vec![vec!["no vlan 225".into()]],
            before,
            ..Default::default()
        };
        let p = revert_plan(&c).unwrap();
        let joined = p.commands.concat().join(" ");
        assert!(joined.starts_with("vlan 225 name \"Cameras\""), "{joined}");
        assert!(joined.contains("vlan 225 untagged 5"), "{joined}");
        assert!(joined.contains("vlan 225 untagged 6"), "{joined}");
        assert!(joined.contains("vlan 225 tagged 47"), "{joined}");
        assert_eq!(p.vlan, Some(225));
    }

    /// The VLAN id is read back out of the stored commands, including the `no
    /// vlan <id>` form a delete writes.
    #[test]
    fn the_vlan_is_found_in_the_stored_commands() {
        let c = crate::dto::Change {
            commands: vec![vec!["no vlan 225".into()]],
            ..Default::default()
        };
        assert_eq!(change_vlan_id(&c).unwrap(), 225);
        let c = crate::dto::Change {
            commands: vec![vec!["interface 7".into(), "shutdown".into()]],
            ..Default::default()
        };
        assert!(change_vlan_id(&c).is_err());
    }

    /// Kinds with no honest undo say so.  Guessing would be worse: the operator
    /// would read "reverted" and believe it.
    #[test]
    fn kinds_with_no_undo_are_refused_by_name() {
        for kind in ["vlan-members", "refresh", "nonsense"] {
            let c = crate::dto::Change {
                kind: kind.into(),
                ..Default::default()
            };
            let e = revert_plan(&c).unwrap_err();
            assert!(e.contains("cannot be reverted"), "{kind}: {e}");
        }
    }

    /// Every VLAN change the planner produces names its VLAN, and nothing else is
    /// refused for not naming one.
    #[test]
    fn a_vlan_change_must_name_its_vlan() {
        let mut p = Planned {
            kind: "vlan-name",
            ports: vec![],
            summary: "x".into(),
            commands: vec![vec!["vlan 7 name \"x\"".into()]],
            before: BTreeMap::new(),
            vlan: None,
            note: None,
        };
        assert!(check_queueable(&p).is_err());
        p.vlan = Some(7);
        assert!(check_queueable(&p).is_ok());
        // A port change names a port, not a VLAN, and must not be refused for it.
        p.kind = "port-name";
        p.ports = vec!["7".into()];
        p.vlan = None;
        assert!(check_queueable(&p).is_ok());
    }

    /// A no-op must not be queueable: it would take a place in the queue in front
    /// of a real change and put a row in the audit log that changed nothing.
    #[test]
    fn an_empty_plan_is_not_queueable() {
        let mut p = Planned {
            kind: "port-name",
            ports: vec!["7".into()],
            summary: "x".into(),
            commands: Vec::new(),
            before: BTreeMap::new(),
            vlan: None,
            note: None,
        };
        assert!(check_queueable(&p).is_err());
        // A refresh has no commands by design and must still be queueable.
        p.kind = "refresh";
        assert!(check_queueable(&p).is_ok());
        p.kind = "port-name";
        p.commands = vec![vec![]];
        assert!(
            check_queueable(&p).is_err(),
            "an empty group is no commands"
        );
        p.commands = vec![vec!["interface 7".into()]];
        assert!(check_queueable(&p).is_ok());
    }

    /// The wanted-row shape of a membership submit: absent fields mean "leave
    /// alone", and the diff needs a complete row either way.
    #[test]
    fn a_wanted_row_defaults_to_the_current_one() {
        let p = port("7");
        let tagged_all: Vec<String> = p.tagged_vlans.iter().map(|v| v.to_string()).collect();
        assert_eq!(tagged_all, vec!["7"]);
        // `tagged: []` (the editor cleared every checkbox) is not the same as
        // `tagged: None` (the editor did not send the field).
        let intent = Intent {
            tagged: Some(vec![]),
            ..Default::default()
        };
        assert_eq!(intent.tagged.map(|v| v.len()), Some(0));
        let intent = Intent::default();
        assert!(intent.tagged.is_none());
    }

    /// A VLAN id that the CLI would refuse is refused before anything is queued.
    #[test]
    fn vlan_ids_are_checked() {
        assert!(vlan_id(&Intent {
            vlan: Some(225),
            ..Default::default()
        })
        .is_ok());
        assert!(vlan_id(&Intent {
            vlan: None,
            ..Default::default()
        })
        .is_err());
        assert!(vlan_id(&Intent {
            vlan: Some(0),
            ..Default::default()
        })
        .is_err());
        assert!(vlan_id(&Intent {
            vlan: Some(5000),
            ..Default::default()
        })
        .is_err());
    }
}
