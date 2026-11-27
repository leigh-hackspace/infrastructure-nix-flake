//! DNS message and zone tests: the parser's defences, and what the zone says.

use super::*;
use crate::config::Config;
use crate::lease::{Lease, LeaseDb};
use std::net::Ipv4Addr;

const CFG: &str = include_str!("../../tests/fixtures/minimal.json");

fn cfg() -> Config {
    Config::parse(CFG).expect("fixture parses")
}

fn query(name: &str, qtype: u16) -> Message {
    Message {
        id: 0x1234,
        flags: FLAG_RD,
        questions: vec![Question {
            name: name.to_string(),
            qtype,
            qclass: CLASS_IN,
        }],
        answers: Vec::new(),
        authority: Vec::new(),
        additional: Vec::new(),
        edns: None,
    }
}

fn ask(c: &Config, db: &LeaseDb, name: &str, qtype: u16) -> ZoneAnswer {
    answer(c, db, &query(name, qtype).questions[0], 1_000)
}

fn answers_has(a: &ZoneAnswer, rtype: u16, needle: &[u8]) -> bool {
    a.answers
        .iter()
        .any(|r| r.rtype == rtype && r.rdata == needle)
}

#[test]
fn a_query_round_trips_through_encode_and_parse() {
    let q = query("services1.int.example.org", TYPE_A);
    let bytes = q.encode();
    let back = Message::parse(&bytes).expect("our own query parses");
    assert_eq!(back.id, q.id);
    assert_eq!(back.questions, q.questions);
    assert_eq!(back.opcode(), 0);
}

#[test]
fn a_response_carries_the_question_back_and_the_ra_bit() {
    let q = query("services1.int.example.org", TYPE_A);
    let r = Message::response(&q);
    assert!(r.is_response());
    assert!(r.flags & FLAG_RA != 0);
    assert_eq!(r.questions, q.questions);
    let back = Message::parse(&r.encode()).expect("parses");
    assert!(back.is_response());
}

#[test]
fn name_compression_is_read_and_a_loop_is_refused() {
    // header + question name "a." then a pointer to itself.
    let mut bytes = vec![0u8; 12];
    bytes[5] = 1; // qdcount
    bytes.push(1);
    bytes.push(b'a');
    bytes.extend_from_slice(&[0xc0, 0x0c]); // points at offset 12, i.e. itself
    bytes.extend_from_slice(&TYPE_A.to_be_bytes());
    bytes.extend_from_slice(&CLASS_IN.to_be_bytes());
    let e = Message::parse(&bytes).unwrap_err();
    assert!(e.contains("loops") || e.contains("forward"), "{e}");
}

#[test]
fn a_label_over_63_bytes_is_refused() {
    let mut bytes = vec![0u8; 12];
    bytes[5] = 1; // one question
    bytes.push(64);
    bytes.extend_from_slice(&[b'a'; 64]);
    bytes.push(0);
    bytes.extend_from_slice(&TYPE_A.to_be_bytes());
    bytes.extend_from_slice(&CLASS_IN.to_be_bytes());
    assert!(Message::parse(&bytes).is_err());
}

#[test]
fn an_edns_query_advertises_its_size() {
    let mut q = query("example.com", TYPE_A);
    q.edns = Some(Edns {
        udp_size: 1232,
        do_bit: false,
    });
    let bytes = q.encode();
    let back = Message::parse(&bytes).unwrap();
    assert_eq!(back.edns.map(|e| e.udp_size), Some(1232));
    assert_eq!(back.udp_payload_size(), 1232);
    let no_edns = Message::parse(&query("example.com", TYPE_A).encode()).unwrap();
    assert_eq!(no_edns.udp_payload_size(), 512);
}

#[test]
fn the_zone_answers_a_records_and_nxdomains_nothing_else() {
    let c = cfg();
    let db = LeaseDb::new();
    let a = ask(&c, &db, "services1.int.example.org", TYPE_A);
    assert!(a.authoritative);
    assert_eq!(a.rcode, RCODE_NOERROR);
    assert!(answers_has(&a, TYPE_A, &[10, 0, 0, 20]));

    let aaaa = ask(&c, &db, "services1.int.example.org", TYPE_AAAA);
    assert_eq!(aaaa.rcode, RCODE_NOERROR, "a NODATA answer, not NXDOMAIN");
    assert!(aaaa.answers.is_empty());
    assert!(
        !aaaa.authority.is_empty(),
        "the SOA says how long to believe it"
    );

    let missing = ask(&c, &db, "nope.int.example.org", TYPE_A);
    assert_eq!(missing.rcode, RCODE_NXDOMAIN);

    let outside = ask(&c, &db, "example.com", TYPE_A);
    assert!(!outside.authoritative, "not ours: forward it");
}

#[test]
fn a_name_answers_for_both_families_it_has() {
    let c = cfg();
    let db = LeaseDb::new();
    let a = ask(&c, &db, "aibox.int.example.org", TYPE_AAAA);
    assert!(a.answers.iter().any(|r| r.rtype == TYPE_AAAA));
    let a = ask(&c, &db, "aibox.int.example.org", TYPE_ANY);
    assert_eq!(a.answers.len(), 2, "one A and one AAAA");
}

#[test]
fn the_apex_answers_soa_and_ns() {
    let c = cfg();
    let db = LeaseDb::new();
    let soa = ask(&c, &db, "int.example.org", TYPE_SOA);
    assert_eq!(soa.answers.len(), 1);
    assert_eq!(soa.answers[0].rtype, TYPE_SOA);
    let ns = ask(&c, &db, "int.example.org", TYPE_NS);
    assert_eq!(ns.answers[0].rtype, TYPE_NS);
}

#[test]
fn a_live_lease_is_resolvable_by_its_own_hostname() {
    let c = cfg();
    let mut db = LeaseDb::new();
    db.upsert(Lease {
        ip: Ipv4Addr::new(10, 0, 0, 178),
        mac: [0x02, 0, 0, 0, 0, 1],
        client_id: None,
        hostname: Some("daves-laptop".into()),
        interface: "vlan225".into(),
        tag: "vlan225".into(),
        issued: 0,
        expires: 2_000,
        reserved: false,
    });
    let fqdn = ask(&c, &db, "daves-laptop.int.example.org", TYPE_A);
    assert_eq!(fqdn.rcode, RCODE_NOERROR);
    assert!(answers_has(&fqdn, TYPE_A, &[10, 0, 0, 178]));
    let bare = ask(&c, &db, "daves-laptop", TYPE_A);
    assert!(answers_has(&bare, TYPE_A, &[10, 0, 0, 178]));
    // ... and it stops answering the moment the lease lapses, which is the
    // whole reason the table is consulted rather than a file.
    let expired = answer(
        &c,
        &db,
        &query("daves-laptop.int.example.org", TYPE_A).questions[0],
        9_999,
    );
    assert_eq!(expired.rcode, RCODE_NXDOMAIN);
}

#[test]
fn the_address_table_wins_over_a_lease_for_the_same_address() {
    let c = cfg();
    let mut db = LeaseDb::new();
    db.upsert(Lease {
        ip: Ipv4Addr::new(10, 0, 0, 20),
        mac: [0x02, 0, 0, 0, 0, 2],
        client_id: None,
        hostname: Some("impostor".into()),
        interface: "vlan225".into(),
        tag: "vlan225".into(),
        issued: 0,
        expires: 2_000,
        reserved: true,
    });
    let a = ask(&c, &db, "services1.int.example.org", TYPE_A);
    assert!(answers_has(&a, TYPE_A, &[10, 0, 0, 20]));
    // Both names exist for that address, and a PTR names both: a reverse answer
    // with one name would hide the other.
    let ptr = ask(&c, &db, &reverse_v4(Ipv4Addr::new(10, 0, 0, 20)), TYPE_PTR);
    assert!(ptr.answers.len() >= 2, "{:?}", ptr.answers);
}

#[test]
fn a_reverse_query_for_an_unknown_address_is_noerror_not_nxdomain() {
    let c = cfg();
    let db = LeaseDb::new();
    let a = ask(&c, &db, &reverse_v4(Ipv4Addr::new(10, 0, 0, 77)), TYPE_PTR);
    assert!(a.authoritative);
    assert_eq!(
        a.rcode, RCODE_NOERROR,
        "the name exists, it just has no target"
    );
    assert!(a.answers.is_empty());
}

#[test]
fn type_names_are_the_words_operators_type() {
    assert_eq!(type_name(TYPE_A), "A");
    assert_eq!(type_name(TYPE_AAAA), "AAAA");
    assert_eq!(type_name(TYPE_PTR), "PTR");
    assert_eq!(type_name(1234), "?");
}

#[test]
fn rdata_addresses_are_read_only_from_fixed_width_records() {
    let r = a_record("x", Ipv4Addr::new(10, 0, 0, 1), 5);
    assert_eq!(
        rdata_address(&r),
        Some(std::net::IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
    );
    let ptr = ptr_record("x", "y", 5);
    assert_eq!(rdata_address(&ptr), None);
}
