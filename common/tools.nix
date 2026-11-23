# MODULE — NixOS module; in a machine's module list or a directory's imports, never read as data.
{
  config,
  lib,
  pkgs,
  INFRA,
  ...
}: let
  # GREP DROP-IN — a grep-compatible, multithreaded `grep` on every box's PATH, and
  # the name is load-bearing (docs/grep-dropin.md owns the receipts).  ugrep keys its
  # grep-compatibility mode to **argv[0]**: installed as `bin/grep`, `grep --version`
  # answers `ugrep 7.8.5 (grep compat)` and GNU's BRE default, hidden-file search and
  # by-name ordering come back, which is what keeps `grep 'a+'` a literal `a+`.
  # Why it is a symlink named grep and not an alias: an alias never reaches `bash -c`
  # — measured — and that is exactly the shell an agent's tool call runs in, while
  # PATH is inherited; and on this pinned nixpkgs every PATH directory ahead of
  # /run/current-system/sw/bin is missing, so there is nothing to shadow *into*, and
  # the closure itself is the only declarative place to change — this pinned nixpkgs
  # writes no home files (rule 32), so the per-user profile dirs on PATH are empty and
  # there is nothing earlier than /run/current-system/sw/bin to shadow into.
  # `just grep-compat` is the gate that proves the swap changes nothing this repo
  # depends on; it runs from `just check-eval`.
  grepCompat = pkgs.runCommand "grep-as-ugrep" {} ''
    mkdir -p $out/bin
    ln -s ${pkgs.ugrep}/bin/ugrep $out/bin/grep
  '';
in {
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
    # The grep on PATH, and the same engine under its own names for humans: `ugrep`
    # and `ug` stay available so a person typing `ug` gets the interactive flavour,
    # while `grep` gets the compat mode keyed to argv[0] (docs/grep-dropin.md).
    grepCompat
    ugrep
    # Networking Tools
    openldap
    arp-scan
    tcpdump
    speedtest-go
    nmap
    # Development Tools
    git
    # GitHub's CLI, for the API rather than for transport: git over ssh already works
    # here (origin is git@github.com:, keyed by each box's own account key), and `gh`
    # is the way to ask GitHub about PRs, workflow runs and releases without a token in
    # a URL or a `curl` with an `Authorization:` header in the transcript (rule 13).
    # Only the *binary* is declarative: `gh auth login` writes
    # ~/.config/gh/hosts.yml holding an OAuth token, which is per-account runtime state
    # and must never be generated into the store — a store path is world-readable to
    # every box that builds or substitutes it (the reason `config.sopsSecretText` is
    # gated by name in common/sops.nix).  So `gh` lands on a box with `just switch`
    # and signs in once per account afterwards, off NAS-backed home directories.
    gh
    direnv
    deno
    # jq and python3 are on every box so ad-hoc inspection (flake.lock, the JSON
    # the exporters and monitoring endpoints emit, a service's API) works from a
    # bare ssh session.  Neither is in the default profile, and reaching for
    # `nix run` mid-debug pulls a whole store path and a daemon lock; `nix develop
    # --command` is not an option at all when the point of the call is to read the
    # state of a box that is mid-switch.  python3 is the interpreter only — no
    # packages, per the Rust-for-everything rule (AGENTS.md 1).
    jq
    python3
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

  # tmux is how an agent session outlives the ssh that started it (common/users.nix
  # turns lingering on for exactly that reason), and pi — the coding agent — asks the
  # tmux server what it has been configured to do and warns on every start until the
  # answer changes.  Its check (`checkTmuxKeyboardSetup` in pi's interactive-mode.js)
  # is two
  # `tmux show -gv` reads: `extended-keys` must answer `on` or `always`, then
  # `extended-keys-format` must not answer `xterm`.  Both of tmux's defaults are the
  # warning-triggering ones and nothing in this repo had ever configured tmux, so the
  # warning was fleet-wide and permanent: measured 2026-11-16 there was no ~/.tmux.conf
  # and no /etc/tmux.conf on any box.  (Four boxes have tmux, the ones importing this
  # file.  `gw` has none — `routerModules` deliberately does not import tools.nix, and
  # the first read that said otherwise was `pgrep -a`, whose `-a` implies `-f` and so
  # matches the shell running the pgrep.)
  #
  # What the warning is about: with those defaults tmux reports Shift+Enter, Ctrl+Enter
  # and plain Enter as one key, so pi's editor cannot take a newline.
  #
  # /etc/tmux.conf and not a ~/.tmux.conf for the reason rule 32 gives for
  # /etc/ssh/ssh_config: this pinned nixpkgs has no home-file option under
  # `users.users`, so a dotfile here could only be hand-copied box by box and drift.
  # tmux reads /etc/tmux.conf first and ~/.tmux.conf second, so an account can still
  # override it and the account that matters — the one whose server holds the agent's
  # panes — needs nothing installed.
  #
  # `csi-u` rather than the `xterm` modifyOtherKeys default because pi prefers it and
  # tmux >= 3.5 can speak it (this flake pins tmux 3.8).  `on` and deliberately not
  # `always`: `always` forces mode 1 on the outer terminal for every program in the
  # pane, including ones that want the standard sequences.
  #
  # **This file cannot fix a tmux server that is already running** — tmux reads it at
  # server start only, and `tmux kill-server` is not an option from inside the session
  # it would take down.  A live server takes the same two lines by hand:
  #
  #     tmux set -g extended-keys on && tmux set -g extended-keys-format csi-u
  #
  # and since a session option unset on the session falls back to the global value,
  # that reaches the session you are standing in.  It does not on its own make the keys
  # work: there is a third gate, the `extkeys` terminal-feature, which pi cannot see and
  # which is negotiated when a *client attaches*.  Measured 2026-11-16 on the live
  # server: the two lines satisfied the warning in one second and left `pane_key_mode`
  # reading `VT10x` with no `extkeys` in `client_termfeatures`, because that client
  # predates the change.  So the warning can go away while the keys stay broken, and
  # `docs/nix-dev-workflow.md` ("tmux's keys: what pi's warning checks, and what it
  # cannot") owns the sequence that settles it and what to conclude if it never does.
  #
  # Re-verify with: tmux show -gv extended-keys extended-keys-format   # on, csi-u
  environment.etc."tmux.conf".text = ''
    # Managed by NixOS — common/tools.nix.  tmux reads this file before ~/.tmux.conf,
    # so a per-account file still wins.  pi (the coding agent) warns on every start
    # until the first line below is not `off`; the third gate it cannot check is in
    # docs/nix-dev-workflow.md.
    set -g extended-keys on
    set -g extended-keys-format csi-u
  '';

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
