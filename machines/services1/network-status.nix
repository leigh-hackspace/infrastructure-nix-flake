# network-status: real-time web dashboard for the OPNsense router (10.3.1.1).
#
# Rust source: ../../network-status (zero external crates, house style —
# see status-dashboard/). The service ssh's to the router every few seconds
# with the machine-hop key and serves per-interface bandwidth, firewall
# connection (pf state) counts, CPU/memory/load and potential issues on a
# realtime canvas-charted page.
#
# Exposed at network-info.int.leighhack.org (nginx vhost in http.nix,
# ACL-restricted to the local network).
{
  config,
  lib,
  pkgs,
  ...
}:

let
  networkStatus = pkgs.rustPlatform.buildRustPackage {
    pname = "network-status";
    version = "0.1.0";
    src = ../../network-status;
    cargoLock.lockFile = ../../network-status/Cargo.lock;
    doCheck = false;
  };
in
{
  systemd.services.network-status = {
    description = "Router network status dashboard (network-info.int.leighhack.org)";
    wantedBy = [ "multi-user.target" ];
    after = [ "network.target" ];
    # Runs as leigh-admin so it can read the machine-hop key directly.
    serviceConfig = {
      Type = "simple";
      User = "leigh-admin";
      # ssh client for probing the router (note: systemd 260 removed the
      # Path= unit option, so PATH is set explicitly).
      Environment = [
        "HOME=/home/leigh-admin"
        "PATH=${pkgs.openssh}/bin"
      ];
      ExecStart = lib.concatStringsSep " " [
        "${networkStatus}/bin/network-status"
        "--bind" "127.0.0.1"
        "--port" "8091"
        "--router" "root@10.3.1.1"
        "--ssh-key" "/home/leigh-admin/.ssh/agent-hop-key"
        "--interval" "5"
        "--wan" "em0"
        "--title"
        # Quoted: the em dash + spaces would otherwise split the argv.
        "\"Network — router\""
      ];
      Restart = "always";
      RestartSec = "5s";
    };
    # Never give up.
    startLimitIntervalSec = 0;
  };
}
