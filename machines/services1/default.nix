flakeInputs: {
  networking.hostName = "services1"; # Define your hostname.

  # services1 delegates builds to aibox (common/build-farm.nix).  Nix prefers the
  # external builder and falls back to local when it is unreachable, so a deploy
  # still works when aibox is down; `just build-remote` is the version that
  # refuses to fall back.  maxJobs is aibox's core count (Ryzen 5 6600H, 6c/12t).
  infra.buildFarm.builderHost = (import ../../common/hosts.nix).aibox;
  infra.buildFarm.maxJobs = 6;

  imports = [
    ./sops.nix
    ./services

    ./ai.nix
    ./configuration.nix
    ./containers.nix
    ./dns-sync.nix
    # Metrics exporters. Prometheus/Grafana run on services2, so this box is one
    # of the things being measured rather than the thing doing the measuring.
    ../../common/exporters.nix
    ./hardware-configuration.nix
    ./http.nix
    ./network-status.nix
    ./networking.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];
}
