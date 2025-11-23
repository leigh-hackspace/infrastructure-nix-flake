{
  pkgs,
  lib,
  config,
  ...
}:

{
  services.samba = {
    enable = true;
    openFirewall = true;
    settings = {
      global = {
        # "passdb backend" = ''
        #   ldapsam:"ldap://ldap.int.leighhack.org"
        # '';

        # "ldap suffix" = "DC=ldap,DC=goauthentik,DC=io";
        # "ldap user suffix" = "ou=users";
        # "ldap group suffix" = "ou=groups";

        "workgroup" = "WORKGROUP";
        "server string" = "smbnix";
        "netbios name" = "smbnix";
        "security" = "user";
        "hosts allow" = "10.3. 100.64. 192.168.49. 127.0.0.1 localhost";
        "hosts deny" = "0.0.0.0/0";
        "guest account" = "nobody";
        "map to guest" = "bad user";
      };
      "Filestore" = {
        "path" = "/mnt/filestore";
        "browseable" = "yes";
        "read only" = "no";
        "guest ok" = "yes";
        "create mask" = "0664";
        "directory mask" = "0775";
        "force user" = "hackspacer";
        "force group" = "users";
      };
    };
  };

  services.samba-wsdd = {
    enable = true;
  };
}
