# Podman and the "never give up" policy for every oci-containers unit.
#
# Imported for both machines in flake.nix.  The per-machine copies of this file
# were byte-identical apart from services1's container-update timer, which now
# lives in machines/services1/containers.nix.
{ config, lib, INFRA, ... }:
{
  virtualisation.podman = {
    enable = true;
    autoPrune.enable = true;
    dockerCompat = true;
    dockerSocket.enable = true;
    defaultNetwork.settings.dns_enabled = true;
  };

  virtualisation.oci-containers.backend = "podman";

  # OCI container services must never give up.  The default Restart policy
  # from `oci-containers` is "on-failure", and systemd's start rate-limit
  # (5 starts in 10s) permanently stops a unit after a handful of quick
  # failures — e.g. a container that keeps failing while waiting for the NAS
  # to come back after a power cut.  Force Restart=always and disable the
  # rate limit so containers retry forever.
  #
  # Note that the nixos-utils.containers module imported in flake.nix is the
  # *container image update* timer, not this policy.
  systemd.services = lib.mapAttrs' (name: _: lib.nameValuePair "podman-${name}" INFRA.mkNeverGiveUpOverride)
    config.virtualisation.oci-containers.containers;
}
