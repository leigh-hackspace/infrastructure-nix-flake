{
  pkgs,
  lib,
  config,
  ...
}:

{
  services.cockpit = {
    enable = true;
    port = 9090;
    allowed-origins = [
      "http://10.3.1.20:9090"
      "https://cockpit.int.leighhack.org"
    ];
    settings = {
      WebService = {
        AllowUnencrypted = true;
      };
    };
  };
}
