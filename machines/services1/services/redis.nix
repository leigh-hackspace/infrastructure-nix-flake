{
  pkgs,
  lib,
  config,
  ...
}: let
  CONFIG = import ../../../common/config.nix;
in {
  services.redis.servers.outline = {
    enable = true;
    port = 8401;
    bind = "127.0.0.1";
    settings = {
      protected-mode = "no";
    };
  };
}
