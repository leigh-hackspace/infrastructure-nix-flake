{
  pkgs,
  lib,
  config,
  ...
}:

let
  CONFIG = import ../config.nix;
in
{
  # services.redis.servers.affine = {
  #   enable = true;
  #   port = 8301;
  #   bind = "127.0.0.1 10.88.0.1";
  #   settings = {
  #     protected-mode = "no";
  #   };
  # };

  services.redis.servers.outline = {
    enable = true;
    port = 8401;
    bind = "127.0.0.1";
    settings = {
      protected-mode = "no";
    };
  };
}
