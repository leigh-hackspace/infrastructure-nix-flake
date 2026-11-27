//! Config tests: the fixture the DHCP tests also use, plus the findings a
//! generator is most likely to produce.

use super::*;

const FIXTURE: &str = include_str!("../../tests/fixtures/minimal.json");

fn cfg() -> Config {
    Config::parse(FIXTURE).expect("the fixture parses")
}

/// Replace one fragment, so each test says exactly what it changed.
fn with(from: &str, to: &str) -> String {
    assert!(
        FIXTURE.contains(from),
        "fixture no longer contains {from:?}"
    );
    FIXTURE.replace(from, to)
}

#[test]
fn the_fixture_parses_and_derives_the_pool() {
    let c = cfg();
    assert_eq!(c.zone, "int.example.org");
    let p = c.pool_for("vlan225").expect("one pool");
    assert_eq!(p.net.cidr(), "10.0.0.0/24");
    assert_eq!(p.net.broadcast(), std::net::Ipv4Addr::new(10, 0, 0, 255));
    assert!(c.in_zone("grafana.int.example.org."));
    assert!(!c.in_zone("example.com"));
}

#[test]
fn the_fixture_is_accepted_by_the_daemon_itself() {
    // `netd check` is the build gate; this is the same call it makes.
    Config::load(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/minimal.json"
    )))
    .expect("the checked-in fixture is valid");
}

#[test]
fn an_unknown_key_is_refused_with_its_path() {
    let e = Config::parse(&with("\"forwarders\"", "\"forwarder\"")).unwrap_err();
    assert!(e.contains("config.dns"), "{e}");
    assert!(e.contains("forwarder"), "{e}");
    assert!(e.contains("known keys"), "{e}");
}

#[test]
fn a_reservation_inside_the_dynamic_range_is_refused() {
    let e = Config::parse(&with(
        "\"address\": \"10.0.0.20\"",
        "\"address\": \"10.0.0.200\"",
    ))
    .unwrap_err();
    assert!(e.contains("inside the dynamic range"), "{e}");
}

#[test]
fn two_devices_claiming_one_mac_is_refused() {
    let text = with(
        r#""reservations": [
      {
        "name": "services1",
        "address": "10.0.0.20",
        "macs": ["c8:d3:ff:a5:b2:25"],
        "client_ids": ["00:02:ab:cd"]
      }
    ],"#,
        r#""reservations": [
      {
        "name": "services1",
        "address": "10.0.0.20",
        "macs": ["c8:d3:ff:a5:b2:25"],
        "client_ids": []
      },
      {
        "name": "impostor",
        "address": "10.0.0.21",
        "macs": ["c8:d3:ff:a5:b2:25"],
        "client_ids": []
      }
    ],"#,
    );
    let e = Config::parse(&text).unwrap_err();
    assert!(e.contains("claimed by both"), "{e}");
}

#[test]
fn a_reservation_with_no_identifier_is_refused() {
    let text = with(r#""macs": ["c8:d3:ff:a5:b2:25"],"#, r#""macs": [],"#)
        .replace(r#""client_ids": ["00:02:ab:cd"]"#, r#""client_ids": []"#);
    let e = Config::parse(&text).unwrap_err();
    assert!(e.contains("matches nothing"), "{e}");
}

#[test]
fn an_option_tag_typo_is_refused() {
    let e = Config::parse(&with(
        r#"{ "tag": "vlan225", "code": 6"#,
        r#"{ "tag": "vlan 225", "code": 6"#,
    ))
    .unwrap_err();
    assert!(e.contains("matches nothing"), "{e}");
}

#[test]
fn a_non_64_ra_prefix_is_refused() {
    let e = Config::parse(&with("2001:db8:225::/64", "2001:db8:225::/48")).unwrap_err();
    assert!(e.contains("only /64"), "{e}");
}

#[test]
fn a_prefix_with_host_bits_set_is_refused() {
    let e = Config::parse(&with("2001:db8:225::/64", "2001:db8:225::1000/64")).unwrap_err();
    assert!(e.contains("not a network address"), "{e}");
}

#[test]
fn a_router_lifetime_shorter_than_the_advertisement_interval_is_refused() {
    let e = Config::parse(&with(
        r#""router_lifetime": 1200"#,
        r#""router_lifetime": 60"#,
    ))
    .unwrap_err();
    assert!(e.contains("under three advertisement intervals"), "{e}");
}

#[test]
fn an_empty_acl_is_a_finding_because_it_refuses_the_router_itself() {
    let text = with(
        r#""acl": ["127.0.0.0/8", "10.0.0.0/24", "fd00::/8"],"#,
        r#""acl": [],"#,
    );
    let e = Config::parse(&text).unwrap_err();
    assert!(e.contains("every query would be refused"), "{e}");
}

#[test]
fn a_name_outside_the_zone_is_refused() {
    let e = Config::parse(&with(
        r#""name": "services1.int.example.org""#,
        r#""name": "services1.example.com""#,
    ))
    .unwrap_err();
    assert!(e.contains("outside the zone"), "{e}");
}

#[test]
fn a_conditional_forwarder_inside_the_zone_is_refused() {
    let e = Config::parse(&with(
        r#""zone": "ts.example.org", "servers": ["100.100.100.100"]"#,
        r#""zone": "int.example.org", "servers": ["100.100.100.100"]"#,
    ))
    .unwrap_err();
    assert!(e.contains("never forward it"), "{e}");
}

#[test]
fn a_pool_outside_its_own_network_is_refused() {
    let e = Config::parse(&with(r#""to": "10.0.0.253""#, r#""to": "10.9.0.253""#)).unwrap_err();
    assert!(e.contains("is not inside"), "{e}");
}

#[test]
fn a_non_contiguous_netmask_is_refused() {
    let e = Config::parse(&with(
        r#""netmask": "255.255.255.0""#,
        r#""netmask": "255.0.255.0""#,
    ))
    .unwrap_err();
    assert!(e.contains("not a contiguous mask"), "{e}");
}

#[test]
fn every_finding_is_collected_rather_than_the_first() {
    // Two independent mistakes, both named.
    let text = with(r#""address": "10.0.0.20""#, r#""address": "10.0.0.200""#).replace(
        r#""tag": "vlan225", "code": 6"#,
        r#""tag": "vlan 225", "code": 6"#,
    );
    let e = Config::parse(&text).unwrap_err();
    assert!(e.contains("inside the dynamic range"), "{e}");
    assert!(e.contains("matches nothing"), "{e}");
    assert!(e.lines().count() >= 2, "{e}");
}

#[test]
fn a_client_id_beats_a_mac_when_both_are_present() {
    let c = cfg();
    let mac = mac_parse("c8:d3:ff:a5:b2:25").unwrap();
    assert_eq!(c.reservation_for(&mac, None).unwrap().name, "services1");
    let cid = client_id_parse("00:02:ab:cd").unwrap();
    let other = mac_parse("02:00:00:00:00:01").unwrap();
    assert_eq!(
        c.reservation_for(&other, Some(&cid)).unwrap().name,
        "services1"
    );
    assert!(c.reservation_for(&other, None).is_none());
}

#[test]
fn names_are_canonicalised_and_reversible() {
    assert_eq!(
        canonical_name("Services1.INT.Example.ORG."),
        "services1.int.example.org"
    );
    let c = cfg();
    let ip: std::net::IpAddr = "10.0.0.20".parse().unwrap();
    assert!(
        c.dns
            .records
            .iter()
            .any(|r| r.name == "services1.int.example.org" && r.addresses.contains(&ip)),
        "the table carries both the FQDN and the bare label for one device"
    );
    assert!(!c.in_zone("example.com"));
}

#[test]
fn hex_and_client_ids_round_trip() {
    assert_eq!(
        hex_parse("01:04:0A:03:01:14").unwrap(),
        vec![1, 4, 10, 3, 1, 20]
    );
    assert!(hex_parse("0").is_err());
    assert_eq!(client_id_parse("00:02").unwrap(), vec![0, 2]);
    assert_eq!(client_id_hex(&vec![0, 2, 0xab]), "00:02:ab");
}

#[test]
fn a_non_contiguous_mask_cannot_become_a_network() {
    assert!(V4Net::from_mask(
        std::net::Ipv4Addr::new(10, 0, 0, 1),
        std::net::Ipv4Addr::new(255, 0, 255, 0)
    )
    .is_none());
    assert_eq!(
        V4Net::from_mask(
            std::net::Ipv4Addr::new(10, 0, 0, 1),
            std::net::Ipv4Addr::new(255, 255, 255, 0)
        )
        .unwrap()
        .cidr(),
        "10.0.0.0/24"
    );
}
