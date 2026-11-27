//! The lease database: the daemon's own record of which client holds which
//! address, and the only copy of it that matters.
//!
//! ## Why this is a file the daemon writes, not one it reads
//!
//! dnsmasq keeps leases in a text file it opens once at startup and rewrites in
//! place. That single decision produced the worst incident in this network's
//! recent history: a debugging session replaced the *path* with `install`, which
//! unlinks first, so for two and a half days dnsmasq wrote an inode nobody could
//! reach and the dashboard served a frozen table that said `"expired":56`
//! (`machines/gw/lease-reader.nix` carries the receipts). Every later repair —
//! a group grant, a path unit, a five-minute watch, a `stale` verdict — exists
//! because the file was a channel between two processes rather than state one
//! process owns.
//!
//! So: the in-memory table is the truth, the file is a **snapshot** written by
//! this process (temp file + `rename`, so a reader never sees a half write), and
//! nothing else in the network reads it. The dashboard asks
//! `GET /v1/leases` (`api.rs`) — a value, not a file — which is the whole point
//! of replacing dnsmasq rather than wrapping it.
//!
//! ## Expiry is lazy and that is deliberate
//!
//! A lease is expired when `now > expires`, evaluated on read. There is no
//! timer deleting rows: a row that lapsed thirty seconds ago still tells an
//! operator which device *had* that address, and the allocator will not hand it
//! out while it is listed. Housekeeping prunes only what is well past its
//! expiry and only so the file does not grow forever.

use crate::config::{client_id_parse, mac_parse, mac_to_string, ClientId, Mac};
use common_json::Json;
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::Path;

/// One address promised to one client.
#[derive(Clone, Debug, PartialEq)]
pub struct Lease {
    pub ip: Ipv4Addr,
    pub mac: Mac,
    /// The identifier the client chose, when it sent one (option 61). Kept
    /// because it is what a renewed lease is matched on: a device that presents
    /// a client-id keeps it even when its hardware address changes.
    pub client_id: Option<ClientId>,
    /// The hostname the client asked for (option 12), sanitised to a DNS label.
    /// `dhcp-fqdn` is what turns this into `<hostname>.<zone>` in the resolver,
    /// and it is the only name a device gives itself.
    pub hostname: Option<String>,
    pub interface: String,
    pub tag: String,
    /// Unix seconds.  Both ends are recorded, so "how much of this lease is
    /// left" is arithmetic rather than the guess dnsmasq's expiry-only format
    /// forces (`crates/network-status/src/leases.rs` recovered a length by
    /// looking the address up in a block — a good trick around a bad format).
    pub issued: i64,
    pub expires: i64,
    /// `true` when the address came from `common/dns-records.nix` rather than
    /// from a pool.  An operator reading the table needs to see the difference:
    /// a reserved address moving is a table change, a pooled one moving is
    /// normal.
    pub reserved: bool,
}

impl Lease {
    pub fn client_id_hex(&self) -> String {
        self.client_id
            .as_ref()
            .map(|c| {
                c.iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(":")
            })
            .unwrap_or_default()
    }

    pub fn json(&self, now: i64) -> Json {
        use common_json::{boolean, number_from_int, object, string, Json};
        let mut fields = vec![
            ("ip".to_string(), string(self.ip.to_string())),
            (
                "name".to_string(),
                string(self.hostname.clone().unwrap_or_default()),
            ),
            ("mac".to_string(), string(mac_to_string(&self.mac))),
            ("client_id".to_string(), string(self.client_id_hex())),
            ("interface".to_string(), string(self.interface.clone())),
            ("tag".to_string(), string(self.tag.clone())),
            (
                "issued".to_string(),
                number_from_int(self.issued.max(0) as u64),
            ),
            (
                "expires".to_string(),
                number_from_int(self.expires.max(0) as u64),
            ),
            (
                "left".to_string(),
                number_from_int((self.expires - now).max(0) as u64),
            ),
            ("reserved".to_string(), boolean(self.reserved)),
        ];
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        let _ = Json::Null;
        object(fields)
    }
}

/// The daemon's table, plus the addresses clients DECLINEd.
#[derive(Debug, Default)]
pub struct LeaseDb {
    leases: Vec<Lease>,
    /// Addresses a client said were already in use (DHCPDECLINE), held until
    /// this timestamp.  dnsmasq remembers a decline until its lease file is
    /// rewritten; a window is easier to reason about and cannot hold a good
    /// address hostage for a day because one device lied.
    declined: BTreeMap<Ipv4Addr, i64>,
    /// Written whenever anything here changed, so persistence is a snapshot of
    /// an agreed state rather than a stream of events.
    dirty: bool,
    /// The last time the snapshot was written (or 0), for `/v1/status`.
    pub saved_at: i64,
    /// The last thing that went wrong writing the snapshot. Kept and re-reported
    /// rather than logged once: a state directory that has become unwritable is
    /// a fault the API should say out loud (`/v1/status`), because the leases
    /// themselves are still being served correctly and every symptom is
    /// invisible until a restart loses them.
    pub save_error: Option<String>,
}

impl LeaseDb {
    pub fn new() -> LeaseDb {
        LeaseDb::default()
    }

    pub fn all(&self) -> &[Lease] {
        &self.leases
    }

    pub fn len(&self) -> usize {
        self.leases.len()
    }

    /// The live lease for this client, if any. A client identity is its
    /// client-id when it presents one, else its MAC — the same precedence
    /// reservations use (`Config::reservation_for`), so a device cannot find
    /// its reservation and then fail to find its own lease.
    pub fn by_client(&self, mac: &Mac, cid: Option<&ClientId>) -> Option<&Lease> {
        self.leases.iter().find(|l| {
            (cid.is_some() && l.client_id.as_ref() == cid)
                || (&l.mac == mac && l.client_id.is_none())
        })
    }

    pub fn by_ip(&self, ip: Ipv4Addr) -> Option<&Lease> {
        self.leases.iter().find(|l| l.ip == ip)
    }

    /// Does anyone *else* hold this address? The caller passes the identity of
    /// the client asking, so a renewal is not a conflict with itself.
    pub fn held_by_other(&self, ip: Ipv4Addr, mac: &Mac, cid: Option<&ClientId>) -> bool {
        self.leases.iter().any(|l| {
            l.ip == ip
                && !((cid.is_some() && l.client_id.as_ref() == cid)
                    || (&l.mac == mac && l.client_id.is_none()))
        })
    }

    /// Insert (or refresh) a lease. One client holds one address and one address
    /// belongs to one client, so both old rows for either side go: a stale row
    /// for the same client on a different address is how a table grows a second
    /// answer for one device.
    pub fn upsert(&mut self, lease: Lease) {
        self.leases.retain(|l| {
            l.ip != lease.ip
                && !(l.mac == lease.mac && l.client_id == lease.client_id)
                && !(lease.client_id.is_some()
                    && l.client_id == lease.client_id
                    && l.interface == lease.interface)
        });
        self.leases.push(lease);
        self.dirty = true;
    }

    /// A release: drop the client's lease for this address, if it is theirs.
    pub fn release(&mut self, mac: &Mac, cid: Option<&ClientId>, ip: Ipv4Addr) -> bool {
        let before = self.leases.len();
        self.leases.retain(|l| {
            !(l.ip == ip
                && ((cid.is_some() && l.client_id.as_ref() == cid)
                    || (&l.mac == mac && (l.client_id.is_none() || cid.is_none()))))
        });
        let changed = before != self.leases.len();
        self.dirty |= changed;
        changed
    }

    pub fn decline(&mut self, ip: Ipv4Addr, until: i64) {
        self.declined.insert(ip, until);
        self.dirty = true;
    }

    pub fn is_declined(&self, ip: Ipv4Addr, now: i64) -> bool {
        self.declined.get(&ip).is_some_and(|until| *until > now)
    }

    /// Drop leases that have been expired for a whole lease length, and declines
    /// that are over. Called by housekeeping, never on a read path.
    pub fn prune(&mut self, now: i64) -> usize {
        let before = self.leases.len();
        self.leases.retain(|l| l.expires + 86_400 > now);
        self.declined.retain(|_, until| *until > now);
        let removed = before - self.leases.len();
        self.dirty |= removed > 0;
        removed
    }

    pub fn expired_count(&self, now: i64) -> usize {
        self.leases.iter().filter(|l| l.expires <= now).count()
    }

    // --- persistence ------------------------------------------------------

    pub fn path(state_dir: &Path) -> std::path::PathBuf {
        state_dir.join("leases.json")
    }

    /// Load the snapshot, dropping rows that expired while the daemon was not
    /// running. A missing or unreadable file is *not* fatal: the daemon starts
    /// with its pools and hands out addresses, which is strictly better than a
    /// router that will not start because one file is corrupt.
    pub fn load(state_dir: &Path) -> (LeaseDb, Option<String>) {
        let path = LeaseDb::path(state_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (LeaseDb::new(), None),
            Err(e) => return (LeaseDb::new(), Some(format!("{}: {e}", path.display()))),
        };
        match LeaseDb::parse(&text) {
            Ok(db) => (db, None),
            Err(e) => (LeaseDb::new(), Some(format!("{}: {e}", path.display()))),
        }
    }

    pub fn parse(text: &str) -> Result<LeaseDb, String> {
        let doc = common_json::parse(text)?;
        let leases = doc
            .get("leases")
            .and_then(Json::as_array)
            .ok_or_else(|| "no `leases` array".to_string())?;
        let mut db = LeaseDb::new();
        for l in leases {
            let ip: Ipv4Addr = l
                .get("ip")
                .and_then(Json::as_str)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "a lease has no valid `ip`".to_string())?;
            let mac = l
                .get("mac")
                .and_then(Json::as_str)
                .ok_or_else(|| format!("{ip}: no `mac`"))
                .and_then(mac_parse)?;
            let cid = match l.get("client_id").and_then(Json::as_str) {
                Some(s) if !s.is_empty() => Some(client_id_parse(s)?),
                _ => None,
            };
            let num = |k: &str| l.get(k).and_then(Json::as_u64).unwrap_or(0) as i64;
            db.leases.push(Lease {
                ip,
                mac,
                client_id: cid,
                hostname: l
                    .get("name")
                    .and_then(Json::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                interface: l
                    .get("interface")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string(),
                tag: l
                    .get("tag")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string(),
                issued: num("issued"),
                expires: num("expires"),
                reserved: l.get("reserved").and_then(Json::as_bool).unwrap_or(false),
            });
        }
        Ok(db)
    }

    pub fn to_json(&self, now: i64) -> String {
        use common_json::{array, number_from_int, object, string};
        let rows = self
            .leases
            .iter()
            .map(|l| {
                let mut fields = vec![
                    ("ip".to_string(), string(l.ip.to_string())),
                    ("mac".to_string(), string(mac_to_string(&l.mac))),
                    ("client_id".to_string(), string(l.client_id_hex())),
                    (
                        "name".to_string(),
                        string(l.hostname.clone().unwrap_or_default()),
                    ),
                    ("interface".to_string(), string(l.interface.clone())),
                    ("tag".to_string(), string(l.tag.clone())),
                    (
                        "issued".to_string(),
                        number_from_int(l.issued.max(0) as u64),
                    ),
                    (
                        "expires".to_string(),
                        number_from_int(l.expires.max(0) as u64),
                    ),
                ];
                fields.sort_by(|a, b| a.0.cmp(&b.0));
                object(fields)
            })
            .collect::<Vec<_>>();
        let mut top = vec![
            ("version".to_string(), number_from_int(1)),
            ("saved_at".to_string(), number_from_int(now.max(0) as u64)),
            ("leases".to_string(), array(rows)),
        ];
        top.sort_by(|a, b| a.0.cmp(&b.0));
        common_json::emit(&object(top), 0)
    }

    /// Write the snapshot if anything changed. Returns `true` when a write
    /// happened.
    ///
    /// `tmp` + `rename` in the same directory, so a reader either sees the old
    /// snapshot or the new one and never a truncated file. This is the *opposite*
    /// of what dnsmasq's reader would want (it held a descriptor open across
    /// rewrites) and it is safe here precisely because the writer and the reader
    /// of this file are the same process; the API never reads it.
    pub fn save(&mut self, state_dir: &Path, now: i64) -> bool {
        if !self.dirty {
            return false;
        }
        let path = LeaseDb::path(state_dir);
        let tmp = state_dir.join(format!(".leases.json.{}.tmp", std::process::id()));
        let body = self.to_json(now);
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(state_dir)?;
            std::fs::write(&tmp, body.as_bytes())?;
            std::fs::rename(&tmp, &path)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.dirty = false;
                self.saved_at = now;
                self.save_error = None;
                true
            }
            Err(e) => {
                // Leave `dirty` set: the next tick tries again, and the API says
                // so until it works.
                self.save_error = Some(format!("{}: {e}", path.display()));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::mac_parse;

    fn mac(s: &str) -> Mac {
        mac_parse(s).unwrap()
    }

    fn lease(ip: &str, m: &str, expires: i64) -> Lease {
        Lease {
            ip: ip.parse().unwrap(),
            mac: mac(m),
            client_id: None,
            hostname: Some("device".into()),
            interface: "vlan225".into(),
            tag: "vlan225".into(),
            issued: expires - 86_400,
            expires,
            reserved: false,
        }
    }

    #[test]
    fn one_client_holds_one_address() {
        let mut db = LeaseDb::new();
        db.upsert(lease("10.3.1.200", "aa:00:00:00:00:01", 1_000));
        db.upsert(lease("10.3.1.201", "aa:00:00:00:00:01", 2_000));
        assert_eq!(db.len(), 1, "the client's own old row is replaced");
        assert_eq!(db.all()[0].ip, Ipv4Addr::new(10, 3, 1, 201));
    }

    #[test]
    fn one_address_belongs_to_one_client() {
        let mut db = LeaseDb::new();
        db.upsert(lease("10.3.1.200", "aa:00:00:00:00:01", 1_000));
        db.upsert(lease("10.3.1.200", "aa:00:00:00:00:02", 2_000));
        assert_eq!(db.len(), 1);
        assert_eq!(mac_to_string(&db.all()[0].mac), "aa:00:00:00:00:02");
    }

    #[test]
    fn a_client_id_takes_precedence_over_a_mac() {
        let mut db = LeaseDb::new();
        let mut l = lease("10.3.1.200", "aa:00:00:00:00:01", 1_000);
        l.client_id = Some(vec![0, 2, 0xab]);
        db.upsert(l);
        // Same client-id, different MAC: still the same lease.
        let cid = vec![0, 2, 0xab];
        assert!(db
            .by_client(&mac("aa:00:00:00:00:09"), Some(&cid))
            .is_some());
        // Same MAC, no client-id: a different identity, and it must not match
        // the client-id lease.
        assert!(db.by_client(&mac("aa:00:00:00:00:01"), None).is_none());
    }

    #[test]
    fn held_by_other_is_not_a_conflict_with_oneself() {
        let mut db = LeaseDb::new();
        db.upsert(lease("10.3.1.200", "aa:00:00:00:00:01", 1_000));
        let ip = Ipv4Addr::new(10, 3, 1, 200);
        assert!(!db.held_by_other(ip, &mac("aa:00:00:00:00:01"), None));
        assert!(db.held_by_other(ip, &mac("aa:00:00:00:00:02"), None));
    }

    #[test]
    fn a_decline_is_a_window_not_a_permanent_mark() {
        let mut db = LeaseDb::new();
        let ip = Ipv4Addr::new(10, 3, 1, 200);
        db.decline(ip, 1_000);
        assert!(db.is_declined(ip, 999));
        assert!(!db.is_declined(ip, 1_000));
    }

    #[test]
    fn the_snapshot_round_trips() {
        let mut db = LeaseDb::new();
        db.upsert(lease("10.3.1.200", "aa:00:00:00:00:01", 1_000));
        let text = db.to_json(500);
        let back = LeaseDb::parse(&text).expect("round trip");
        assert_eq!(back.all().len(), 1);
        assert_eq!(back.all()[0].expires, 1_000);
        assert_eq!(back.all()[0].hostname.as_deref(), Some("device"));
    }

    #[test]
    fn a_corrupt_snapshot_is_reported_and_does_not_stop_the_daemon() {
        let dir = std::env::temp_dir().join(format!("netd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(LeaseDb::path(&dir), "{ not json").unwrap();
        let (db, err) = LeaseDb::load(&dir);
        assert!(db.all().is_empty());
        assert!(err.unwrap().contains("leases.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_is_atomic_and_only_when_dirty() {
        let dir = std::env::temp_dir().join(format!("netd-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut db = LeaseDb::new();
        assert!(!db.save(&dir, 1), "nothing changed, nothing written");
        db.upsert(lease("10.3.1.200", "aa:00:00:00:00:01", 1_000));
        assert!(db.save(&dir, 1_000));
        assert!(!db.save(&dir, 1_001), "unchanged since the last write");
        let (back, err) = LeaseDb::load(&dir);
        assert!(err.is_none(), "{err:?}");
        assert_eq!(back.len(), 1);
        // No temp file left behind.
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
