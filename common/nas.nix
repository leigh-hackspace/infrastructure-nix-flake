# NAS client: the NFS mounts, their automount units, and the `wait-for-nas`
# gate that NAS-dependent services order themselves against.
#
# Imported for both machines in flake.nix.  A machine declares its shares in
# `machines/<name>/hardware-configuration.nix`:
#
#   infra.nas.exports = [
#     { where = "/mnt/cameras"; share = "/mnt/sas-10k/cameras"; }
#     { share = "/mnt/sas-10k/filestore"; }   # where defaults to /mnt/<basename>
#   ];
#
# and every service that needs the NAS must declare (AGENTS.md, NAS dependency
# rule):
#
#   systemd.services.<svc> = {
#     after    = [ "wait-for-nas.service" ];
#     requires = [ "wait-for-nas.service" ];
#   };
#
# Why the automount units are generated here: `x-systemd.automount` in a mount's
# options is only honoured by systemd-fstab-generator.  NixOS `systemd.mounts`
# produces *unit-file* mounts, for which the option is dead config — the
# `.automount` units have to exist explicitly, otherwise the shares are mounted
# eagerly at boot (and a NAS that is still importing after a power cut stalls
# the whole machine).  Generating both halves from one list is what stops them
# drifting apart.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.infra.nas;

  # Options every NAS share gets.  `_netdev` keeps the mount out of the boot
  # critical path, `x-systemd.automount` is documentation for the fstab case
  # (the real automount unit is generated below).
  shareOptions = ["nfsvers=4.2" "_netdev" "x-systemd.automount" "retry=5" "timeo=5" "x-systemd.mount-timeout=30"];

  # systemd's unit name for a mount point: leading '/' dropped, '/' -> '-', and
  # any '-' in the path escaped as \x2d (so /mnt/ds-photos becomes
  # mnt-ds\x2dphotos, which is the name NixOS actually generates for it).
  unitNameOf = path:
    lib.replaceStrings ["-" "/"] ["\\x2d" "-"] (lib.removePrefix "/" path);
in {
  options.infra.nas = {
    host = lib.mkOption {
      type = lib.types.str;
      default = "10.3.1.6";
      description = ''
        IP of the NAS (nas2, TrueNAS). nas1/10.3.1.5 is disabled and does not
        answer. Referenced by the share devices and by wait-for-network
        (common/tools.nix), which pings it to decide the network is up.
      '';
    };

    exports = lib.mkOption {
      type = with lib.types;
        listOf (submodule {
          options = {
            share = lib.mkOption {
              type = str;
              description = "Path of the export on the NAS, e.g. /mnt/sas-10k/cameras.";
            };
            where = lib.mkOption {
              type = str;
              defaultText = lib.literalExpression ''"/mnt/''${baseNameOf share}"'';
              description = "Local mount point.";
            };
            options = lib.mkOption {
              type = with lib.types; nullOr (listOf str);
              default = null;
              description = ''
                Replace the generated NFS mount options (rarely wanted).  Given as
                a list, e.g. [ "nfsvers=4.2" "_netdev" ].
              '';
            };
          };
        });
      default = [];
      example = lib.literalExpression ''[ { share = "/mnt/sas-10k/cameras"; } ]'';
      description = "NFS shares to mount from infra.nas.host.";
    };
  };

  config = lib.mkIf (cfg.exports != []) {
    boot.supportedFilesystems = ["nfs"];

    systemd.mounts =
      map (m: {
        where = m.where;
        what = "${cfg.host}:${m.share}";
        type = "nfs";
        options = lib.concatStringsSep "," (
          if m.options == null
          then shareOptions
          else m.options
        );
        # The NAS has to be reachable before an NFS mount can even be attempted.
        after = ["wait-for-network.service"];
        requires = ["wait-for-network.service"];
      })
      cfg.exports;

    systemd.automounts =
      map (m: {
        where = m.where;
        # Enable at boot so the mount points exist and can be triggered lazily.
        wantedBy = ["multi-user.target"];
      })
      cfg.exports;

    # Wait for the NAS to be up *and* for its NFS exports to be genuinely
    # mounted before NAS-dependent services start.
    #
    # The mounts themselves are lazy, so simply pinging the NAS is not enough:
    # TrueNAS can answer ping long before its NFS exports are ready.  This
    # service keeps triggering the automounts until `findmnt` reports a real
    # nfs* filesystem instead of an idle autofs mount, then exits successfully.
    systemd.services.wait-for-nas = {
      description = "Wait for NAS mounts to become available";
      wantedBy = ["multi-user.target"];
      after =
        map (m: "${unitNameOf m.where}.automount") cfg.exports
        ++ ["network-online.target"];
      wants = ["network-online.target"];
      path = [pkgs.util-linux];

      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        # Never give up: keep waiting for as long as the NAS takes to come back.
        TimeoutStartSec = 0;
        ExecStart = pkgs.writeShellScript "wait-for-nas" ''
          for m in ${lib.concatStringsSep " " (map (m: m.where) cfg.exports)}; do
            # `ls` on an automount point makes systemd attempt the real mount.
            # While the NAS (or its NFS exports) are still coming up the
            # attempt fails quickly; retrigger until the share is mounted.
            until [ "$(findmnt -n -o FSTYPE "$m" 2>/dev/null)" != "autofs" ] \
               && [ -n "$(findmnt -n -o FSTYPE "$m" 2>/dev/null)" ]; do
              ls "$m" >/dev/null 2>&1 || true
              sleep 2
            done
          done
        '';
      };
    };
  };
}
