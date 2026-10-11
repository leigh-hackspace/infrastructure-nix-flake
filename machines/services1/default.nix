flakeInputs: {
  networking.hostName = "services1"; # Define your hostname.

  # services1 delegates builds to services2 (common/build-farm.nix). It is opt-in per
  # invocation via `just build-remote`, so a deploy still works when the builder is
  # down.
  infra.buildFarm.builderHost = (import ../../common/hosts.nix).services2;

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
