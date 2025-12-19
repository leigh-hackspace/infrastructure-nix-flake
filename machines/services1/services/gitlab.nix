{
  config,
  lib,
  ...
}:

{
  services.gitlab = {
    enable = true;
    databasePasswordFile = pkgs.writeText "dbPassword" "zgvcyfwsxzcwr85l";
    initialRootPasswordFile = pkgs.writeText "rootPassword" "dakqdvp4ovhksxer";
    secrets = {
      secretFile = pkgs.writeText "secret" "Aig5zaic";
      otpFile = pkgs.writeText "otpsecret" "Riew9mue";
      dbFile = pkgs.writeText "dbsecret" "we2quaeZ";
      jwsFile = pkgs.runCommand "oidcKeyBase" { } "${pkgs.openssl}/bin/openssl genrsa 2048 > $out";
    };
  };

  services.nginx.virtualHosts = {
    "gitlab.int.leighhack.org" = {
      locations."/" = {
        proxyPass = "http://unix:/run/gitlab/gitlab-workhorse.socket";
        recommendedProxySettings = true;
      };
    };
  };
}
