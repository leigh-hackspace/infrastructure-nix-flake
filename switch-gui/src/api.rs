//! The JSON view of a snapshot, for `/api/snapshot`.
//!
//! Hand-built with [`crate::jsonw`] rather than a serialise derive: the shape
//! is small, it is consumed by the console link and by curl, and it keeps this
//! tool dependency-free.  Field names are stable once published, so they are
//! documented by [`Obj`] calls rather than inferred from the Rust structs.
//!
//! Everything goes through [`Obj`], which owns the comma placement.  Doing it
//! by hand produced `"id":1,,"name":...` and `"trunkType":""untaggedVlan":1`
//! (caught by the round-trip test below), which is the argument for not
//! hand-rolling separators at all.

use crate::jsonw::quoted;
use crate::model::{Snapshot, Vlan};

/// One JSON object under construction.
///
/// `first` is what keeps the separators correct: every writer prepends a comma
/// except the first, so callers cannot forget one or add two.
struct Obj {
    s: String,
    first: bool,
}

impl Obj {
    fn new() -> Obj {
        Obj {
            s: String::from("{"),
            first: true,
        }
    }

    fn sep(&mut self) {
        if !self.first {
            self.s.push(',');
        }
        self.first = false;
    }

    /// Write `"key":` and leave the value to the caller.  Every writer goes
    /// through here, which is what keeps the quotes and separators correct —
    /// writing them per method is how a missing opening quote once made the
    /// whole endpoint invalid.
    fn key(&mut self, key: &str) {
        self.sep();
        self.s.push('"');
        self.s.push_str(key);
        self.s.push_str("\":");
    }

    fn str(&mut self, key: &str, value: &str) -> &mut Obj {
        self.key(key);
        self.s.push_str(&quoted(value));
        self
    }

    fn num(&mut self, key: &str, value: u64) -> &mut Obj {
        self.key(key);
        self.s.push_str(&value.to_string());
        self
    }

    /// A number that may be absent, written as JSON `null` rather than skipped:
    /// a client should not have to distinguish "no untagged VLAN" from a field
    /// that a different version renamed.
    fn opt_num(&mut self, key: &str, value: Option<u32>) -> &mut Obj {
        self.key(key);
        match value {
            Some(v) => self.s.push_str(&v.to_string()),
            None => self.s.push_str("null"),
        }
        self
    }

    /// A boolean that may be absent (`null`), like [`Obj::opt_num`].
    fn opt_bool(&mut self, key: &str, value: Option<bool>) -> &mut Obj {
        self.key(key);
        self.s.push_str(match value {
            Some(true) => "true",
            Some(false) => "false",
            None => "null",
        });
        self
    }

    /// A value that is already serialised JSON (a nested object or array).
    fn raw(&mut self, key: &str, json: &str) -> &mut Obj {
        self.key(key);
        self.s.push_str(json);
        self
    }

    fn into_string(&mut self) -> String {
        self.s.push('}');
        std::mem::take(&mut self.s)
    }
}

/// A JSON array of already-serialised items.
fn array(items: &[String]) -> String {
    format!("[{}]", items.join(","))
}

pub fn snapshot_json(s: &Snapshot) -> String {
    let mut root = Obj::new();

    root.str("switch", &s.switch);

    let mut sys = Obj::new();
    sys.str("name", &s.system.name)
        .str("location", &s.system.location)
        .str("contact", &s.system.contact)
        .str("software", &s.system.software)
        .str("rom", &s.system.rom)
        .str("mac", &s.system.mac)
        .str("serial", &s.system.serial)
        .str("uptime", &s.system.uptime)
        .str("cpu", &s.system.cpu);
    root.raw("system", &sys.into_string());

    let vlans: Vec<String> = s.vlans.iter().map(vlan_json).collect();
    root.raw("vlans", &array(&vlans));

    let ports: Vec<String> = s.sorted_ports().iter().map(|p| port_json(p)).collect();
    root.raw("ports", &array(&ports));

    let warnings: Vec<String> = s.warnings.iter().map(|w| quoted(w)).collect();
    root.raw("warnings", &array(&warnings));

    root.num("takenAt", s.taken_at_secs)
        .num("portCount", s.ports.len() as u64);

    root.into_string()
}

fn vlan_json(v: &Vlan) -> String {
    let mut o = Obj::new();
    o.num("id", v.id as u64)
        .str("name", &v.name)
        .str("status", &v.status)
        .str("jumbo", &v.jumbo);
    o.into_string()
}

fn port_json(p: &crate::model::Port) -> String {
    let mut o = Obj::new();
    o.str("id", &p.id)
        .str("name", &p.name)
        .str("media", &p.media)
        .str("stpState", &p.stp_state)
        .str("stpStateFrom", &p.stp_state_from)
        .opt_bool("linkUp", p.link_up)
        .str("trunkGroup", &p.trunk_group)
        .str("trunkType", &p.trunk_type)
        .opt_num("untaggedVlan", p.untagged_vlan);
    let tagged: Vec<String> = p.tagged_vlans.iter().map(|v| v.to_string()).collect();
    o.raw("taggedVlans", &array(&tagged));
    match &p.counters {
        Some(c) => {
            let mut co = Obj::new();
            co.str("port", &c.port)
                .str("totalBytes", &c.total_bytes)
                .str("totalFrames", &c.total_frames)
                .str("errorsRx", &c.errors_rx)
                .str("dropsRx", &c.drops_rx)
                .str("flowControl", &c.flow_ctrl)
                .str("bcastLimit", &c.bcast_limit);
            o.raw("counters", &co.into_string());
        }
        None => {
            o.raw("counters", "null");
        }
    }
    o.into_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Counters, Port};

    fn fixture() -> Snapshot {
        let mut s = Snapshot::default();
        s.switch = "switch3".into();
        s.system.name = "switch1".into();
        s.system.contact = r#"a "quoted" \ backslash"#.into();
        s.vlans.push(Vlan {
            id: 1,
            name: "DEFAULT".into(),
            status: "Port-based".into(),
            jumbo: "No".into(),
        });
        let mut p = Port::new("1");
        p.untagged_vlan = Some(1);
        p.tagged_vlans = vec![232, 225];
        p.link_up = Some(true);
        p.counters = Some(Counters {
            port: "1".into(),
            total_bytes: "2,993,652...".into(),
            total_frames: "1".into(),
            errors_rx: "0".into(),
            drops_rx: "0".into(),
            flow_ctrl: "off".into(),
            bcast_limit: "0".into(),
        });
        s.ports.insert("1".into(), p);
        // A second port with nothing filled in: every optional field is then
        // absent, which is where separator bugs hide.
        s.ports.insert("2".into(), Port::new("2"));
        s.warnings.push("show trunks: nope".into());
        s.taken_at_secs = 1_700_000_000;
        s
    }

    /// The real test: whatever we emit has to parse through the repo's own
    /// strict parser, and parse back to the same values.
    #[test]
    fn a_snapshot_serialises_to_valid_json() {
        let json = snapshot_json(&fixture());
        let v = common_json::parse(&json).unwrap_or_else(|e| panic!("invalid JSON: {e}\n{json}"));
        assert_eq!(
            v.at(&["system", "name"]).and_then(|x| x.as_str()),
            Some("switch1")
        );
        assert_eq!(v.at(&["switch"]).and_then(|x| x.as_str()), Some("switch3"));
        assert_eq!(
            v.at(&["system", "contact"]).and_then(|x| x.as_str()),
            Some(r#"a "quoted" \ backslash"#)
        );
        assert_eq!(v.at(&["takenAt"]).and_then(|x| x.as_f64()), Some(1.7e9));
        assert_eq!(v.at(&["portCount"]).and_then(|x| x.as_f64()), Some(2.0));
        assert!(json.contains(r#""untaggedVlan":1"#));
        assert!(json.contains(r#""taggedVlans":[232,225]"#));
        assert!(json.contains("show trunks: nope"));
    }

    #[test]
    fn an_empty_snapshot_is_still_well_formed() {
        let json = snapshot_json(&Snapshot::default());
        common_json::parse(&json).expect("valid JSON");
        assert!(json.contains(r#""vlans":[]"#));
        assert!(json.contains(r#""ports":[]"#));
        assert!(json.contains(r#""warnings":[]"#));
    }

    /// A port with no data must produce `null`s, not missing or doubled
    /// separators — this is the case that the hand-rolled version got wrong.
    #[test]
    fn empty_fields_become_nulls() {
        let json = snapshot_json(&fixture());
        let v = common_json::parse(&json).unwrap();
        let ports = v.get("ports").and_then(|p| p.as_array()).unwrap();
        assert_eq!(ports.len(), 2);
        let second = &ports[1];
        assert_eq!(second.get("untaggedVlan"), Some(&common_json::Json::Null));
        assert_eq!(second.get("counters"), Some(&common_json::Json::Null));
        assert_eq!(second.get("linkUp"), Some(&common_json::Json::Null));
        let first = &ports[0];
        assert_eq!(first.get("linkUp"), Some(&common_json::Json::Bool(true)));
        assert_ne!(
            first.get("counters"),
            Some(&common_json::Json::Null),
            "the populated port should have counters"
        );
    }

    /// Ports come out in patch-panel order, which is the whole point of the
    /// endpoint being useful to a human with a curl command.
    #[test]
    fn ports_are_sorted() {
        let mut s = Snapshot::default();
        for id in ["48", "Trk1", "9", "1"] {
            s.ports.insert(id.into(), Port::new(id));
        }
        let json = snapshot_json(&s);
        let v = common_json::parse(&json).unwrap();
        let ports = v.get("ports").and_then(|p| p.as_array()).unwrap();
        let ids: Vec<&str> = ports
            .iter()
            .map(|p| p.get("id").and_then(|i| i.as_str()).unwrap())
            .collect();
        assert_eq!(ids, vec!["1", "9", "48", "Trk1"]);
    }
}
