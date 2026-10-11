flakeInputs: {
  networking.hostName = "services2"; # Define your hostname.

  # services2 is the build machine for this network (common/build-farm.nix): 211 GB
  # of free store and no services worth protecting from a build, while services1 is
  # the box that runs the services and is at 84% of its root partition.
  infra.buildFarm.serveBuilds = true;

  imports = [
    ./sops.nix

    # What this box actually runs: the observability stack (gatus + Prometheus +
    # Grafana) and its own status dashboard.  See ./README.md for why monitoring
    # lives here rather than on services1.
    ./gatus.nix
    ./monitoring.nix
    ./status.nix

    ./configuration.nix
    ./hardware-configuration.nix
    ./networking.nix

    # Metrics exporters — the per-box half of monitoring.  This box is measured by
    # the Prometheus it runs itself, so it needs its own exporters too.
    ../../common/exporters.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];
}
