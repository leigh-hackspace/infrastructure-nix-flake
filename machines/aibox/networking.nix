{ config, pkgs, ... }:

{
  boot.kernel.sysctl = {
    # enable IPv4 and IPv6 forwarding on all interfaces
    "net.ipv4.conf.all.forwarding" = true;
    "net.ipv6.conf.all.forwarding" = true;

    "net.ipv4.conf.all.arp_filter" = 1;
    "net.ipv4.conf.default.arp_filter" = 1;
  };

  networking = {
    useDHCP = false;
    firewall.enable = false;
    networkmanager.enable = false;
    nftables.enable = true;
  };

  systemd.network = {
    enable = true;
    wait-online.enable = false;

    links = {
      "10-lan" = {
        matchConfig = {
          MACAddress = "84:47:09:40:d2:80";
        };
        linkConfig = {
          Name = "lan";
        };
      };
    };

    netdevs = {
      "11-vlan225" = {
        netdevConfig = {
          Kind = "vlan";
          Name = "vlan225";
          MACAddress = "84:47:09:40:d2:25";
        };
        vlanConfig.Id = 225;
      };

      "12-vlan227" = {
        netdevConfig = {
          Kind = "vlan";
          Name = "vlan227";
          MACAddress = "84:47:09:40:d2:27";
        };
        vlanConfig.Id = 227;
      };
    };

    networks = {
      "10-lan" = {
        matchConfig.Name = "lan";
        linkConfig.RequiredForOnline = "yes";
        networkConfig = {
          DHCP = false;
        };
        vlan = [
          "vlan225"
          "vlan227"
        ];
      };

      "11-vlan225" = {
        matchConfig.Name = "vlan225";
        networkConfig = {
          DHCP = true;
          IPv6AcceptRA = true;
        };
      };

      "12-vlan227" = {
        matchConfig.Name = "vlan227";
        networkConfig = {
          DHCP = true;
          IPv6AcceptRA = true;
        };
      };
    };
  };

  networking.nftables.ruleset = ''
    table inet firewall {
      chain rpfilter {
        type filter hook prerouting priority mangle + 10; policy drop;
        meta nfproto ipv4 udp sport . udp dport { 68 . 67, 67 . 68 } accept comment "DHCPv4 client/server"
        fib saddr . mark oif exists accept
      }

      chain input {
        type filter hook input priority filter; policy drop;

        # assuming we trust our LAN clients
        iifname { "lo", "podman0" } accept comment "trusted interfaces"

        # handle packets according to connection state
        ct state vmap { invalid : drop, established : accept, related : accept, new : jump input-allow, untracked : jump input-allow }

        # if we make it here, block and log
        tcp flags syn / fin,syn,rst,ack log prefix "refused connection: " level info
      }

      chain input-allow {
        # Log NEW SSH connections (both accepted and rejected)
        ip saddr 10.3.0.0/16 tcp dport 22 log prefix "SSH accepted: " accept  comment "ssh from Local Networks"
        tcp dport 22                      log prefix "SSH rejected: " drop    comment "ssh from outside allowed range"

        tcp dport 80    accept comment "http  from anywhere"
        tcp dport 443   accept comment "https from anywhere"

        tcp dport 4011  accept comment "pixiecore API  from anywhere"
        udp dport 67    accept comment "pixiecore DHCP from anywhere"
        udp dport 69    accept comment "pixiecore TFTP from anywhere"

        ## These are Podman so there don't apply here (use forward instead)
        tcp dport 8080  log prefix "8080 accepted: " accept comment "httpx from anywhere"
        tcp dport 8081  log prefix "8081 accepted: " accept comment "httpx from anywhere"
        tcp dport 8082  log prefix "8082 accepted: " accept comment "httpx from anywhere"
        tcp dport 7860  log prefix "7860 accepted: " accept comment "httpx from anywhere"

        # Allow various discovery protocols
        udp dport 137   accept comment "NetBIOS Name Service"
        udp dport 138   accept comment "NetBIOS Datagram Service"
        udp dport 1900  accept comment "Simple Service Discovery Protocol (SSDP)"
        udp dport 5353  accept comment "mDNS/Avahi"
        udp dport 5355  accept comment "Link-Local Multicast Name Resolution (LLMNR)"
        udp dport 10001 accept comment "Unifi discovery service"

        # Rate limit ping (replace your existing icmp rule)
        icmp type echo-request limit rate 10/second accept comment "allow ping (rate limited)"

        icmpv6 type != { nd-redirect, 139 } accept comment "Accept all ICMPv6 messages except redirects and node information queries (type 139). See RFC 4890, section 4.4."
        ip6 daddr fe80::/64 udp dport 546   accept comment "DHCPv6 client"

        # DHCPv6
        udp dport dhcpv6-client udp sport dhcpv6-server counter accept comment "IPv6 DHCP"

        # Log all NEW connections that didn't match any rule above
        log prefix "rejected NEW: " drop
      }

      chain forward {
        # type filter hook forward priority 0; policy drop;
        type filter hook forward priority filter + 10; policy drop;

        # # Allow all podman network traffic
        # ip saddr 10.88.0.0/16 log prefix "PODMAN OUT " accept comment "allow from podman network"
        # ip daddr 10.88.0.0/16 log prefix "PODMAN IN  " accept comment "allow to   podman network"

        # Handle packets according to connection state
        ct state vmap { invalid : drop, established : accept, related : accept, new : jump forward-allow }
      }

      chain forward-allow {
        # Log and allow traffic to podman containers
        ip daddr 10.88.0.0/16 tcp dport 8080 log prefix "podman 8080 NEW: " accept
        ip daddr 10.88.0.0/16 tcp dport 8081 log prefix "podman 8081 NEW: " accept

        # Allow all other podman network traffic
        ip saddr 10.88.0.0/16 accept comment "allow from podman network"
        ip daddr 10.88.0.0/16 accept comment "allow to   podman network"

        # Log rejected NEW forwarded connections
        log prefix "rejected forward NEW: " drop
      }
    }

    table ip nat {
      chain pre {
        type nat hook prerouting priority dstnat; policy accept;
        # we'll add rules for our 1:1 NAT here later
      }

      chain post {
        type nat hook postrouting priority srcnat; policy accept;
      }

      chain out {
        type nat hook output priority mangle; policy accept;
        # we'll add rules for our 1:1 NAT here later
      }
    }
  '';

  # This ensures switching configuration doesn't break anything. (NEEDS TWEAKING)
  system.activationScripts.fixPodman = ''
    ${pkgs.podman}/bin/podman network reload --all || true
  '';

  # Apply Podman's own rules after `nftables` starts because `nftables` will always start afresh.
  systemd.services.nftables.postStart = ''
    ${pkgs.podman}/bin/podman network reload --all || true
  '';
}
