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

  # This box builds for itself, and has done since 2026-11-23; before that it delegated
  # to aibox (the history and the why are in common/nix-gc.nix's header, and what it would
  # take to want delegation back is in common/ssh-hop.nix's footer).  Nothing about this
  # box's own arithmetic changed, so the two facts worth carrying are unchanged: it is
  # 4c/8t with 15 GiB of RAM and the tightest root on the network, so
  # `just check-build` — five system toplevels, all copied back into *this* store — is a
  # big ask here, and `just test`/`just clippy`/`just check-build` belong on aibox, the
  # biggest box.  What delegation was hiding is that a `just switch` here has always
  # compiled the crates that changed; it just did it somewhere else.

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
    # Whose inference endpoints pi may dial, discovered rather than asserted:
    # ../../common/pi-agent.nix, common/pi-endpoints.nix, docs/pi-agent-models.md.
    # This box held one of the four hand-written `models.json` copies it replaces, and
    # one of the copies of the engine's Bearer key it removes.
    ../../common/pi-agent.nix
    ./hardware-configuration.nix
    ./http.nix
    ./networking.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];

  programs.pi-agent.enable = true;
}
