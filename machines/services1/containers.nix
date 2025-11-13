{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  update-containers = (
    pkgs.writeShellScriptBin "update-containers" ''
      images=$(${pkgs.podman}/bin/podman ps -a --format="{{.Image}}" | sort -u)

      for image in $images
      do
        ${pkgs.podman}/bin/podman pull $image
      done

      ${pkgs.systemd}/bin/systemctl restart podman-*
    ''
  );
in
{
  systemd.timers = {
    update-containers = {
      timerConfig = {
        Unit = "update-containers.service";
        OnCalendar = "Mon 02:00";
      };
      wantedBy = [ "timers.target" ];
    };
  };
  systemd.services = {
    update-containers = {
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${update-containers}/bin/update-containers";
      };
    };
  };

  virtualisation.podman = {
    enable = true;
    autoPrune.enable = true;
    dockerCompat = true;
    dockerSocket.enable = true;
    defaultNetwork.settings.dns_enabled = true;
  };

  virtualisation.oci-containers.backend = "podman";
}
