flakeInputs: {
  networking.hostName = "services2"; # Define your hostname.

  # services2 is the build machine for this network (common/build-farm.nix): 211 GB
  # of free store and no services worth protecting from a build, while services1 is
  # the box that runs the services and is at 84% of its root partition.
  infra.buildFarm.serveBuilds = true;

  imports = [
    ./sops.nix

    # What this box runs: its own status dashboard and (since the RTX 3060 arrived)
    # the Strata engine.  The rest of what it used to run — gatus, Prometheus and
    # Grafana — moved to services1 on 2026-10-17, because this box's 30 GB of RAM
    # is strata's: the engine sits at ~28 GiB RSS against those services and the
    # kernel reaps process after process under that pressure.  See ./README.md for
    # what still runs here, what is left to measure this box, and ./STRATA.md for
    # the engine.
    ./status.nix
    ./strata.nix

    ./configuration.nix
    ./hardware-configuration.nix
    # NVIDIA driver for the RTX 3060 that runs Strata's CUDA engine.
    ./nvidia.nix
    ./networking.nix

    # Metrics exporters — the per-box half of monitoring.  This box is still
    # measured, now by services1's Prometheus (machines/services1/services/
    # observability.nix), which scrapes 10.3.1.21 over the LAN.
    ../../common/exporters.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];

  # The Qwen3.8-Flash-Next Coder engine on the RTX 3060; see ./strata.nix for
  # the one-time model prep and the memory sizing.
  services.strata.enable = true;
}
