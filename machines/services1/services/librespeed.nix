{lib, ...}: let
  LIBRESPEED_UID = 8004;
in {
  users.users.librespeed = {
    uid = LIBRESPEED_UID;
    group = "users";
    isNormalUser = true;
  };

  # The speedtest container itself, still published on the wildcard at 8004. The name
  # `speed.leighhack.org` and the **anubis** bot gate that fronts it are rows/targets on
  # the front door now (`common/front-door.nix`'s `anubis = true`, rendered by
  # `machines/gw/http.nix`): the gate is a front-door function and there is one of it,
  # so its target is `10.3.1.20:8004` and the unix socket and the `nginx`-in-`anubis`
  # group line moved with it. A second instance here would be a second gate deciding
  # about the same name.
  virtualisation.oci-containers.containers = {
    librespeed = {
      hostname = "librespeed";
      image = "ghcr.io/librespeed/speedtest:latest";
      autoStart = true;
      ports = [
        "${toString LIBRESPEED_UID}:8080"
      ];
      environment = {
        TZ = "Europe/London";
      };
    };
  };
}
