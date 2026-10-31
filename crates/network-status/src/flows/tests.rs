use super::*;
use common_json::{parse as json, Json};

/// `10.3.0.0/16` and `10.3.14.0/24` are ours; `192.168.49.1` (the tailnet peer
/// that queries this router's DNS) and everything public is not.
fn local() -> Localness {
    Localness::new(
        &"10.3.0.0/16 10.3.14.0/24"
            .split_whitespace()
            .map(String::from)
            .collect::<Vec<String>>(),
    )
}

/// Rows as gw printed them on 2026-10-25, with the two byte counters made
/// writable: `counted` substitutes them, and each placeholder ends in a space so
/// that no real counter value can contain it.
const DOWNLOAD: &str = "ipv4     2 tcp      6 431999 ESTABLISHED src=10.3.1.20 dst=104.16.246.239 sport=58506 dport=443 packets=535 bytes=ORIG  src=104.16.246.239 dst=81.187.223.123 sport=443 dport=58506 packets=1828 bytes=REPLY  [ASSURED] mark=0 zone=0 use=2";
const INBOUND: &str = "ipv4     2 udp      17 8 src=192.168.49.1 dst=10.3.1.1 sport=40087 dport=53 packets=1 bytes=ORIG  src=10.3.1.1 dst=192.168.49.1 sport=53 dport=40087 packets=1 bytes=REPLY  mark=0 zone=0 use=2";
const TUNNEL: &str = "ipv4     2 tcp      6 60 ESTABLISHED src=192.168.20.82 dst=172.66.0.218 sport=55676 dport=443 packets=954 bytes=ORIG  src=172.66.0.218 dst=192.168.20.82 sport=443 dport=55676 packets=1183 bytes=REPLY  [ASSURED] mark=0 zone=0 use=2";
const DNS: &str = "ipv4     2 udp      17 4 src=10.3.1.30 dst=10.3.1.1 sport=51522 dport=53 packets=2 bytes=ORIG  src=10.3.1.1 dst=10.3.1.30 sport=53 dport=51522 packets=2 bytes=REPLY  mark=0 zone=0 use=2";
/// The one row with no counters at all: real, and created before `acct` was
/// switched on.  Also a LAN-to-LAN flow (frigate pulling a camera's RTSP).
const RTSP: &str = "ipv4     2 tcp      6 75890 ESTABLISHED src=10.3.1.30 dst=10.3.14.13 sport=60486 dport=554 src=10.3.14.13 dst=10.3.1.30 sport=554 dport=60486 [ASSURED] mark=0 zone=0 use=2";

fn table(rows: &[String]) -> String {
    rows.join("\n")
}

/// `row` with the two `bytes=` counters set (original tuple first, then the
/// reply tuple, which is the order the kernel prints them in).
fn counted(row: &str, orig: u64, reply: u64) -> String {
    row.replace("bytes=ORIG", &format!("bytes={orig}"))
        .replace("bytes=REPLY", &format!("bytes={reply}"))
}

fn update(flows: &mut Flows, ts: i64, rows: &[String]) {
    flows.update(ts, &table(rows), &local(), 15);
}

fn api(flows: &Flows, q: &Query) -> Json {
    json(&flows.json(q, &Query::default(), 5))
        .unwrap_or_else(|e| panic!("the connections API emitted invalid JSON ({e})"))
}

fn q() -> Query {
    Query {
        min_age: 0,
        ..Default::default()
    }
}

fn rows_out(api: &Json) -> &[Json] {
    api.get("connections").unwrap().as_array().unwrap()
}

fn field(row: &Json, key: &str) -> Option<f64> {
    row.get(key).and_then(|v| v.as_f64())
}

#[test]
fn a_download_shows_who_and_how_fast() {
    let mut flows = Flows::new(&[], None);
    let big = || counted(DOWNLOAD, 1_000, 1_000_000);

    // First sample: the kernel's totals are already known, but a rate needs
    // two readings, so it is `null` and not 0 — the difference is what stops
    // a just-seen connection looking like an idle one.
    update(&mut flows, 1000, &[big()]);
    let out = api(&flows, &q());
    let rows = rows_out(&out);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("host").and_then(|v| v.as_str()),
        Some("10.3.1.20")
    );
    assert_eq!(
        rows[0].get("peer").and_then(|v| v.as_str()),
        Some("104.16.246.239")
    );
    assert_eq!(rows[0].get("kind").and_then(|v| v.as_str()), Some("out"));
    assert_eq!(
        rows[0].get("service").and_then(|v| v.as_str()),
        Some("https")
    );
    assert_eq!(field(&rows[0], "bytes_down"), Some(1_000_000.0));
    assert_eq!(field(&rows[0], "bytes_up"), Some(1_000.0));
    assert_eq!(field(&rows[0], "age"), Some(0.0));
    assert!(
        rows[0].get("down_bps").map(|v| v.as_f64().is_none()) == Some(true),
        "no rate before a second reading: {:?}",
        rows[0]
    );

    // Five seconds later: 5 MB more down, 1 KB more up.
    update(&mut flows, 1005, &[counted(DOWNLOAD, 2_000, 6_000_000)]);
    let out = api(&flows, &q());
    let rows = rows_out(&out);
    assert_eq!(field(&rows[0], "down_bps"), Some(1_000_000.0));
    assert_eq!(field(&rows[0], "up_bps"), Some(200.0));
    assert_eq!(field(&rows[0], "age"), Some(5.0));
    assert_eq!(field(&rows[0], "samples"), Some(2.0));
    assert_eq!(field(&out, "active"), Some(1.0));
    assert_eq!(field(&out, "interval"), Some(5.0));
    // The totals block is the same sum, which is how the page can say
    // "the network is pulling 1 MB/s" without adding rows up itself.
    assert_eq!(
        field(out.at(&["totals"]).unwrap(), "down_bps"),
        Some(1_000_000.0)
    );
}

#[test]
fn the_age_filter_hides_a_connection_that_has_not_been_running_long() {
    // This is the 5-second rule the page exists for, and the reason the
    // default min_age is one sample interval: a one-sample-old connection has
    // no rate to show.
    let mut flows = Flows::new(&[], None);
    let aged = Query {
        min_age: 5,
        ..Default::default()
    };

    update(&mut flows, 1000, &[counted(DOWNLOAD, 1, 1)]);
    let out = api(&flows, &aged);
    assert!(rows_out(&out).is_empty(), "age 0 < 5");
    assert_eq!(field(&out, "tracked"), Some(1.0), "tracked, just not shown");
    assert_eq!(field(&out, "active"), Some(1.0));

    update(&mut flows, 1004, &[counted(DOWNLOAD, 2, 2)]);
    let out = api(&flows, &aged);
    assert!(rows_out(&out).is_empty(), "age 4 < 5");
    update(&mut flows, 1005, &[counted(DOWNLOAD, 3, 3)]);
    let out = api(&flows, &aged);
    assert_eq!(rows_out(&out).len(), 1, "age 5 >= 5");
    assert_eq!(field(&out, "matched"), Some(1.0));
}

#[test]
fn a_reused_tuple_starts_its_history_again() {
    // Ports are ephemeral: the same five fields can come back belonging to a
    // different connection.  Reporting the old age and a negative rate would
    // both be lies, so the flow restarts instead.
    let mut flows = Flows::new(&[], None);
    update(&mut flows, 1000, &[counted(DOWNLOAD, 5_000, 5_000_000)]);
    update(&mut flows, 1005, &[counted(DOWNLOAD, 6_000, 6_000_000)]);
    update(&mut flows, 1010, &[counted(DOWNLOAD, 1_000, 2_000)]);

    let out = api(&flows, &q());
    let rows = rows_out(&out);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        field(&rows[0], "age"),
        Some(0.0),
        "a new connection, not an old one"
    );
    assert_eq!(field(&rows[0], "samples"), Some(1.0));
    assert!(rows[0].get("down_bps").unwrap().as_f64().is_none());
    assert_eq!(
        field(&rows[0], "bytes_down"),
        Some(2_000.0),
        "the new connection's own total"
    );
}

#[test]
fn down_is_always_peer_to_host_whichever_way_the_connection_ran() {
    let mut flows = Flows::new(&[], None);
    // Out: services1 pulling from Cloudflare.  Up: the same box, and the
    // tailnet peer reaching this router's DNS from the other direction.
    update(
        &mut flows,
        1000,
        &[
            counted(DOWNLOAD, 1_000, 1_000_000),
            counted(INBOUND, 81, 137),
        ],
    );
    update(
        &mut flows,
        1005,
        &[
            counted(DOWNLOAD, 2_000, 6_000_000),
            counted(INBOUND, 181, 1_137),
        ],
    );

    let out = api(
        &flows,
        &Query {
            sort: Sort::Host,
            ..q()
        },
    );
    let rows = rows_out(&out);
    assert_eq!(rows.len(), 2);

    let out_row = rows
        .iter()
        .find(|r| r.get("kind").and_then(|v| v.as_str()) == Some("out"))
        .expect("the download");
    let in_row = rows
        .iter()
        .find(|r| r.get("kind").and_then(|v| v.as_str()) == Some("in"))
        .expect("the inbound query");

    // `in` is the tailnet peer's request arriving: the *original* tuple is
    // the download from this router's point of view, which is the one case
    // where "down" is not the reply tuple.
    assert_eq!(
        in_row.get("host").and_then(|v| v.as_str()),
        Some("10.3.1.1")
    );
    assert_eq!(
        in_row.get("peer").and_then(|v| v.as_str()),
        Some("192.168.49.1")
    );
    assert_eq!(
        field(in_row, "down_bps"),
        Some(20.0),
        "peer→host: (181-81)/5"
    );
    assert_eq!(
        field(in_row, "up_bps"),
        Some(200.0),
        "host→peer: (1137-137)/5"
    );
    assert_eq!(in_row.get("service").and_then(|v| v.as_str()), Some("dns"));

    assert_eq!(field(out_row, "down_bps"), Some(1_000_000.0));
    assert_eq!(field(out_row, "up_bps"), Some(200.0));
}

#[test]
fn both_sides_local_is_lan_and_neither_is_wan() {
    let mut flows = Flows::new(&[], None);
    update(
        &mut flows,
        1000,
        &[counted(DNS, 128, 251), counted(TUNNEL, 56_154, 20_096_487)],
    );
    let out = api(&flows, &q());
    let kinds: Vec<(&str, &str)> = rows_out(&out)
        .iter()
        .map(|r| {
            (
                r.get("kind").and_then(|v| v.as_str()).unwrap(),
                r.get("host").and_then(|v| v.as_str()).unwrap(),
            )
        })
        .collect();
    assert!(
        kinds.contains(&("lan", "10.3.1.30")),
        "a DNS query between two LAN boxes: {kinds:?}"
    );
    assert!(
        kinds.contains(&("wan", "192.168.20.82")),
        "the router's own traffic out of the uplink has no LAN end: {kinds:?}"
    );
}

#[test]
fn a_connection_the_kernel_is_not_counting_has_null_totals_not_zero() {
    // `acct` off when the connection began: no counters.  Zeros here would
    // read as "this RTSP stream has moved nothing", which is the opposite of
    // the truth, and the reason the sysctl is on is invisible.
    let mut flows = Flows::new(&[], None);
    update(&mut flows, 1000, &[RTSP.to_string()]);
    let out = api(&flows, &q());
    let rows = rows_out(&out);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("bytes_down").unwrap().as_u64().is_none());
    assert_eq!(
        rows[0].get("accounted").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(field(&out, "unaccounted"), Some(1.0));
    assert_eq!(
        field(out.at(&["totals"]).unwrap(), "unaccounted"),
        Some(1.0)
    );

    // A counted connection in the same table is counted, not tainted, and
    // the uncounted one still contributes nothing to the byte totals.
    update(
        &mut flows,
        1005,
        &[RTSP.to_string(), counted(DOWNLOAD, 1, 1)],
    );
    let out = api(&flows, &q());
    assert_eq!(field(&out, "active"), Some(2.0));
    assert_eq!(field(&out, "unaccounted"), Some(1.0), "one of the two");
    let totals = out.at(&["totals"]).unwrap();
    assert_eq!(field(totals, "unaccounted"), Some(1.0));
    assert_eq!(
        field(totals, "bytes_down"),
        Some(1.0),
        "only the counted flow's byte"
    );
    let rows = rows_out(&out);
    let flag = |svc: &str| {
        rows.iter()
            .find(|r| r.get("service").and_then(|v| v.as_str()) == Some(svc))
            .unwrap_or_else(|| panic!("no {svc} row in {rows:?}"))
            .get("accounted")
            .and_then(|v| v.as_bool())
    };
    assert_eq!(flag("rtsp"), Some(false));
    assert_eq!(flag("https"), Some(true));
}

#[test]
fn a_connection_that_vanishes_survives_the_grace_then_stops_being_active() {
    let mut flows = Flows::new(&[], None);
    update(&mut flows, 1000, &[counted(DOWNLOAD, 1, 1)]);
    update(&mut flows, 1005, &[counted(DOWNLOAD, 2, 2)]);
    // Gone from the table (a UDP entry expired between packets, say), but
    // inside `--conn-stale`: remembered, so its age does not restart.
    update(&mut flows, 1010, &[]);
    let out = api(&flows, &q());
    assert_eq!(field(&out, "tracked"), Some(1.0), "still remembered");
    assert_eq!(field(&out, "active"), Some(0.0), "but not in the table now");
    assert!(rows_out(&out).is_empty());
    // Past the grace, it is forgotten entirely.
    update(&mut flows, 1040, &[]);
    let out = api(&flows, &q());
    assert_eq!(field(&out, "tracked"), Some(0.0));
}

/// `DOWNLOAD` to a different peer: two rows that differ only in their peer
/// are two connections of one host, which is what the per-host roll-up is
/// for.  Both tuples move together, as they would in a real entry.
fn dl(peer: &str, orig: u64, reply: u64) -> String {
    counted(&DOWNLOAD.replace("104.16.246.239", peer), orig, reply)
}

#[test]
fn hosts_roll_up_every_active_connection_not_just_the_page() {
    let mut flows = Flows::new(&[], None);
    let slow = |peer: &str, n: u64| dl(peer, 1_000 + n, 1_000_000 + n * 10_000);
    update(
        &mut flows,
        1000,
        &[
            slow("104.16.246.239", 0),
            slow("104.16.247.239", 0),
            counted(DNS, 100, 200),
        ],
    );
    update(
        &mut flows,
        1005,
        &[
            slow("104.16.246.239", 1_000), // 2 MB/s down
            slow("104.16.247.239", 100),   // 200 KB/s down
            counted(DNS, 150, 300),        // 10/20 B/s
        ],
    );

    // One host, two connections, plus a LAN DNS client of its own.
    let out = api(&flows, &q());
    let hosts = out.get("hosts").unwrap().as_array().unwrap();
    assert_eq!(hosts.len(), 2, "one entry per host, ranked by rate");
    let first = &hosts[0];
    assert_eq!(
        first.get("addr").and_then(|v| v.as_str()),
        Some("10.3.1.20")
    );
    assert_eq!(
        field(first, "conns"),
        Some(2.0),
        "both of its connections counted"
    );
    assert_eq!(
        field(first, "down_bps"),
        Some(2_200_000.0),
        "and their rates summed: 2 MB/s + 200 KB/s"
    );
    assert_eq!(
        hosts[1].get("addr").and_then(|v| v.as_str()),
        Some("10.3.1.30")
    );
    assert_eq!(field(&hosts[1], "conns"), Some(1.0));

    // Turning the age filter up must hide rows, not the answer to "who is
    // the biggest talker", so the roll-up ignores it.
    let out = api(
        &flows,
        &Query {
            min_age: 9_999,
            ..q()
        },
    );
    assert!(rows_out(&out).is_empty(), "the page is filtered…");
    let hosts = out.get("hosts").unwrap().as_array().unwrap();
    assert_eq!(
        hosts[0].get("addr").and_then(|v| v.as_str()),
        Some("10.3.1.20"),
        "…the ranking is not"
    );
    assert_eq!(field(&hosts[0], "conns"), Some(2.0));
}

#[test]
fn sorting_is_by_current_rate_and_the_limit_makes_the_page() {
    let mut flows = Flows::new(&[], None);
    // One host, two connections: the second peer is the one pulling data.
    update(
        &mut flows,
        1000,
        &[
            dl("104.16.246.239", 1_000, 1_000_000),
            dl("104.16.247.239", 1_000, 1_000_000),
        ],
    );
    update(
        &mut flows,
        1005,
        &[
            dl("104.16.246.239", 1_100, 1_050_000),
            dl("104.16.247.239", 2_000, 11_000_000),
        ],
    );

    let out = api(&flows, &q());
    let rows = rows_out(&out);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].get("peer").and_then(|v| v.as_str()),
        Some("104.16.247.239"),
        "the 2 MB/s connection comes first"
    );
    assert_eq!(field(&rows[0], "down_bps"), Some(2_000_000.0));
    assert_eq!(field(&rows[1], "down_bps"), Some(10_000.0));

    // Sorting by bytes rather than rate puts the long-tailed connection on
    // top instead, which is a different question and why both exist.
    let out = api(
        &flows,
        &Query {
            sort: Sort::Bytes,
            ..q()
        },
    );
    assert_eq!(
        rows_out(&out)[0].get("peer").and_then(|v| v.as_str()),
        Some("104.16.247.239")
    );

    // `limit` truncates the page and says so, rather than pretending the
    // table was that short.
    let out = api(&flows, &Query { limit: 1, ..q() });
    assert_eq!(rows_out(&out).len(), 1);
    assert_eq!(field(&out, "returned"), Some(1.0));
    assert_eq!(field(&out, "matched"), Some(2.0));
    assert_eq!(field(&out, "tracked"), Some(2.0));
}

#[test]
fn the_search_box_matches_names_addresses_ports_and_kinds() {
    let mut path = std::env::temp_dir();
    path.push(format!("network-status-connections-{}", std::process::id()));
    let path = path.to_string_lossy().into_owned();
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, "10.3.1.20 services1.int.leighhack.org services1\n").unwrap();

    let mut flows = Flows::new(std::slice::from_ref(&path), None);
    let rows = [counted(DOWNLOAD, 1, 1), counted(DNS, 1, 1)];
    update(&mut flows, 1000, &rows);
    update(&mut flows, 1005, &rows);

    let find = |needle: &str| -> usize {
        let q = Query {
            needle: Some(needle.into()),
            ..q()
        };
        rows_out(&api(&flows, &q)).len()
    };
    assert_eq!(find("services1"), 1, "by name, from the records file");
    assert_eq!(find("SERVICES1"), 1, "case-insensitively");
    assert_eq!(find("104.16"), 1, "by peer address fragment");
    assert_eq!(find("53"), 1, "by port");
    assert_eq!(find("https"), 1, "by service");
    assert_eq!(find("lan"), 1, "by kind");
    assert_eq!(find("10.3.1.30"), 1);
    assert_eq!(find("nothing-like-this"), 0);

    // The name appears on the row it belongs to, and nowhere else.
    let out = api(&flows, &q());
    let named = rows_out(&out)
        .iter()
        .filter(|r| r.get("host_name").map(|v| v.as_str().is_some()) == Some(true))
        .count();
    assert_eq!(named, 1);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn a_query_string_defaults_and_clamps() {
    let mut params = HashMap::new();
    params.insert("limit".into(), "5000".into());
    params.insert("sort".into(), "nonsense".into());
    params.insert("min_age".into(), "30".into());
    params.insert("q".into(), "  Aibox  ".into());
    let defaults = Query {
        min_age: 5,
        limit: 200,
        ..Default::default()
    };
    let q = parse_query(&params, &defaults);
    assert_eq!(q.min_age, 30);
    assert_eq!(q.limit, 200, "the page may ask for less, never for more");
    assert_eq!(q.sort, Sort::Rate, "an unknown column sorts by rate");
    assert_eq!(q.needle.as_deref(), Some("aibox"), "trimmed and folded");

    params.insert("min_age".into(), "later".into());
    params.insert("q".into(), "   ".into());
    let q = parse_query(&params, &defaults);
    assert_eq!(q.min_age, 5, "garbage falls back to the operator's default");
    assert_eq!(q.needle, None, "a blank search is no search");

    params.insert("limit".into(), "0".into());
    assert_eq!(
        parse_query(&params, &defaults).limit,
        1,
        "and not zero rows either"
    );

    params.insert("kind".into(), "in".into());
    assert_eq!(parse_query(&params, &defaults).kind, Some(Kind::In));
    params.insert("kind".into(), "everything".into());
    assert_eq!(parse_query(&params, &defaults).kind, None);
}

#[test]
fn the_table_is_reported_to_prometheus() {
    let mut flows = Flows::new(&[], None);
    update(
        &mut flows,
        1000,
        &[counted(DOWNLOAD, 1, 1), RTSP.to_string()],
    );
    update(&mut flows, 1005, &[counted(DOWNLOAD, 2, 2)]);
    let m = flows.metrics(5);
    // Both connections are still *tracked* after one sample away: the RTSP
    // row is inside the --conn-stale grace, which is why `tracked` and
    // `active` are different numbers and both are worth charting.
    assert!(m.contains("router_connections_tracked 2\n"), "{m}");
    assert!(m.contains("router_connections_active 1\n"), "{m}");
    assert!(m.contains("router_connections_long_running 1\n"), "{m}");
    assert!(m.contains("router_connections_unaccounted 0\n"), "{m}");
    // The RTSP row disappeared at the second sample, and it had no counters.
    for line in m.lines() {
        assert!(!line.contains("NaN"), "{line}");
    }
}

#[test]
fn an_empty_table_is_a_valid_empty_answer() {
    // A router with nothing forwarded, or a first sample of nothing: the
    // page needs the same shape, not an error.
    let mut flows = Flows::new(&[], None);
    update(&mut flows, 1000, &[]);
    let out = api(&flows, &q());
    assert!(rows_out(&out).is_empty());
    assert_eq!(field(&out, "tracked"), Some(0.0));
    assert!(out.get("hosts").unwrap().as_array().unwrap().is_empty());
    assert_eq!(
        out.get("filter").unwrap().get("sort").unwrap().as_str(),
        Some("rate")
    );
    assert_eq!(field(out.at(&["defaults"]).unwrap(), "min_age"), Some(5.0));
}

#[test]
fn a_counter_that_goes_backwards_rebaselines_instead_of_going_negative() {
    // The kernel's byte counters are 32-bit, so a flow that moves more than
    // 4 GiB wraps them; and a tuple whose entry expired and was handed to a new
    // connection starts from zero again.  Both arrive as a smaller number, and
    // neither may be reported as negative bytes or as a negative rate.
    //
    // The cost of choosing "start over" over "assume a wrap" is measured, not
    // theoretical: the flow reads as newly seen for one sample, so the default
    // 5 s threshold hides it for one sample, and its byte total reads 4 GiB
    // light.  What it never does is show a download going down.
    let mut flows = Flows::default();
    update(&mut flows, 1000, &[counted(DOWNLOAD, 4_294_000_000, 100)]);
    let out = api(&flows, &q());
    assert_eq!(
        field(&rows_out(&out)[0], "bytes_up"),
        Some(4_294_000_000.0),
        "just under 2^32, which is the point where the kernel's counter wraps"
    );

    update(&mut flows, 1005, &[counted(DOWNLOAD, 2_000, 100)]);
    let out = api(&flows, &q());
    let row = &rows_out(&out)[0];
    assert_eq!(
        field(row, "bytes_up"),
        Some(2_000.0),
        "the new baseline, not a huge negative"
    );
    assert_eq!(
        row.get("up_bps").unwrap().as_f64(),
        None,
        "no rate across a gap this size"
    );
    assert_eq!(
        field(row, "samples"),
        Some(1.0),
        "and the history is dropped, not averaged"
    );

    // The next sample is a normal sample again.
    update(&mut flows, 1010, &[counted(DOWNLOAD, 12_000, 100)]);
    let out = api(&flows, &q());
    let row = &rows_out(&out)[0];
    assert_eq!(field(row, "up_bps"), Some(2_000.0), "(12000-2000)/5");
    assert_eq!(field(row, "samples"), Some(2.0));
}
