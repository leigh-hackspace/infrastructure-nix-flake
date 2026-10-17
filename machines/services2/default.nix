flakeInputs: {
  networking.hostName = "services2"; # Define your hostname.

  # services2 no longer serves remote builds (2026-10-17): aibox is the network's
  # builder now (common/build-farm.nix).  The reason is in this box's STRATA.md —
  # its 30 GiB of RAM is the Coder engine's, and a delegated build competing with
  # the engine was the box's worst failure mode.  It still builds locally, and
  # keeps its 211 GB of free store.

  imports = [
    ./sops.nix

    # What this box runs: its own status dashboard and (since the RTX 3060 arrived)
    # the network's only live Strata engine.  The rest of what it used to run —
    # gatus, Prometheus and Grafana — moved to services1 on 2026-10-17, because
    # this box's 30 GB of RAM is strata's: the engine sits at ~28 GiB RSS against
    # those services and the kernel reaps process after process under that
    # pressure.  See ./README.md for what still runs here and what is left to
    # measure this box, and ./STRATA.md for the engine.
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
