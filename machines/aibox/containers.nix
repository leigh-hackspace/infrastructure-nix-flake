{
  lib,
  config,
  ...
}:
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
  # Mirrors machines/services1/containers.nix; the nixos-utils.containers
  # module is imported in flake.nix for the aibox flake output.
  systemd.services = lib.mapAttrs' (name: _: lib.nameValuePair "podman-${name}" {
    startLimitIntervalSec = 0;
    serviceConfig = {
      Restart = lib.mkForce "always";
      RestartSec = "5s";
    };
  }) config.virtualisation.oci-containers.containers;
}
