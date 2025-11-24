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
}
