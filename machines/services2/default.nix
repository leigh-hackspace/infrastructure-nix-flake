flakeInputs: {
  networking.hostName = "services2"; # Define your hostname.

  # services2 is the build machine for this network (common/build-farm.nix): 211 GB
  # of free store and no services worth protecting from a build, while services1 is
  # the box that runs the services and is at 84% of its root partition.
  infra.buildFarm.serveBuilds = true;

  imports = [
    ./sops.nix

    # What this box actually runs: the observability stack (gatus + Prometheus +
    # Grafana), its own status dashboard, and (since the RTX 3060 arrived) a
    # Strata instance.  See ./README.md for why monitoring lives here rather than
    # on services1 and machines/services2/STRATA.md for the engine.
    ./gatus.nix
    ./monitoring.nix
    ./status.nix
    ./strata.nix

    ./configuration.nix
    ./hardware-configuration.nix
    # NVIDIA driver for the RTX 3060 that runs Strata's CUDA engine.
    ./nvidia.nix
    ./networking.nix

    # Metrics exporters — the per-box half of monitoring.  This box is measured by
    # the Prometheus it runs itself, so it needs its own exporters too.
    ../../common/exporters.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];

  # The Qwen3.8-Flash-Next Coder engine on the RTX 3060; see ./strata.nix for
  # the one-time model prep and the memory sizing.
  services.strata.enable = true;
}
