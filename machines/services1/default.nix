{
  # `flakeInputs` arrives through specialArgs — flake.nix's `extraArgs` — which is the
  # one thing this file needs it for: `imports` below wants
  # `flakeInputs.sops-nix.nixosModules.sops`, and a dashed attribute name cannot be a
  # function-argument pattern.  It used to be curried (`flakeInputs: {…}`, applied at
  # flake.nix's call site) for exactly that line.
  flakeInputs,
  ...
}: {
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
    ./int-dns.nix
    # Metrics exporters: this box runs the apps and is measured by its own Prometheus
    # (observability moved here from services2 on 2026-10-17, so services1 is now both
    # the scraper and one of the scraped — see services/observability.nix and
    # common/exporters.nix for what that costs).
    ../../common/exporters.nix
    ./hardware-configuration.nix
    ./http.nix
    ./networking.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];
}
