flakeInputs: {
  config,
  pkgs,
  ...
}: {
  networking.hostName = "aibox"; # Define your hostname.

  imports = [
    ./ai.nix
    ./alexandria.nix
    ./configuration.nix
    ./frigate-monitor.nix
    ./hardware-configuration.nix
    ./immich.nix
    ./monitoring.nix
    ./netboot.nix
    ./networking.nix
    # PARKED (see docs/repo-audit-2026-10-07.md §1.4): the GTX 1060 was removed
    # from this box on 2026-09-02, so ./nvidia.nix is kept for reference only.
    # ./nvidia.nix
    ./sso.nix
    ./status-dashboard.nix
    ./strata.nix
    ./whisper.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];

  # Strata (Qwen3.8-Flash-Next on the iGPU) replaces llama-server as this box's
  # coding model; see machines/aibox/strata.nix for why the two cannot coexist.
  services.strata.enable = true;
}
