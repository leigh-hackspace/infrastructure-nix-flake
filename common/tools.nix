{
  config,
  lib,
  pkgs,
  INFRA,
  ...
}: {
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
    vim
    clinfo
    redis
    nginx-sso
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
    # The repo's formatter is alejandra (see .zed/settings.json and `just fmt`);
    # nixfmt is deliberately not installed so the two cannot drift.
    nil
    nixd
    alejandra
    just
    # System Monitoring Tools
    iotop
    lsof
    smem
    memray
  ];

  # GPU diagnostics are per-machine on purpose: services1 is the Intel/i915 box
  # (it installs intel-gpu-tools) and aibox is the AMD/ROCm box (amdgpu_top).
  # Keeping them here put both vendors' tools on both machines.

  # Ping the NAS and only exit once a successful ping comes back. Prevents services starting before the network is truely ready.
  systemd.services.wait-for-network = {
    description = "Wait for Network";
    after = ["network-online.target"];
    wants = ["network-online.target"];
    # Start at boot so its state is always visible (e.g. on the status dashboard).
    wantedBy = ["multi-user.target"];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      # The NAS address is an option (infra.nas.host, declared in common/nas.nix)
      # rather than a second copy of the constant.
      ExecStart = "${pkgs.bash}/bin/bash -c 'until ${pkgs.iputils}/bin/ping -c1 -W2 ${config.infra.nas.host} >/dev/null 2>&1; do sleep 2; done; sleep 1'";
      # Never give up: the NAS can take a long time to come back after a power cut.
      TimeoutStartSec = 0;
    };
  };

  # nginx is the reverse proxy for every app on both machines, so a crash loop
  # must never end with systemd giving up permanently.  The nixpkgs module sets
  # Restart=always but leaves the default 60s start rate limit in place (5 quick
  # failures = dead forever), which is the trap common/systemd.nix exists for.
  systemd.services.nginx = lib.mkIf config.services.nginx.enable INFRA.mkNeverGiveUpOverride;
}
