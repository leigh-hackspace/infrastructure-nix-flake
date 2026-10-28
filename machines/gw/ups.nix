# ups: the UPS on this box's USB port, and what a power cut does to the network.
#
# What is attached, read off the machine on 2026-10-28 (`lsusb`,
# `/sys/bus/usb/devices/2-1.2/`, `journalctl -k`):
#
#     usb 2-1.2   idVendor=051d idProduct=0002   American Power Conversion
#     Back-UPS XS 1400U  FW:926.T2 .I USB FW:T2   serial 4B1836P34201
#     hid-generic 0003:051D:0002.0005: hiddev97,hidraw4
#
# Two facts about that device shape everything below.  **It is HID-only**: there
# is no serial half, no `usblp`, and — unlike a laptop battery — it will never
# appear in `/sys/class/power_supply` or in `upower`, because Linux has no
# in-kernel driver for APC's HID UPS protocol.  `hid-generic` claims it and stops
# there, which is why `power.ups` (NUT) has to own `/dev/hidraw4`: NUT is the
# maintained implementation of the protocol, and `usbhid-ups` speaks APC's HIDP
# dialect.  The kernel's own HID power-supply handling is not an alternative, it
# is the thing that leaves `/sys/class/power_supply` empty.
#
# **And it is on `gw`.** Not on a service box: on the machine that answers DNS
# and DHCP for every device in the building, holds the network's certificate and
# terminates every vhost (`docs/router-front-door-migration.md`), and dials the
# AAISP tunnel through which all IPv6 leaves.  A router that dies hard takes the
# *name resolution, the leases and the default route* of boxes that are still
# drawing current, and `common/nas.nix`'s NFS clients with them.  That is the
# whole reason this file is a policy module and not just `power.ups.enable =
# true`: NUT can shut down *the box it runs on*, and the correct shutdown for
# this network is an ordered one that starts at the far end.
#
# ## Who decides what
#
# | Question | Owner | Why there |
# | --- | --- | --- |
# | is the UPS discharging? | NUT (`usbhid-ups` → `upsd` → `upsmon`) | protocol |
# | for how long before acting? | `upssched`, in the generated `upssched.conf` below | it is the one component that survives a restart, and `AT ONLINE * CANCEL-TIMER` is the semantics we want: a two-minute flicker must never take the building down |
# | does the box still have mains *now*? | `ups-watchdog`'s re-check (advisory, `crates/ups-watchdog/src/plan.rs`) | a lost `ONLINE` notification must not be a power-off; and a broken `upsc` must not be a battery-to-zero |
# | whom do we tell, in what order? | `ups-watchdog`, from `shutdownTargets` below | NUT has never heard of the rest of the network |
# | does the UPS drop its load? | NUT's `ups-killpower.service` at shutdown, armed by POWERDOWNFLAG | see `powerDownFlag` |
#
# **There is deliberately one timer in this design, and it is not in Rust.** A
# second countdown would be a second countdown to disagree with.
#
# ## What it deliberately does not do
#
# * **No `upsmon` state machine of its own on this box.** `upsmon` is enabled
#   because it is the supported client of `upsd` and the thing that raises the
#   notifications `upssched` reacts to — but its `SHUTDOWNCMD` is pointed at the
#   same unit `upssched` is, so `upsmon` deciding, the timer deciding and a human
#   deciding all arrive at one sequence. Two ways to power off a router is the
#   same class of mistake as two DHCP servers.
# * **No NUT clients on the other boxes.** They are told over ssh, with the same
#   hop key `common/build-farm.nix` uses for the remote builder. Serving `upsd`
#   to the LAN would mean the fleet's shutdown depends on this box's TCP stack
#   being healthy at exactly the moment it may not be, and `openFirewall` stays
#   false for the same reason (`assertions` below refuse a `LISTEN` line).
# * **Nothing in the list is on the NAS's or Proxmox's power supply, and this
#   module does not know whose it is.** `nas` and `monster1` are excluded on
#   purpose: one is TrueNAS (not a flake machine, its own shutdown verb) and the
#   other hosts VMs this flake does not own.  If either is on the same UPS as
#   `gw`, it needs its own answer and `docs/follow-ups.md` has the item.
#
# ## Deploying and proving it
#
# This box holds `nix.settings.max-jobs = 0`, so the crate below compiles on
# aibox and is copied back, like every other derivation here (AGENTS.md rule 22).
# After the switch:
#
#     sudo systemctl status upsdrv upsd upsmon upssched   # the NUT chain is up
#     sudo upsc apc@localhost                              # the UPS answers
#     sudo upsc apc@localhost ups.status                   # OL, or OB
#     just ups-drill                                       # the whole sequence, run for real
#                                                          # except that it switches nothing off
#     cat /run/current-system/etc/systemd/system/ups-fleet-shutdown.service
#
# That last one is AGENTS.md rule 10 and it is not ceremony: the flags below go
# through systemd's quoting, and `upssched.conf` is generated.
#
# And read `docs/ups-and-fleet-shutdown.md` first if you are about to change the
# grace period, the target list, or anything that touches the UPS's load: it has
# the battery maths, the recovery-on-mains question, and what was measured rather
# than assumed about `shutdown.return` on this model.
{
  config,
  lib,
  pkgs,
  CRANE,
  ...
}: let
  HOSTS = import ../../common/hosts.nix;
  VLANS = import ../../common/vlans.nix;

  # The name in `/etc/nut/ups.conf`, and therefore in `upsc <name>@localhost`,
  # `upsmon`'s MONITOR line and every log line this feature produces.  One
  # string, one place.
  upsName = "apc";

  # Seconds the outage has to last before the fleet goes.  Sized for a flicker
  # and a generator start, not for an outage: the XS 1400U carrying a router and
  # nothing else has tens of minutes of runtime, and 300 s of that is not what
  # the number is buying.  The cost of it being wrong in the *other* direction
  # is the network staying up until the battery is empty and `gw` dying mid-write
  # — which is why `LOWBATT` below ignores the grace period entirely.
  # docs/ups-and-fleet-shutdown.md has the runtime arithmetic.
  graceSeconds = 300;

  # Whom to tell, in the order they are told.  **This order is the file's
  # opinion and it is load-bearing:** services1 is the database, the NAS's NFS
  # client and every container; services2 is a second engine; aibox is a build
  # box whose work survives being interrupted but whose store should not be
  # interrupted mid-copy.  `gw` is *not* in the list — see `selfAddresses`.
  shutdownTargets = [
    "services1"
    "services2"
    "aibox"
  ];

  # Addresses that are this box.  `common/hosts.nix` calls the router `router`
  # and the machine's hostname is `gw`, and every VLAN's gateway address is also
  # here, because a hop to 10.3.2.1 from 10.3.2.1 is the same bug with a
  # different spelling.  AGENTS.md rule 27 is the receipt for why "ssh to the
  # box you are on" is a *silent* success rather than an error: the hop key is
  # authorized on every box including this one, so the only guard that works is
  # the one that refuses by address.
  selfAddresses = lib.unique ([
      HOSTS.router
      "127.0.0.1"
    ]
    ++ (map (v: v.ipv4) VLANS));

  upsWatchdog = CRANE.crate {
    pname = "ups-watchdog";
    version = "0.1.0";
    member = "ups-watchdog";
  };

  # The password NUT's client authenticates with, in the world-readable store.
  #
  # It is **not a secret**, and that claim rests on exactly one thing: `upsd`
  # listens on loopback only (`assertions` below refuse a `LISTEN`, and
  # `openFirewall` is never set), so the only process that can present it is
  # `upsmon` on this box. Writing it as a sops secret would be honest about a
  # credential that does not exist — and would hide the real trigger: the moment
  # any second box becomes a NUT *client*, this string protects something, the
  # assertion has to be deleted to get there, and that is the edit that should
  # make you look at `docs/authentik-container-migration.md`-sized care instead.
  nutPasswordFile = pkgs.writeText "nut-upsmon-password" "gw-loopback-only";

  # NUT's POWERDOWNFLAG: the file `ups-killpower.service` waits for at shutdown.
  # One path, three readers — `upsmon.conf`, that unit's `ConditionPathExists=`,
  # and `ups-watchdog --killpower-flag`, which is what arms it on the grace-timer
  # path (where `upsmon` never reaches its own power-off state and so never writes
  # it).
  #
  # **Arming it is not the same as the UPS dropping its load, and today it does
  # not.** `upsc apc@localhost` on the deployed box reports
  # `driver.flag.allow_killpower: 0`, and NUT 2.8 refuses to send the killpower
  # command unless the driver is explicitly given that flag — so `upsdrvctl
  # shutdown` runs, declines the part that matters, and gw stays off with its
  # outlets live after an outage that outlasts the battery. That is left as
  # upstream left it: enabling the flag means this sequence really does cut the
  # network's power, which is a decision for a person to make with a battery on it
  # rather than a default to slip into a module. The reasoning, and what measuring
  # it would take, is `docs/ups-and-fleet-shutdown.md` §5 and §7.1.
  powerDownFlag = "/run/killpower";

  stateDir = "/run/ups-watchdog";

  # The dispatcher: NUT's one entry point into the fleet.
  #
  # `upssched` calls CMDSCRIPT as `<upsname> <event>` and `upsmon`'s SHUTDOWNCMD
  # is a shell line, so both call this with the same two arguments and the event
  # names are the ones the generated `upssched.conf` uses.
  #
  # `--no-block` is not a style choice.  This script runs as a child of `upsmon`
  # inside a notification callback: if it waited for the whole sequence — five
  # hops, a drain, a poweroff — `upsmon` would be blocked from reading the UPS
  # for the duration, and an `ONLINE` arriving then is worth nothing.
  #
  # It runs as root because `upsmon` does (see `power.ups.upsmon.user`), and it
  # needs root to start a system unit; `machines/gw/README.md`'s "least privilege
  # we already pay for" note in docs/ups-and-fleet-shutdown.md says what the
  # unrooted version would cost.
  upsTrigger = pkgs.writeShellScript "ups-trigger" ''
    #!${pkgs.runtimeShell} $@
    set -u

    ups="''${1:-unknown}"
    event="''${2:-unknown}"

    reason=""
    start=0
    notice=""
    priority=info
    case "$event" in
      # The grace timer expired: still on battery after ${toString graceSeconds}s.
      ${upsName}-outage)
        reason=outage-grace-expired
        start=1
        ;;
      # NUT says the battery is nearly empty.  The grace period is over, whatever
      # the clock says.
      ${upsName}-low-battery)
        reason=low-battery
        start=1
        priority=crit
        ;;
      # upsmon reached its own power-off decision.  Same sequence, different
      # witness; `ups-watchdog` treats it as "already late".
      ${upsName}-shutdown)
        reason=upsmon-shutdown
        start=1
        priority=crit
        ;;
      # The three notices.  Each is one message to every server and to this
      # box's own terminals, and all three go through one unit: the body is
      # written to ${stateDir}/notice, the unit reads it back.  A per-event unit
      # would be three units whose ExecStart lines differ by one sentence, and
      # three ways to deploy a message that disagrees with another.
      ${upsName}-observing)
        notice="POWER OUTAGE DETECTED|On battery at $(${pkgs.coreutils}/bin/date -u '+%Y-%m-%dT%H:%M:%SZ').|The servers power off in ${toString graceSeconds}s unless mains return.|Nothing has been switched off yet.|On gw: 'systemctl stop ups-fleet-shutdown' stops it; 'journalctl -fu ups-watchdog' watches it."
        ;;
      ${upsName}-restored)
        notice="MAINS RESTORED|Utility power is back at $(${pkgs.coreutils}/bin/date -u '+%Y-%m-%dT%H:%M:%SZ').|The outage timer was cancelled; no shutdown, no restart, nothing to do.|If a fleet shutdown had already started it cannot be undone: check 'journalctl -u ups-fleet-shutdown'."
        ;;
      ${upsName}-comm-lost)
        notice="UPS UNREACHABLE|gw can no longer read the UPS at $(${pkgs.coreutils}/bin/date -u '+%Y-%m-%dT%H:%M:%SZ').|The outage countdown was cancelled: nothing will be powered off on the strength of a UPS we are not reading.|If mains really are gone, this box is now running on battery with no clock on it: 'journalctl -u upsdrv -u upsd' on gw."
        ;;
      *)
        printf '%s\\n' "unhandled event '$event' (ups=$ups)" \
          | ${pkgs.systemd}/bin/systemd-cat -t ups-watchdog -p warning
        ;;
    esac

    if [ -n "$notice" ]; then
      ${pkgs.coreutils}/bin/mkdir -p ${stateDir}
      # `|` is the line separator: the notice file's first line is the headline
      # and the rest is the body (`ups-watchdog`'s notice_text does the split).
      printf '%s\\n' "$notice" | ${pkgs.coreutils}/bin/tr '|' '\\n' > ${stateDir}/notice
      printf '%s\\n' "event $event: broadcasting to the fleet" \
        | ${pkgs.systemd}/bin/systemd-cat -t ups-watchdog -p info
      ${pkgs.systemd}/bin/systemctl start --no-block ups-notice.service
    fi

    if [ "$start" = 1 ]; then
      ${pkgs.coreutils}/bin/mkdir -p ${stateDir}
      printf '%s\\n' "$reason" > ${stateDir}/reason
      printf '%s\\n' "event $event: starting ups-fleet-shutdown.service (reason $reason)" \
        | ${pkgs.systemd}/bin/systemd-cat -t ups-watchdog -p "$priority"
      ${pkgs.systemd}/bin/systemctl start --no-block ups-fleet-shutdown.service
    fi
  '';

  # `upssched.conf`.  The two `START-TIMER`/`CANCEL-TIMER` lines are the whole
  # outage policy, and they are here rather than in Rust so that the countdown
  # lives beside the thing that raises the events.
  upsschedConf = pkgs.writeText "upssched.conf" ''
    CMDSCRIPT ${upsTrigger}
    PIPEFN /run/nut/upssched.pipe
    LOCKFN /run/nut/upssched.lock
    DATADIR /run/nut/upssched

    # The outage timer: armed by ONBATT, disarmed by ONLINE, fires as the event
    # `${upsName}-outage` when it expires.  A second ONBATT while it runs does not
    # restart it (upssched refuses a duplicate timer), which is the behaviour we
    # want: the clock starts at the first sign of the outage.
    AT ONBATT * START-TIMER ${upsName}-outage ${toString graceSeconds}
    AT ONBATT * EXECUTE ${upsName}-observing
    AT ONLINE * CANCEL-TIMER ${upsName}-outage
    AT ONLINE * EXECUTE ${upsName}-restored

    # Low battery does not wait for the timer; it pre-empts it.
    AT LOWBATT * EXECUTE ${upsName}-low-battery

    # And if the UPS stops answering, the timer is cancelled rather than trusted.
    # A countdown to powering off a network that is being driven by a UPS we can no
    # longer read is a worse failure than the hard-off it was preventing:
    # `ups-watchdog` has no proof either way at that point, and this line is where
    # that is decided.
    AT COMMBAD * CANCEL-TIMER ${upsName}-outage
    AT COMMBAD * EXECUTE ${upsName}-comm-lost
    AT TIMEDOUT * CANCEL-TIMER ${upsName}-outage
    AT TIMEDOUT * EXECUTE ${upsName}-comm-lost
  '';

  # Seconds the notice has to be on the screen before the first hop.  The
  # intervention it is buying is one command (`systemctl stop
  # ups-fleet-shutdown`) typed by whoever reads the wall, so it is timed for a
  # human noticing a terminal and reaching for a keyboard — and it is deliberately
  # *not* generous: every second here is a second of battery spent deciding, and
  # an unattended building at 04:00 reads no walls.
  noticeDelay = 30;

  # The same window when the reason is `LOWBATT` or `upsmon`'s own decision:
  # at that point the countdown is the battery's, not ours.
  lateNoticeDelay = 5;

  # How long to sit after the last hop before powering this box off, so the
  # machines that accepted the command are through their own teardown before the
  # router, the NAS's gateway and everyone's DNS server disappear.
  settleSeconds = 10;

  # What gets run on the far side of a notice hop.  `wall -n` (no `--nobanner`
  # header of its own, since the message carries its own banner) under sudo
  # because `wall` writes to terminals it does not own, and `leigh-admin` has
  # passwordless sudo everywhere by `common/base.nix`.  The message itself travels
  # on stdin, so nothing user-influenced is ever interpolated into this string.
  wallCommand = "sudo -n ${pkgs.util-linux}/bin/wall -n";

  # The flags both the shutdown and the notice need: how to hop, how to ask the
  # UPS, whom to hop to.  One list, because the notice and the poweroff must be
  # aimed at the same machines with the same key — two copies of an identity path
  # is how a fleet ends up being warned by one key and powered off by another.
  sharedArgs =
    [
      "--identity-file"
      # The same key `common/build-farm.nix` hands to the nix daemon for the
      # remote-build hop, and for the same reason: root has it, and the public half
      # is authorized for `leigh-admin` on every box (`common/users.nix`).  Passed
      # by path; `ups-watchdog` never opens it, `ssh` does.
      "/home/leigh-admin/.ssh/agent-hop-key"
      "--ssh-user"
      "leigh-admin"
      "--ssh-bin"
      "${pkgs.openssh}/bin/ssh"
      "--upsc-bin"
      "${config.power.ups.package}/bin/upsc"
      "--ups"
      "${upsName}@localhost"
      "--status-timeout"
      "10"
      "--wall-command"
      wallCommand
      "--local-wall"
      "--local-wall-bin"
      "${pkgs.util-linux}/bin/wall"
      "--broadcast-timeout"
      "15"
      "--ssh-timeout"
      "30"
      "--connect-timeout"
      "5"
    ]
    ++ lib.concatMap (n: [
      "--host"
      "${n}=${HOSTS.${n}}"
    ])
    shutdownTargets
    # Both lists are built from `shutdownTargets`, so a notice cannot be sent to a
    # machine that will not be powered off, or the other way round.  The refusal
    # list is the belt to that brace: see `selfAddresses`.
    ++ lib.concatMap (a: ["--self-address" a]) selfAddresses;

  # The arguments of the two units.  Generated, never typed: the addresses come
  # from `common/hosts.nix` and the refusal list from `selfAddresses`, so neither
  # list can disagree with the table that moves the rest of the network.
  watchdogArgs = extra:
    [
      "run"
      "--reason-file"
      "${stateDir}/reason"
      "--killpower-flag"
      powerDownFlag
      # Store paths rather than a PATH to hope in: the last command of the sequence
      # is the one that ends the outage, and it runs on this box, where a unit's
      # PATH is the store's and not the operator's.
      "--poweroff-bin"
      "${pkgs.systemd}/bin/systemctl"
      # The notice that goes out **before** the first hop.  A wall that arrives
      # after the poweroff command has been sent is a confession, so the message
      # names the countdown, says how to stop it, and the unit then waits
      # `--notice-delay-seconds` before it touches anything: that window is the
      # whole value of the notice, and `--late-notice-delay-seconds` is what is left
      # of it when the battery is already on its way out.
      "--broadcast"
      "--broadcast-headline"
      "FLEET SHUTDOWN BEGINNING"
      "--broadcast-message"
      "gw is on battery and the grace period has run out."
      "--broadcast-message"
      "This router will ssh poweroff every server below, then itself."
      "--broadcast-message"
      "Grace: ${toString noticeDelay}s from the moment this message appeared."
      "--broadcast-message"
      "On gw: 'systemctl stop ups-fleet-shutdown' stops it here."
      "--notice-delay-seconds"
      (toString noticeDelay)
      "--late-notice-delay-seconds"
      (toString lateNoticeDelay)
      "--settle-seconds"
      (toString settleSeconds)
    ]
    ++ sharedArgs
    ++ extra;

  # … and the notice's, which is the same shared list behind a different verb.
  noticeArgs =
    [
      "broadcast"
      "--notice-file"
      "${stateDir}/notice"
    ]
    ++ sharedArgs;

  fleetUnit = "ups-fleet-shutdown";
in {
  assertions = [
    {
      # `nutPasswordFile` lives in the world-readable store, which is only
      # acceptable while nothing outside this box can be asked to present it.
      assertion = config.power.ups.upsd.listen == [];
      message = ''power.ups.upsd.listen names a listener while power.ups.users.upsmon's password is a store path (see machines/gw/ups.nix): a reachable upsd turns that string into a credential, and it has to move to sops first.'';
    }
    {
      assertion = config.power.ups.openFirewall or false == false;
      message = ''power.ups.openFirewall is set on the router. The fleet is told over ssh, so nothing needs 3493 open, and this box's firewall is the network's (machines/gw/firewall.nix).'';
    }
    {
      # The list is the policy; a name that is not in the address table is a
      # machine that will silently not be told, and "silently" is the failure.
      assertion = builtins.all (n: HOSTS ? ${n}) shutdownTargets;
      message = ''machines/gw/ups.nix: shutdownTargets names something common/hosts.nix does not have: ${builtins.concatStringsSep ", " (builtins.filter (n: !(HOSTS ? ${n})) shutdownTargets)}.'';
    }
    {
      # Belt to `selfAddresses`' braces: the router powering itself off over ssh,
      # first, would leave the remaining boxes running with no router and no
      # process that can reach them.
      assertion = !(builtins.elem "router" shutdownTargets) && !(builtins.elem "gw" shutdownTargets);
      message = ''machines/gw/ups.nix: the router must not be in shutdownTargets. It powers itself off locally, last — see the header and crates/ups-watchdog/src/plan.rs.'';
    }
    {
      # `usbhid-ups` on `port = auto` needs exactly one UPS-shaped device.  A
      # second one on this box's USB ports would make `auto` a guess; the pin is
      # the vendor/product pair read off the machine above.
      assertion = config.power.ups.ups.${upsName}.driver == "usbhid-ups";
      message = ''power.ups.ups.${upsName}.driver is not usbhid-ups, but everything in machines/gw/ups.nix (and the hidraw4 reading in its header) is about an APC HID-over-USB UPS.'';
    }
  ];

  # --- NUT ---------------------------------------------------------------------
  #
  # `standalone`: one UPS, this box, no NUT clients.  That is the mode's actual
  # definition and it is why nothing here opens a port — the fleet is powered off
  # over ssh, not over NUT.
  power.ups = {
    enable = true;
    mode = "standalone";

    # The driver reads `port = auto` from `/etc/nut/ups.conf`.  `vendor`/`product`
    # are left alone deliberately: `usbhid-ups` matches on the HID usage and this
    # box has one UPS, while the assertions above refuse the day that stops being
    # true.  `explore_delay`/`synchronous` are the two knobs to reach for if the
    # driver ever starts but reports nothing.
    ups.${upsName} = {
      driver = "usbhid-ups";
      port = "auto";
      description = "APC Back-UPS XS 1400U on usb 2-1.2 (serial 4B1836P34201)";
      # `sdorder` 0: this is the UPS that carries the router, so it is the one NUT
      # is allowed to drop last, at shutdown, via ups-killpower.
      shutdownOrder = 0;
    };

    # Loopback only.  Empty means "no LISTEN line", and NUT's built-in default is
    # 127.0.0.1 and ::1 — which is what the `assertions` above insist on.
    upsd.listen = [];

    users.upsmon = {
      passwordFile = toString nutPasswordFile;
      actions = ["report"];
      upsmon = "primary";
    };

    # `upsmon` is the only client `upsd` has, and it is the only thing that turns
    # a driver's flags into the notifications `upssched` counts on.  It runs as
    # root here so that its NOTIFYCMD/SHUTDOWNCMD children can start a system
    # unit: the alternative is a `systemd.path` unit woken by a marker file
    # written by `nutmon`, which is one more unit and one more race on the path
    # that has to work during a power cut.  nixpkgs' own module already runs
    # `upsd` as root (its TODO says so), so this box is not the place where that
    # gets decided.
    upsmon = {
      user = "root";
      group = "root";
      monitor.${upsName} = {
        system = "${upsName}@localhost";
        powerValue = 1;
        user = "upsmon";
        # `master`: this box is the one with the UPS, so it is the one that
        # decides. There are no NUT slaves on this network.
        type = "master";
      };
      settings = {
        MINSUPPLIES = 1;
        POWERDOWNFLAG = powerDownFlag;
        # NUT's own path to a power-off, redirected at the same unit `upssched`
        # starts. `upsmon` re-runs SHUTDOWNCMD every 5 s until the machine is
        # down, and `RemainAfterExit` on the unit is what makes the second and
        # third of those starts do nothing at all.
        SHUTDOWNCMD = "${upsTrigger} ${upsName} ${upsName}-shutdown";
        # upsmon → upssched. The module sets this by default; it is stated
        # because everything above depends on it and a default is not a contract.
        NOTIFYCMD = "${config.power.ups.package}/bin/upssched";
        # Without +EXEC on each of these, upssched is never told and the timers
        # never start: NOTIFYFLAG is the switch, not the log level.
        NOTIFYFLAG = [
          ["ONBATT" "SYSLOG+EXEC"]
          ["ONLINE" "SYSLOG+EXEC"]
          ["LOWBATT" "SYSLOG+EXEC"]
          ["COMMBAD" "SYSLOG+EXEC"]
          ["TIMEDOUT" "SYSLOG+EXEC"]
        ];
      };
    };

    # `schedulerRules` is `types.str` in this nixpkgs module (its own default is a
    # string too), and the module hands it straight to
    # `environment.etc."nut/upssched.conf".source` — so a derivation here is an
    # eval error, found by `nixos-rebuild build` rather than by the router's
    # eval-only gate.  `toString` on a `writeText` is its store path, which is
    # exactly what the default form is.
    schedulerRules = toString upsschedConf;
  };

  # `ups-trigger` writes the reason file into /run, and `ups-watchdog` reads it
  # back. 0755 root:root because both are root and nothing else needs it.
  systemd.tmpfiles.rules = ["d ${stateDir} 0755 root root -"];

  environment.systemPackages = [upsWatchdog];

  # --- the two units -----------------------------------------------------------
  #
  # Neither is `WantedBy`: nothing starts them on boot, and that is the point —
  # a oneshot that runs at boot would be a fleet shutdown triggered by a reboot.
  #
  # `RemainAfterExit` is the load-bearing line.  `upsmon` polls SHUTDOWNCMD every
  # five seconds and `upssched` can fire twice (LOWBATT after the timer), and each
  # re-start must be a no-op rather than a second pass over the fleet.  It is
  # also why the unit is not wrapped in `INFRA.mkNeverGiveUp`: a service that
  # powers the machine off must not be restarted, and it must never be
  # `Restart=always` with the poweroff as its idea of "exited cleanly".
  systemd.services.${fleetUnit} = {
    description = "UPS outage: power the fleet off over ssh, then this box";
    documentation = ["https://github.com/leigh-hackspace/infrastructure-nix-flake/blob/main/docs/ups-and-fleet-shutdown.md"];

    # The hops are the work; the unit must outlive a slow box, and `TimeoutStop`
    # has to be short because a unit that is slow to die on the way down costs
    # seconds of battery at the worst possible moment.
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${lib.getExe upsWatchdog} ${lib.escapeShellArgs (watchdogArgs [])}";
      TimeoutStartSec = 240;
      TimeoutStopSec = 10;
      KillMode = "control-group";
    };

    # ssh needs `$PATH`-visible tools of its own; the unit's PATH is the store's,
    # not the operator's.
    path = [
      pkgs.coreutils
      pkgs.openssh
      pkgs.procps
    ];

    unitConfig = {
      # Started by name from ups-trigger and by hand by an operator; never pulled
      # in by a target.
      RefuseManualStart = false;
      DefaultDependencies = false;
      After = "network.target";
    };
  };

  # The outage notice: `ups-trigger` writes ${stateDir}/notice and starts this.
  #
  # A unit rather than `systemctl cat`-style directness inside the dispatcher,
  # because it is the piece of this that has to survive being unreachable — it
  # runs on the router, over the hop key, while the clock is running — and a unit
  # leaves three things a shell function does not: `journalctl -u ups-notice` says
  # which servers took the message, `systemctl is-failed ups-notice` says whether
  # to trust it, and `just ups-notice-test` on a running system proves the whole
  # path without touching a power cable.
  #
  # No `RemainAfterExit` (unlike its shutdown sibling): it is expected to run
  # again — outage, restore, outage — and unlike a poweroff it is idempotent
  # anyway.  Not wrapped in `INFRA.mkNeverGiveUp` either: a notice that failed once
  # has usually failed because the network is down, and restarting it in a loop
  # spends the battery it is announcing.
  systemd.services.ups-notice = {
    description = "UPS: broadcast a notice to every server (wall over ssh)";
    documentation = ["https://github.com/leigh-hackspace/infrastructure-nix-flake/blob/main/docs/ups-and-fleet-shutdown.md"];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${lib.getExe upsWatchdog} ${lib.escapeShellArgs noticeArgs}";
      TimeoutStartSec = 120;
      TimeoutStopSec = 10;
    };
    path = [
      pkgs.coreutils
      pkgs.openssh
      pkgs.procps
    ];
    unitConfig = {
      RefuseManualStart = false;
      DefaultDependencies = false;
      After = "network.target";
    };
  };

  # The drill unit. `--dry-run` is not the operator's judgement call at 03:00, it
  # is a second unit: it cannot be started by an event, because nothing refers to
  # it, and it prints the argv it would run instead of running it.
  systemd.services."${fleetUnit}-dry" = {
    description = "UPS outage drill: print what would be done, do nothing";
    documentation = ["https://github.com/leigh-hackspace/infrastructure-nix-flake/blob/main/docs/ups-and-fleet-shutdown.md"];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${lib.getExe upsWatchdog} ${lib.escapeShellArgs (watchdogArgs ["--dry-run" "--no-poweroff"])}";
      TimeoutStartSec = 60;
    };
    path = [
      pkgs.coreutils
      pkgs.openssh
      pkgs.procps
    ];
    unitConfig = {
      RefuseManualStart = false;
      DefaultDependencies = false;
    };
  };
}
