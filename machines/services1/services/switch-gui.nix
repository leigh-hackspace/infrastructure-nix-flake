# switch-gui: ports and VLANs on the ProCurve core switch, in a browser.
#
# Rust source in ../../../switch-gui (zero external crates; the Telnet client,
# the CLI parsers and the HTTP side are all in-tree, the latter on the shared
# common-rs/web).  It reads the switch's configuration over Telnet — merging the
# four `show` tables that split a port's name, link state and VLAN membership
# across them — and serves one page at switch.int.leighhack.org on
# 127.0.0.1:8097.
#
# The credentials live in one sops secret, a KEY=value file (not a template),
# created on 2026-10-12 — see `switch_gui_env_file` in ../sops.nix.  Keys:
#
#   SWITCH_GUI_LOGIN=<switch login>
#   SWITCH_GUI_PASSWORD=<switch password>
#   SWITCH_GUI_OIDC_CLIENT_ID=<authentik client id>
#   SWITCH_GUI_OIDC_CLIENT_SECRET=<authentik client secret>
#   SWITCH_GUI_ENABLE_LOGIN=<optional Manager login, enables writes>
#   SWITCH_GUI_ENABLE_PASSWORD=<optional Manager password>
#
# Without the first two the service exits at startup naming the missing key;
# without the OIDC pair it refuses to serve anything but /healthz on a
# non-loopback bind.  The authentik side is provider pk 35 ("switch-gui"), which
# uses the authorization-code + S256 PKCE contract in switch-gui/src/oidc.rs and
# redirects to https://switch.int.leighhack.org/auth/callback; it advertises the
# `groups` scope, without which every login is refused as "not in Infra".  Read
# docs/authentik-oidc-notes.md before touching it — including how to add a second
# consumer of the groups scope without tripping the ORM.
#
# Why the switch account is not enough on its own: the switch authenticates
# against RADIUS, so a browser login here would either be a second, weaker door
# to the same device or a shared password.  Sign-in is authentik gated on the
# `Infra` group instead, and the switch's own credentials stay on the server.
#
# Writes need the Manager context, and `agent` has it: `enable` is a second
# login on this firmware and the same credentials answer it, so the enable pair
# defaults to the login pair. It is nonetheless **disabled here with
# `--read-only`** — see the ExecStart comment and switch-gui/README.md; the
# console has never echoed a configuration command back, so no write is proven.
# Configuration would apply inside config mode (`Switch::apply` enters it), and
# nothing here saves to NVRAM, so a change is in RAM until someone writes it.
{
  config,
  lib,
  pkgs,
  CRANE,
  INFRA,
  ...
}: let
  HOSTS = import ../../../common/hosts.nix;
  mkIntVhost = import ../../../common/nginx/int-vhost.nix {inherit lib;};

  port = 8097;
  hostname = "switch.int.leighhack.org";

  # How stale the cached snapshot may get before a page load re-reads the
  # switch.  A full read is six-plus Telnet round-trips against a 1990s CPU
  # that pages its own output, so this is the difference between a snappy page
  # and a sluggish one; the Refresh button forces it.
  cacheTtlSecs = 30;

  switchGui = CRANE.cached {
    pname = "switch-gui";
    version = "0.1.0";
    src = ../../../switch-gui;
    cargoLock = CRANE.lockFile ../../../switch-gui/Cargo.lock;
    # `common-json` and `common-web` path dependencies (see common-rs/ and
    # common/crane.nix).
    sharedCrates = ["json" "web"];
  };
in {
  systemd.services.switch-gui = INFRA.mkNeverGiveUp {
    description = "switch-gui: port and VLAN management for the ProCurve switch";
    wantedBy = ["multi-user.target"];
    after = ["network-online.target"];
    wants = ["network-online.target"];

    # Non-secret wiring.  The switch address comes from common/hosts.nix so the
    # GUI, the vhost and any future gatus check agree, and it is an address
    # rather than a name on purpose: this is a tool for debugging the network,
    # so it must not resolve anything.
    environment = {
      SWITCH_GUI_HOST = HOSTS.switch;
      SWITCH_GUI_PORT = "23";
      SWITCH_GUI_REQUIRED_GROUP = "Infra";
      SWITCH_GUI_OIDC_REDIRECT = "https://${hostname}/auth/callback";
      # The OIDC token and introspection calls go out over HTTPS through curl
      # (the tool keeps to zero external crates, so no reqwest), and a build-time
      # store path means an upgrade cannot change it under a running unit.
      SWITCH_GUI_CURL = "${pkgs.curl}/bin/curl";
    };

    serviceConfig = {
      Type = "simple";
      ExecStart = lib.concatStringsSep " " [
        "${switchGui}/bin/switch-gui"
        "--bind"
        "127.0.0.1:${toString port}"
        "--ttl"
        (toString cacheTtlSecs)
        "--switch-url"
        # The switch's own web UI is **http only** — measured: `http://10.3.1.2`
        # answers 200, `https://10.3.1.2` does not connect at all.  It is only
        # ever a link out of the page, but a dead button is still a bug.
        "http://${HOSTS.switch}"
        # Writes are off in this deployment.  The Manager context is reached and
        # `config` is accepted, but the command after it is never echoed by the
        # console, so nothing has been proven to apply — see switch-gui/README.md
        # ("The write path, and why it is off").  Reads are unaffected; drop this
        # flag once a write has been verified live.
        "--read-only"
      ];

      # The switch login, its password and the OIDC client pair.  systemd reads
      # this as PID 1 before dropping privileges, so the service itself never
      # needs to open the secret file — which is why it can be an unprivileged
      # dynamic user rather than root (the pattern filestore/gocardless avoid by
      # reading CONFIG.ENV_FILE themselves; a LAN-facing web app should not run
      # as a privileged account, see the network-status follow-up in AGENTS.md).
      EnvironmentFile = config.sops.secrets.switch_gui_env_file.path;

      # No state at all: the snapshot and the audit log are in memory, so there
      # is no user, no home and no StateDirectory to leave behind.
      DynamicUser = true;
      NoNewPrivileges = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
      PrivateDevices = true;
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectControlGroups = true;
      RestrictSUIDSGID = true;
      LockPersonality = true;
      # Outbound: Telnet to the switch and HTTPS to authentik, plus the DNS
      # lookup of the latter.  Nothing listens on anything but the loopback
      # listener, which is AF_INET/AF_INET6 too.
      RestrictAddressFamilies = ["AF_INET" "AF_INET6"];
      RestrictNamespaces = true;
      CapabilityBoundingSet = "";
    };
  };

  # LAN-only vhost (the binary only listens on 127.0.0.1).  dns-sync picks the
  # record up from `services.nginx.virtualHosts`, so `just dns-sync-sync` after
  # deploying creates switch.int.leighhack.org.
  services.nginx.virtualHosts.${hostname} = mkIntVhost {
    proxyPass = "http://127.0.0.1:${toString port}";
  };
}
