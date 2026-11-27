//! What a DHCP server must get right, asked without a socket.

use super::*;
use crate::config::{mac_parse, Config};
use crate::lease::{Lease, LeaseDb};
use std::net::Ipv4Addr;

const CFG: &str = include_str!("../../tests/fixtures/minimal.json");

fn cfg() -> Config {
    Config::parse(CFG).expect("fixture parses")
}

fn ip(s: &str) -> Ipv4Addr {
    s.parse().expect("a test address parses")
}

/// Build a request the way a client would: header, cookie, options.
fn request(msg_type: u8, mac: &str, extra: &[(u8, Vec<u8>)]) -> Message {
    let mut r = Message {
        op: BOOTREQUEST,
        htype: 1,
        hlen: 6,
        hops: 0,
        xid: 0xdead_beef,
        secs: 0,
        flags: 0x8000,
        ciaddr: Ipv4Addr::UNSPECIFIED,
        yiaddr: Ipv4Addr::UNSPECIFIED,
        siaddr: Ipv4Addr::UNSPECIFIED,
        giaddr: Ipv4Addr::UNSPECIFIED,
        chaddr: [0; 16],
        options: vec![(OPT_MSG_TYPE, vec![msg_type])],
    };
    let m = mac_parse(mac).unwrap();
    r.chaddr[..6].copy_from_slice(&m);
    r.options.extend(extra.iter().cloned());
    r
}

fn client(req: &Message) -> Client {
    Client::read(req, req.msg_type().unwrap()).unwrap()
}

fn lease(ip_str: &str, mac: &str, expires: i64) -> Lease {
    Lease {
        ip: ip(ip_str),
        mac: mac_parse(mac).unwrap(),
        client_id: None,
        hostname: None,
        interface: "vlan225".into(),
        tag: "vlan225".into(),
        issued: expires - 86_400,
        expires,
        reserved: false,
    }
}

#[test]
fn a_discover_gets_an_offer_from_the_pool() {
    let c = cfg();
    let db = LeaseDb::new();
    let req = request(MSG_DISCOVER, "02:00:00:00:00:01", &[]);
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::Offer);
    assert_eq!(a.ip, Some(ip("10.0.0.150")), "the first free address");
    assert!(a.reason.contains("first free"), "{}", a.reason);
    assert_eq!(a.ttl, 86_400);
}

#[test]
fn a_reservation_beats_the_pool() {
    let c = cfg();
    let db = LeaseDb::new();
    let req = request(MSG_DISCOVER, "c8:d3:ff:a5:b2:25", &[]);
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::Offer);
    assert_eq!(a.ip, Some(ip("10.0.0.20")));
    assert!(a.reserved);
    assert!(a.reason.contains("fixed reservation"), "{}", a.reason);
}

#[test]
fn a_request_for_the_reserved_address_is_acknowledged() {
    let c = cfg();
    let db = LeaseDb::new();
    let req = request(
        MSG_REQUEST,
        "c8:d3:ff:a5:b2:25",
        &[
            (OPT_REQUESTED_IP, enc_ip(ip("10.0.0.20"))),
            (OPT_SERVER_ID, enc_ip(ip("10.0.0.1"))),
        ],
    );
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::Ack);
    assert_eq!(a.ip, Some(ip("10.0.0.20")));
}

#[test]
fn a_request_for_someone_elses_address_is_naked() {
    let c = cfg();
    let mut db = LeaseDb::new();
    db.upsert(lease("10.0.0.150", "02:00:00:00:00:99", 10_000));
    let req = request(
        MSG_REQUEST,
        "02:00:00:00:00:01",
        &[(OPT_REQUESTED_IP, enc_ip(ip("10.0.0.150")))],
    );
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::Nak);
    assert!(a.reason.contains("held by"), "{}", a.reason);
}

#[test]
fn a_request_for_an_address_outside_the_pool_is_naked() {
    let c = cfg();
    let db = LeaseDb::new();
    let req = request(
        MSG_REQUEST,
        "02:00:00:00:00:01",
        &[(OPT_REQUESTED_IP, enc_ip(ip("10.0.0.30")))],
    );
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::Nak);
    assert!(a.reason.contains("not a usable address"), "{}", a.reason);
}

#[test]
fn a_request_naming_another_server_is_silent_not_naked() {
    let c = cfg();
    let db = LeaseDb::new();
    let req = request(
        MSG_REQUEST,
        "02:00:00:00:00:01",
        &[
            (OPT_REQUESTED_IP, enc_ip(ip("10.0.0.160"))),
            (OPT_SERVER_ID, enc_ip(ip("10.0.0.9"))),
        ],
    );
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::Ignore, "{}", a.reason);
    assert!(a.reason.contains("picked another offer"), "{}", a.reason);
}

#[test]
fn a_lease_is_kept_while_it_is_alive() {
    let c = cfg();
    let mut db = LeaseDb::new();
    db.upsert(lease("10.0.0.178", "02:00:00:00:00:01", 10_000));
    let req = request(MSG_DISCOVER, "02:00:00:00:00:01", &[]);
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.ip, Some(ip("10.0.0.178")));
    assert!(a.reason.contains("own unexpired lease"), "{}", a.reason);
}

#[test]
fn a_lapsed_lease_gives_the_address_back_to_the_pool() {
    let c = cfg();
    let mut db = LeaseDb::new();
    db.upsert(lease("10.0.0.178", "02:00:00:00:00:01", 1_000));
    let req = request(MSG_DISCOVER, "02:00:00:00:00:01", &[]);
    let a = decide(&c, &db, "vlan225", &client(&req), 5_000);
    assert_eq!(a.ip, Some(ip("10.0.0.150")), "the pool's first free again");
}

#[test]
fn a_declined_address_is_skipped() {
    let c = cfg();
    let mut db = LeaseDb::new();
    db.decline(ip("10.0.0.150"), 10_000);
    let req = request(MSG_DISCOVER, "02:00:00:00:00:01", &[]);
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.ip, Some(ip("10.0.0.151")));
}

#[test]
fn an_ignored_address_inside_the_pool_is_never_offered() {
    let c = cfg();
    let mut db = LeaseDb::new();
    for i in 150..166u8 {
        db.upsert(lease(
            &format!("10.0.0.{i}"),
            &format!("02:00:00:00:01:{i:02x}"),
            10_000,
        ));
    }
    let req = request(MSG_DISCOVER, "02:00:00:00:00:01", &[]);
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(
        a.ip,
        Some(ip("10.0.0.167")),
        "the ignored .166 is stepped over"
    );
}

#[test]
fn a_reserved_address_is_never_offered_to_anyone_else() {
    let c = cfg();
    // Fill the pool from .150 to .253 except the reservation (.20 is outside
    // the range anyway) — then check the exhausted answer is honest rather than
    // an offer of somebody's reservation.
    let mut db = LeaseDb::new();
    for i in 150..254u8 {
        db.upsert(lease(
            &format!("10.0.0.{i}"),
            &format!("02:00:00:00:02:{i:02x}"),
            10_000,
        ));
    }
    let req = request(MSG_DISCOVER, "02:00:00:00:00:01", &[]);
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::Ignore);
    assert!(a.reason.contains("exhausted"), "{}", a.reason);
}

#[test]
fn wpad_is_ignored() {
    let c = cfg();
    let db = LeaseDb::new();
    let req = request(
        MSG_DISCOVER,
        "02:00:00:00:00:01",
        &[(OPT_HOSTNAME, b"wpad".to_vec())],
    );
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::Ignore);
    assert!(a.reason.contains("ignored_names"), "{}", a.reason);
}

#[test]
fn an_interface_with_no_pool_hands_out_nothing() {
    let c = cfg();
    let db = LeaseDb::new();
    let req = request(MSG_DISCOVER, "02:00:00:00:00:01", &[]);
    let a = decide(&c, &db, "vlan226", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::Ignore);
    assert!(a.reason.contains("no dynamic pool"), "{}", a.reason);
}

#[test]
fn a_relayed_request_is_ignored() {
    let c = cfg();
    let db = LeaseDb::new();
    let mut req = request(MSG_DISCOVER, "02:00:00:00:00:01", &[]);
    req.giaddr = ip("10.3.1.1");
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::Ignore);
    assert!(a.reason.contains("relayed"), "{}", a.reason);
}

#[test]
fn a_release_and_a_decline_are_effects_not_offers() {
    let c = cfg();
    let db = LeaseDb::new();
    let mut rel = request(MSG_RELEASE, "02:00:00:00:00:01", &[]);
    rel.ciaddr = ip("10.0.0.150");
    let a = decide(&c, &db, "vlan225", &client(&rel), 1_000);
    assert_eq!(a.kind, Kind::Ignore);
    assert_eq!(a.effect, Effect::Release);
    assert_eq!(
        a.ip,
        Some(ip("10.0.0.150")),
        "the address travels with the effect: the caller has to forget this row"
    );

    let dec = request(
        MSG_DECLINE,
        "02:00:00:00:00:01",
        &[(OPT_REQUESTED_IP, enc_ip(ip("10.0.0.151")))],
    );
    let a = decide(&c, &db, "vlan225", &client(&dec), 1_000);
    assert_eq!(a.effect, Effect::Decline);
    assert_eq!(a.ip, Some(ip("10.0.0.151")));
}

#[test]
fn an_inform_gets_options_and_no_address() {
    let c = cfg();
    let db = LeaseDb::new();
    let mut req = request(MSG_INFORM, "02:00:00:00:00:01", &[]);
    req.ciaddr = ip("10.0.0.42");
    let a = decide(&c, &db, "vlan225", &client(&req), 1_000);
    assert_eq!(a.kind, Kind::InformAck);
    assert_eq!(a.ip, None);
    assert!(lease_from(&a, &client(&req), "vlan225", 1_000).is_none());
}

#[test]
fn only_an_ack_writes_a_lease() {
    let c = cfg();
    let db = LeaseDb::new();
    let x = client(&request(MSG_DISCOVER, "02:00:00:00:00:01", &[]));
    let offer = decide(&c, &db, "vlan225", &x, 1_000);
    assert!(
        lease_from(&offer, &x, "vlan225", 1_000).is_none(),
        "an offer is not a promise"
    );
    let req = request(
        MSG_REQUEST,
        "02:00:00:00:00:01",
        &[(OPT_REQUESTED_IP, enc_ip(ip("10.0.0.150")))],
    );
    let x = client(&req);
    let ack = decide(&c, &db, "vlan225", &x, 1_000);
    let l = lease_from(&ack, &x, "vlan225", 1_000).expect("an ACK is a promise");
    assert_eq!(l.ip, ip("10.0.0.150"));
    assert_eq!(l.expires, 1_000 + 86_400);
}

#[test]
fn the_netboot_menu_picks_the_variant_the_client_asked_for() {
    let text = CFG.replace(
        r#""netboot": []"#,
        r#""netboot": [
          { "tag": "netboot-legacy", "match_option": 93, "match_values": [0],
            "when_tags": ["vlan225"], "boot": "/etc/tftp/undionly.kpxe",
            "server": "10.0.0.32" },
          { "tag": "netboot-efi", "match_option": 93, "match_values": [7, 8, 9],
            "when_tags": ["vlan225"], "boot": "/etc/tftp/ipxe.efi",
            "server": "10.0.0.32" },
          { "tag": "netboot-autoexec", "match_option": 77, "match_contains": "iPXE",
            "when_tags": ["vlan225"], "boot": "http://10.0.0.32/boot/netboot.ipxe",
            "server": "10.0.0.32" }
        ]"#,
    );
    let c = Config::parse(&text).unwrap();
    let db = LeaseDb::new();
    let efi = request(
        MSG_DISCOVER,
        "02:00:00:00:00:01",
        &[(OPT_CLIENT_ARCH, 7u16.to_be_bytes().to_vec())],
    );
    let a = decide(&c, &db, "vlan225", &client(&efi), 1_000);
    assert_eq!(
        a.netboot.as_ref().map(|n| n.tag.as_str()),
        Some("netboot-efi")
    );
    assert!(a.tags.contains("netboot-efi"));

    let ipxe = request(
        MSG_DISCOVER,
        "02:00:00:00:00:02",
        &[
            (OPT_CLIENT_ARCH, 7u16.to_be_bytes().to_vec()),
            (OPT_USER_CLASS, b"iPXE".to_vec()),
        ],
    );
    let a = decide(&c, &db, "vlan225", &client(&ipxe), 1_000);
    assert_eq!(
        a.netboot.as_ref().map(|n| n.tag.as_str()),
        Some("netboot-autoexec"),
        "the last match wins, so iPXE gets its script rather than another loader"
    );

    // The same request on an interface the variant is not offered on.
    let elsewhere = request(
        MSG_DISCOVER,
        "02:00:00:00:00:03",
        &[(OPT_CLIENT_ARCH, 7u16.to_be_bytes().to_vec())],
    );
    let a = decide(&c, &db, "vlan226", &client(&elsewhere), 1_000);
    assert!(a.netboot.is_none());
}

#[test]
fn reply_options_carry_the_defaults_then_the_config_overrides() {
    let c = cfg();
    let tags = c.tags_for("vlan225");
    let pool = c.pool_for("vlan225").unwrap();
    let opts = reply_options(&c, Some(pool), &tags, 86_400, None, Kind::Offer);
    let get = |code: u8| {
        opts.iter()
            .find(|(c, _)| *c == code)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(get(OPT_SUBNET_MASK).unwrap(), vec![255, 255, 255, 0]);
    assert_eq!(get(OPT_ROUTER).unwrap(), vec![10, 0, 0, 1]);
    assert_eq!(get(OPT_DNS).unwrap(), vec![10, 0, 0, 1]);
    assert_eq!(get(OPT_DOMAIN).unwrap(), b"int.example.org".to_vec());
    assert_eq!(get(OPT_BROADCAST).unwrap(), vec![10, 0, 0, 255]);
    // The vendor option from the config, and exactly once.
    assert_eq!(get(43).unwrap(), vec![1, 4, 192, 168, 1, 20]);
    assert_eq!(opts.iter().filter(|(c, _)| *c == 43).count(), 1);
}

#[test]
fn a_nak_carries_no_lease_options() {
    let c = cfg();
    let tags = c.tags_for("vlan225");
    let pool = c.pool_for("vlan225").unwrap();
    let opts = reply_options(&c, Some(pool), &tags, 86_400, None, Kind::Nak);
    let codes: Vec<u8> = opts.iter().map(|(c, _)| *c).collect();
    assert!(codes.contains(&OPT_MSG_TYPE));
    assert!(!codes.contains(&OPT_LEASE_TIME));
    assert!(!codes.contains(&OPT_ROUTER));
}

#[test]
fn a_reply_round_trips_through_the_parser() {
    let req = request(MSG_DISCOVER, "02:00:00:00:00:01", &[]);
    let mut r = Reply::new(&req);
    r.yiaddr = ip("10.0.0.150");
    r.siaddr = ip("10.0.0.32");
    r.file_name = Some("/etc/tftp/ipxe.efi".into());
    r.options = vec![
        (OPT_MSG_TYPE, vec![MSG_OFFER]),
        (OPT_DNS, vec![10, 0, 0, 1]),
    ];
    let bytes = r.encode();
    assert!(bytes.len() >= 300);
    let back = Message::parse(&bytes).expect("our own reply parses");
    assert_eq!(back.op, BOOTREPLY);
    assert_eq!(back.yiaddr, r.yiaddr);
    assert_eq!(back.siaddr, r.siaddr);
    assert_eq!(&back.chaddr[..6], &req.chaddr[..6]);
    assert_eq!(back.msg_type(), Some(MSG_OFFER));
    assert_eq!(back.opt_ip(OPT_DNS), Some(ip("10.0.0.1")));
    assert_eq!(back.xid, req.xid);
}

#[test]
fn option_overload_is_read_from_the_file_field() {
    // A client with a long option list continues it in `file` (option 52 says
    // so). Reading only the options area would lose its client-id and answer
    // for the wrong device.
    let mut bytes = {
        let req = request(
            MSG_DISCOVER,
            "02:00:00:00:00:01",
            &[(OPT_OVERLOAD, vec![2])],
        );
        let mut r = Reply::new(&req);
        r.options = vec![(OPT_MSG_TYPE, vec![MSG_DISCOVER]), (OPT_OVERLOAD, vec![2])];
        r.encode()
    };
    // The `file` region of a BOOTP header is bytes 108..236.
    bytes[108] = OPT_CLIENT_ID;
    bytes[109] = 3;
    bytes[110] = 0;
    bytes[111] = 2;
    bytes[112] = 0xab;
    bytes[113] = OPT_END;
    let parsed = Message::parse(&bytes).expect("overloaded message parses");
    assert_eq!(parsed.opt(OPT_CLIENT_ID), Some(&[0u8, 2, 0xab][..]));
}

#[test]
fn a_truncated_option_is_an_error_not_a_guess() {
    let mut bytes = {
        let req = request(MSG_DISCOVER, "02:00:00:00:00:01", &[]);
        let mut r = Reply::new(&req);
        r.options = vec![(OPT_MSG_TYPE, vec![MSG_DISCOVER])];
        r.encode()
    };
    let area = 240;
    bytes.truncate(area + 2);
    bytes[area] = OPT_CLIENT_ID;
    bytes[area + 1] = 9;
    let e = Message::parse(&bytes).unwrap_err();
    assert!(e.contains("truncated") || e.contains("ends first"), "{e}");
}

#[test]
fn a_short_packet_is_refused() {
    assert!(Message::parse(&[1, 1, 6, 0]).is_err());
}

#[test]
fn a_hostname_is_sanitised_into_one_dns_label() {
    assert_eq!(sanitise_hostname("Daves Laptop"), "daves-laptop");
    assert_eq!(sanitise_hostname("host.example.com"), "host");
    assert_eq!(sanitise_hostname("---"), "");
    assert_eq!(sanitise_hostname("ok_name"), "ok_name");
}

#[test]
fn the_domain_search_option_is_encoded_as_rfc3397_labels() {
    assert_eq!(
        enc_domain_search(&["int.example.org".into(), "ts.example.org".into()]),
        vec![
            3, b'i', b'n', b't', 7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'o', b'r', b'g',
            0, 2, b't', b's', 7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'o', b'r', b'g', 0
        ]
    );
}

#[test]
fn describe_request_names_every_option_that_matters() {
    let req = request(
        MSG_REQUEST,
        "02:00:00:00:00:01",
        &[
            (OPT_REQUESTED_IP, enc_ip(ip("10.0.0.150"))),
            (OPT_HOSTNAME, b"laptop".to_vec()),
        ],
    );
    let rows = describe_request(&req);
    let has = |k: &str, v: &str| rows.iter().any(|(a, b)| a.contains(k) && b == v);
    assert!(has("message-type", "REQUEST"), "{rows:?}");
    assert!(has("requested-ip", "10.0.0.150"), "{rows:?}");
    assert!(has("hostname", "laptop"), "{rows:?}");
}
