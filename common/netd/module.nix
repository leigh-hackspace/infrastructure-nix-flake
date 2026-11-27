# MODULE — `services.netd`: the network's DHCPv4 server, IPv6 router advertiser
# and resolver, as one unit with one generated config file.
#
# The daemon is `crates/netd`; its configuration is one JSON document, rendered
# by `common/netd/mk-config.nix` from the tables the rest of the network is built
# from. This module owns only the parts that are NixOS: where the file lives, who
# may read the state directory, which capabilities the sockets need, and the two
# conflicts that must not be discovered at 03:00.
#
# ## Why the config is validated by building it
#
# `netd check` runs over the rendered document in a `runCommand`, so the file
# under `/etc` is the *output* of a successful check rather than an input to a
# unit that might start and behave oddly. A schema error — a tag that matches no
# interface, a reservation inside a pool, a typo'd key — therefore fails the
# build with the daemon's own sentence, which is the "no friction" half of the
# design: the Nix side writes the document, and the daemon is the only thing that
# decides whether it makes sense.
#
# That check needs the binary, so it builds the daemon. Deliberate: a config
# nobody validated is a router that answers nothing.
#
# ## The two conflicts
#
# Both are assertions rather than documentation, because both are silent at
# runtime:
#
#   * `services.dnsmasq.enable` — two DHCP servers on port 67 and two resolvers
#     on 53. Whichever binds first wins each socket, and the other starts and
#     fails forever or answers half the network. The cutover is one line on each
#     side, in the same commit, and this makes forgetting one a build failure.
#   * `services.resolved.enable` — systemd-resolved's stub takes 127.0.0.53:53,
#     and dnsmasq could not bind port 53 beside it either
#     (`machines/gw/dns.nix` carries the measurement). The module does not turn
#     resolved off for you: on a box whose resolver is netd, resolved is a second
#     resolver with its own cache, and that is a decision, not a default.
{
  config,
  lib,
  pkgs,
  CRANE,
  INFRA,
  ...
}: let
  cfg = config.services.netd;

  netd =
    if cfg.package != null
    then cfg.package
    else
      CRANE.crate {
        pname = "netd";
        version = "0.1.0";
        member = "netd";
        # The two shared crates its manifest reads: `json` for the config and the
        # API bodies, `web` for the API's request/response plumbing.
        sharedCrates = ["json" "web"];
      };

  format = pkgs.formats.json {};

  # The rendered document, as a store path its own derivation depends on. Written
  # by `pkgs.formats.json`'s generator rather than by hand, so what reaches the
  # daemon is exactly what `builtins.toJSON` makes of the Nix value.
  configFile = format.generate "netd-config.json" cfg.config;

  # What the unit is actually handed: the rendered document, but only if the
  # daemon agrees with it. `netd check`'s summary is kept in the build log and
  # printed on failure — a schema error is a sentence naming the field, which is
  # the whole point of validating here rather than at `ExecStart`.
  checkedConfig =
    pkgs.runCommand "netd-config-checked.json" {
      nativeBuildInputs = [netd];
    } ''
      if ! netd check --config ${configFile} > check.log 2>&1; then
        echo "netd refuses this configuration (services.netd.config):" >&2
        cat check.log >&2
        exit 1
      fi
      cp ${configFile} $out
    '';
in {
  options.services.netd = {
    enable = lib.mkEnableOption ''
      netd: the network's DHCPv4 server, IPv6 router advertiser and DNS
      resolver. Turning this on is the cutover, and it requires
      `services.dnsmasq.enable = false` in the same change — see
      docs/netd-2026-11-26.md
    '';

    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      description = ''
        The netd binary. `null` builds it from this flake (`crates/netd`), which
        is what every machine here does; the option exists so a box outside this
        flake can point at its own build.
      '';
    };

    config = lib.mkOption {
      type = format.type;
      default = {};
      description = ''
        The daemon's whole configuration, as a Nix value written to JSON.
        `common/netd/mk-config.nix` builds it from the network's tables; the
        schema is `crates/netd/src/config/` and is strict — an unknown key is a
        build failure, not a warning.
      '';
    };

    allowUnroutableApiBind = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Permit `config.api.bind` to be a wildcard address. The API is
        unauthenticated — its access control is the front door's LAN ACL — so
        binding it to `0.0.0.0` would publish the lease table on every interface
        the box has, including the uplink. Only for a box behind something that
        enforces the ACL itself.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = !config.services.dnsmasq.enable;
        message = ''
          services.netd and services.dnsmasq cannot both be enabled: two DHCP servers on port 67 and two resolvers on port 53, and which one answers each socket is a race rather than a decision. The cutover is `services.dnsmasq.enable = false; services.netd.enable = true;` in the same commit (docs/netd-2026-11-26.md).
        '';
      }
      {
        assertion = !config.services.resolved.enable;
        message = ''
          services.netd answers DNS on 0.0.0.0:53, and systemd-resolved's stub occupies 127.0.0.53:53. Two resolvers on one box is a second cache with its own behaviour on the one box whose answer has to be the network's — `machines/gw/dns.nix` has the measurement from the 2026-10-17 first boot.
        '';
      }
      {
        assertion =
          cfg.allowUnroutableApiBind
          || !(builtins.elem (cfg.config.api.bind or "") ["0.0.0.0" "::"]);
        message = ''
          services.netd.config.api.bind is a wildcard address (${toString (cfg.config.api.bind or "")}). The API is unauthenticated and the front door's LAN ACL is its access control, so this would publish the lease table on the uplink too. Bind the LAN address (`common/hosts.nix`'s `router`), or set `allowUnroutableApiBind = true` if something else enforces the ACL.
        '';
      }
      {
        # The daemon is the one thing on this box that cannot be allowed to exit:
        # it hands out addresses and answers every name. A config that is an empty
        # attrset is what `enable = true` with no `config` looks like, and the
        # daemon would refuse it at build time anyway — this says so in one
        # sentence instead of inside a JSON parse error.
        assertion = (cfg.config.dns.listen or []) != [];
        message = ''
          services.netd.config is empty or has no `dns.listen`. Render it with `common/netd/mk-config.nix` (machines/gw/netd.nix is the worked example), or check that the import of that helper did not lose an argument.
        '';
      }
    ];

    environment.etc."netd/config.json".source = checkedConfig;

    systemd.services.netd =
      INFRA.mkNeverGiveUp {
        description = "netd: DHCP, IPv6 router advertisements and DNS for the network";
        wantedBy = ["multi-user.target"];
        # The VLAN devices are what it answers on. It does not need them to exist
        # to *start* (the DHCP loop attaches per interface, and a missing one is a
        # journal line rather than an exit) — but a netd that starts before
        # networkd has made the VLANs spends its first second looking at an empty
        # /sys/class/net, and the log should say "waiting" rather than "no device".
        after = ["network.target" "systemd-networkd.service"];
        wants = ["systemd-networkd.service"];
        # A config change is a restart: the daemon reads its document once and
        # every generated fact (a pool, a reservation, a name) lives in it. There
        # is no reload path, and none is wanted — a resolver that reloads half a
        # document is a resolver with two configs.
        restartTriggers = [checkedConfig];
        serviceConfig = {
          Type = "simple";
          ExecStart = "${netd}/bin/netd run --config /etc/netd/config.json";
          # `netd check` already ran at build time; this is the same binary, one
          # second look, in case the file under /etc was edited by hand. Fail
          # before binding a socket rather than after.
          ExecStartPre = "${netd}/bin/netd check --config /etc/netd/config.json";
          # Root, because the sockets are: port 67 bound per device with
          # SO_BINDTODEVICE (CAP_NET_RAW) and raw ICMPv6 for router
          # advertisements (CAP_NET_RAW). The capability set is exactly those,
          # and the filesystem is not writable outside the state directory.
          User = "root";
          AmbientCapabilities = ["CAP_NET_RAW" "CAP_NET_BIND_SERVICE"];
          CapabilityBoundingSet = ["CAP_NET_RAW" "CAP_NET_BIND_SERVICE"];
          NoNewPrivileges = true;
          ProtectSystem = "strict";
          ProtectHome = true;
          PrivateTmp = true;
          # The lease snapshot is the only thing it writes.
          StateDirectory = "netd";
          StateDirectoryMode = "0755";
          # Nothing here is supposed to take more than a second to start; a
          # startup that hangs is a wedged activation, and this is how it is
          # noticed inside five minutes rather than at the rollback.
          TimeoutStartSec = "60s";
        };
      }
      // {
        # `StateDirectory=netd` makes /var/lib/netd writable under
        # ProtectSystem=strict. `config.state_dir` is the same path, and the
        # assertion is here because a mismatch is a daemon that cannot persist
        # leases and says so only in its API.
        preStart = ''
          test -w ${toString (cfg.config.state_dir or "/var/lib/netd")} || {
            echo "services.netd.config.state_dir (${toString (cfg.config.state_dir or "")}) is not writable: the unit's StateDirectory is /var/lib/netd" >&2
            exit 1
          }
        '';
      };

    # The state directory is the unit's StateDirectory, but the *daemon* creates
    # the file inside it; nothing else needs a tmpfiles rule. What does need one is
    # the directory on a box where the unit has not started yet — systemd creates it
    # with StateDirectory on first start, so this is only here for `netd leases`
    # run by an operator before the first boot.
    systemd.tmpfiles.rules = ["d /var/lib/netd 0755 root root -"];
  };
}
