{ config, pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    # System Tools
    appimage-run
    tmux
    wget
    inetutils
    dmidecode
    pciutils
    pcimem
    nfs-utils
    openssl
    usbutils
    unzip
    fwupd
    lm_sensors
    libva-utils
    intel-gpu-tools
    nil
    nixd
    vim
    clinfo
    redis
    nginx-sso
    amdgpu_top
    nushell
    # Networking Tools
    openldap
    arp-scan
    tcpdump
    speedtest-go
    nmap
    # Development Tools
    git
    direnv
    deno
    nixfmt-rfc-style
    # System Monitoring Tools
    iotop
    lsof
    smem
    memray
  ];

  # Ping the router and only exit once a successful ping comes back. Prevents services starting before the network is truely ready.
  systemd.services.wait-for-network = {
    description = "Wait for Network";
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${pkgs.bash}/bin/bash -c 'until ${pkgs.iputils}/bin/ping -c1 -W2 10.3.1.1 >/dev/null 2>&1; do sleep 2; done'";
      TimeoutStartSec = 30;
    };
  };
}
