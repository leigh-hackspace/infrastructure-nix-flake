# MODULE — the hackspace intercom: bare Asterisk, moved off the `mercury` Proxmox VM
# (VM 128, Sangoma FreePBX 15 / Asterisk 18 on CentOS 7) on 2026-11-16.
#
# **Read this header before touching the dial plan: it says what was deliberately left
# out, and what was measured to be unused.**
#
# What `mercury` actually was, from its own CDR and config (all of it read off the live
# box on 2026-11-16, receipts in docs/pbx-on-services1-2026-11-16.md): **five extensions,
# two registered devices (a Snom 710 in the front space, a Snom 715 in the workshop), zero
# trunks, zero outbound routes, no voicemail, no call recordings, no IVR, no queues, no
# remote/cloud clients (`zulu_*` and `sangomaconnect_*` tables empty), no phone
# provisioning (`/tftpboot` empty, no TFTP listener, the whole httpd access log was one
# curl), and 305 calls in 2.4 years — every dialled destination an extension or one of
# eight soundboard numbers.** It could never place or receive an external call, because
# there was nothing to place one through. So the FreePBX layer — 116 enabled PHP modules,
# MariaDB, httpd, node/pm2, on an OS that went EOL in June 2024 — existed to generate a
# dial plan whose entire content is below.
#
# What this module therefore is: `services.asterisk` (the nixpkgs module, `confFiles` →
# `/etc/asterisk`) with `package = pkgs.asterisk_22`, and one file per object *type*,
# because that is the shape Asterisk's pjsip loader wants and the shape FreePBX proved it:
# `[1] type=endpoint` and `[1] type=aor` in **separate** files, so `Dial(PJSIP/1)` reads
# the way the handset's caller-id does. Merge them into one file and the two categories
# merge into one object with two `type=` lines, which is a PBX that answers nothing.
#
# ## The address, and why the phones needed no changes
#
# The Snoms were configured by hand with `10.3.1.166` — an **IP, in the phone** — and
# `10.3.1.166` is inside VLAN 225's dynamic range (common/vlans.nix: `.150`–`.253`), which
# is why `machines/gw/dns.nix` now tells dnsmasq never to lease it. This box claims the
# address as a second address on `vlan225` (`machines/services1/networking.nix`, the same
# shape as the IdP's `.36`), and the SIP **passwords came across verbatim** from mercury's
# `pjsip.auth.conf` as a runtime sops secret — so the phones re-registered by themselves
# and nobody had to touch a desk. Moving that address off a bare IP in the DHCP pool
# (`pbx.int`, or at least out of the pool) is open in docs/follow-ups.md. Until then this
# box answers `.166` as much as it answers `.20`, and `just pbx-status` says who is
# registered. The `assertions` below are what stop that fact drifting silently.
#
# ## Secrets: runtime, and one file, because this is exactly the shape that wants it
#
# `/run/secrets/pbx_pjsip_auth` **is** the five `type=auth` sections, copied off mercury
# rather than regenerated. It is not in `infra.buildTimeSecrets`: nothing needs a value
# interpolated into a generated config, the secret *is* the file, so `preStart` installs
# it to `/var/lib/asterisk/pjsip_auth_secrets.conf` at 0640 `asterisk:asterisk` and
# `pjsip.conf` `#include`s it by absolute path. No password enters the Nix store, and
# `pjsip.conf` stays reviewable. `preStart` **fails** if the secret is missing or empty, so
# a box that cannot authenticate its phones says so in the journal instead of quietly
# 401-ing two handsets forever.
#
# ## What was dropped, and how you would find out you were wrong
#
# No voicemail (spool empty, `voicemail.conf` had no mailboxes), no call recording
# (`/var/spool/asterisk/monitor` was **0 bytes** with `one_touch_recording=on` set for two
# years), no follow-me, no ring-group call-waiting confirmations, no DISA/paging/queues/
# conference (the stock modules load, nothing referenced them), and the `72736` "Scream"
# code — already dead on mercury: its ring group points at group `75736`, which is in no
# table, and no `scream.wav` exists anywhere. `s` answers nothing here, and 92 of those 305
# CDR rows were `1→s`, so if somebody misses it they will say so within a fortnight.
#
# **Logging is syslog-only on purpose.** mercury's `freepbx.log` was 3.67 GB on a 26 GB
# root — 83 % of a sampled tail was one deprecation line written by a per-minute cron — and
# `/etc/logrotate.d/` had nothing for it. Here `logger.conf` is the module default
# (`syslog.local0`), i.e. `journalctl -t asterisk`, which journald rotates. If you add a
# `full` logfile, give it a rotation in the same commit.
{
  config,
  lib,
  pkgs,
  ...
}: let
  PBX_ADDRESS = "10.3.1.166";

  # The soundboard: eight files, 2.2 MB, copied out of mercury's
  # /var/lib/asterisk/sounds/en_GB/custom on 2026-11-16. They are the only non-config
  # payload on that VM, and they are what the eight dial codes below play.
  PBX_SOUNDS = ../resources/pbx-sounds;

  # extension -> caller-id, the five devices mercury had. The caller-id is also the label
  # the handset shows itself, so renaming here is a physical-world change.
  EXTENSIONS = {
    "1" = "Frontspace";
    "2" = "Workshop";
    "3" = "test";
    "123" = "Raccoon";
    "1337" = "phyu";
  };

  # dial code -> sound + who it rings. mercury's behaviour: answer, play the sound to the
  # *caller*, then ring the owner for 20 s, then play the sound again if nobody picked up.
  SOUNDBOARD = {
    "1234" = {
      sound = "Voicy_Raccoon_mewing";
      desc = "soundplay";
      rings = "123";
    };
    "25436" = {
      sound = "alien";
      desc = "alien";
      rings = "3";
    };
    "4225" = {
      sound = "the-sound-of-dial-up-internet-6240";
      desc = "dialup modem sounds";
      rings = "3";
    };
    "6667837" = {
      sound = "monster";
      desc = "monstermash";
      rings = "3";
    };
    "779246" = {
      sound = "psycho";
      desc = "psycho";
      rings = "3";
    };
    "934537" = {
      sound = "xfiles";
      desc = "xfiles";
      rings = "3";
    };
    "9463697" = {
      sound = "w95";
      desc = "windows95";
      rings = "3";
    };
    "946674" = {
      sound = "whoosh";
      desc = "Stargate whoosh";
      rings = "123";
    };
  };

  # mercury's codec list, in its own order. Both Snoms speak all five, and a codec the
  # phone prefers but is not offered falls back silently — so parity, not tidiness.
  CODECS = "ulaw,alaw,gsm,g726,g722";

  endpointBody = ext: name: ''
    [${ext}]
    type=endpoint
    context=from-phone
    aors=${ext}
    auth=${ext}-auth
    callerid="${name} <${ext}>"
    allow=${CODECS}
    dtmf_mode=rfc4733
    direct_media=yes
    rtp_symmetric=yes
    force_rport=yes
    rewrite_contact=yes
    aggregate_mwi=yes
    language=en_GB
    ; NO endpoint ACL, and that is a measured decision, not an oversight. Three forms of
    ; `acl=` were tried on Asterisk 22 and all three failed **closed and silently**:
    ;
    ;   * `acl=10.3.0.0/16` → `named_acl.c: ACL '10.3.0.0/16' does not exist … will
    ;     automatically fail if applied`.
    ;   * `acl=10.3.0.0/255.255.0.0` (dotted netmask, as the legacy docs spell it) → same.
    ;   * `acl=lan` with a `[lan] deny=0.0.0.0/0.0.0.0 permit=…` stanza in acl.conf — the
    ;     form the shipped sample prescribes (`type=` is *not* valid in a named section:
    ;     `Could not find option suitable for category 'lan' named 'type'`) — parsed fine,
    ;     and `asterisk -rx "acl show lan"` still answered `Could not find ACL named 'lan'`
    ;     with `acl show` listing nothing, i.e. the file's named ACLs never loaded.
    ;
    ; What it looked like from outside: every REGISTER challenged `401` and was then refused
    ; `Failed to authenticate`, while `pjsip show endpoints/aors/auths` all looked perfect and
    ; the migrated passwords were provably byte-identical to the old box's database. Handsets
    ; were dead by that one line for an hour.
    ;
    ; **The reason was in the journal the whole time and I did not read it**, because I
    ; grepped for auth/401/credential words and the line is filed under a different verb:
    ;
    ;     WARNING acl.c:772 in ast_apply_acl_internal: SIP ACL: Rejecting '10.3.14.215'
    ;             due to use of an invalid ACL 'lan'.
    ;
    ; `grep -c "invalid ACL"` is the check. The A/B that settles it: with `acl=lan` the same
    ; scripted REGISTER gets `401` forever; with the line removed and the daemon restarted it
    ; gets `SIP/2.0 200 OK`, and both handsets register within their next expiry cycle.
    ;
    ; So the boundary here is: **no DNAT for 5060 on the router** (`machines/gw/firewall.nix`
    ; has none, and `wan2lan` policy-drops), services1's own firewall is off because the
    ; router is this network's firewall, there is **no TCP listener at all** (no AMI :5038,
    ; no ARI/HTTP :8088), and every endpoint requires digest auth with a real password.
    ; `acl show lan` returning something is the precondition of ever putting this back; see
    ; docs/follow-ups.md.
  '';

  # mapAttrsToList supplies (key, value); the AOR needs only the extension number, so the
  # caller-id argument is bound and ignored rather than written as a one-argument function.
  aorBody = ext: _name: ''
    [${ext}]
    type=aor
    max_contacts=1
    remove_existing=yes
  '';

  soundboardBody = code: v: ''
    ; ${v.desc}: plays custom/${v.sound} to the caller, then rings ${v.rings}
    exten => ${code},1,Answer()
     same => n,Wait(1)
     same => n,Playback(custom/${v.sound})
     same => n,Dial(PJSIP/${v.rings},20)
     same => n,Playback(custom/${v.sound})
     same => n,Hangup()
  '';

  extensionBody = ext: ''
    exten => ${ext},1,Dial(PJSIP/${ext},30)
     same => n,Hangup()
  '';
in {
  services.asterisk = {
    enable = true;
    package = pkgs.asterisk_22;

    confFiles = {
      # Transports and the include chain. There is deliberately **no**
      # external_media_address / external_signaling_address line, and that absence is a
      # fix: mercury had `external_signaling_address = 104.145.12.182` — a public Microsoft
      # Azure address that has never been ours — which put itself into every `Contact:`
      # header, so both handsets were being told to send in-dialog SIP to a stranger's
      # server. With no trunk there is no external address to advertise, and `acl.conf` is
      # what decides who may reach this socket.
      "pjsip.conf" = ''
        ; GENERATED by machines/services1/services/pbx.nix — never edit /etc/asterisk.

        [global]
        user_agent=hackspace-pbx

        [transport-udp]
        type=transport
        protocol=udp
        bind=0.0.0.0:5060

        ; One object type per file (see the header), and the auth sections from
        ; /run/secrets by absolute path. NOTE the comment character: Asterisk's conf parser
        ; treats `#` as a directive, so `# foo` is a parse *warning* ("Unknown directive
        ; '#'") — only `;` comments, and `#include` is the one `#` form that is real.
        #include pjsip.endpoint.conf
        #include pjsip.aor.conf
        #include </var/lib/asterisk/pjsip_auth_secrets.conf>
      '';

      "pjsip.endpoint.conf" = ''
        ; GENERATED by machines/services1/services/pbx.nix
        ${lib.concatStringsSep "\n" (lib.mapAttrsToList endpointBody EXTENSIONS)}
      '';

      "pjsip.aor.conf" = ''
        ; GENERATED by machines/services1/services/pbx.nix
        ${lib.concatStringsSep "\n" (lib.mapAttrsToList aorBody EXTENSIONS)}
      '';

      "extensions.conf" = ''
        ; GENERATED by machines/services1/services/pbx.nix
        ;
        ; One context, because there is one kind of caller: a registered device on the LAN.
        ; Exact matches are declared before `_X!` and Asterisk takes the most specific
        ; match at the same priority, so the soundboard codes and the five extensions win
        ; over the catch-all.
        [from-phone]
        ; `s` rang nothing on mercury either, and 92 of its 305 CDR rows were calls to it.
        exten => s,1,Hangup()

        ${lib.concatStringsSep "\n" (lib.mapAttrsToList soundboardBody SOUNDBOARD)}

        ${lib.concatStringsSep "\n" (map extensionBody (lib.attrNames EXTENSIONS))}

        ; Anything else numeric gets an audible busy rather than silence, so a wrong
        ; number is obvious on the handset that dialled it.
        exten => _X!,1,Congestion()
        same => n,Hangup()
      '';

      "rtp.conf" = ''
        ; GENERATED by machines/services1/services/pbx.nix
        ;
        ; mercury's defaults, written down instead of inherited. Media flows phone<->phone
        ; and, with direct_media=yes, between the phones' VLAN 227 and this box on VLAN 225
        ; — the router routes those VLANs without NATing them, and `rtp_symmetric=yes` is
        ; what actually makes media survive the VLAN hop.
        [general]
        ipv6=yes
        10000-20000
      '';

      "indications.conf" = ''
        ; GENERATED by machines/services1/services/pbx.nix
        ;
        ; en_GB, which every endpoint's `language=` already says. `country=` is not optional
        ; decoration: without a country that has a section here Asterisk logs
        ; "Missing default country (for indication tones)" and tone apps (including the
        ; busy this plan gives a wrong number) have no cadence to play.
        [general]
        language=en_GB
        country=gb

        [gb]
        description = United Kingdom
        ringcad = 1
        cadence = 400,200,400,2000
        freq1 = 400
        freq2 = 0
      '';

      "cdr.conf" = ''
        ; GENERATED by machines/services1/services/pbx.nix
        ;
        ; Keep a call log. Answering "was this thing ever used" meant reading a MySQL table
        ; off a VM that had never been backed up; a future decommission should not need that
        ; archaeology. Unanswered calls are kept too — a call that rang an empty room is a
        ; fact. journald-backed, not a growing file. (No `compressed`: it is not an option in
        ; Asterisk 22's CDR ACO, and a made-up key is a load error, not a default.)
        [general]
        enable = yes
        unanswered = yes
      '';
    };
  };

  # The five SIP auth sections, verbatim from mercury, rendered by sops at boot. Group
  # `asterisk` is what lets preStart install a 0640 file the daemon can read, and nothing
  # else on this box is in that group.
  #
  # **`sopsFile` is a separate file on purpose, and it is the whole reason this deploy
  # does not bounce the network.** Every name in `secrets/secrets.yaml` is decrypted into
  # its own `runCommand` keyed on that *file* (common/sops.nix), so adding one key there
  # re-hashes all of them — measured here: with the key in `secrets.yaml`, `nix flake check
  # --no-build` failed on `sops-secret-pg_pass.drv` and then `sops-secret-strata_api_key.drv`
  # because nothing was realised any more, which is the same fact as "every unit that
  # embeds any secret is dirtied and restarted": Grafana, Outline, Affine, nginx-sso, both
  # Strata engines and the status dashboards on four boxes, to add a phone password. A
  # runtime-only secret with no build-time caller costs that nothing at all, so it lives in
  # `secrets/pbx.yaml` — same recipients (verified with the box's own runtime key: `sudo
  # env SOPS_AGE_KEY_FILE=/var/lib/sops-nix/key.txt sops -d secrets/pbx.yaml`).
  sops.secrets.pbx_pjsip_auth = {
    sopsFile = ../../../secrets/pbx.yaml;
    group = "asterisk";
    mode = "0440";
  };

  systemd.services.asterisk = {
    # The nixpkgs module seeds /var/lib/asterisk with `cp -r <package>/var/lib/asterisk/*`
    # **only if the directory does not exist** — so this runs *after* it (`mkAfter`), never
    # before, and that is not a style choice. Pre-creating those directories is not a
    # harmless optimisation: the package's `var/lib/asterisk` carries
    # `documentation/core-en_US.xml`, and without that tree `res_stasis` has nothing to
    # initialise from. Measured on the first deploy of this module, with `mkBefore`: the
    # daemon exited 1 in 26 ms, logged **nothing** to syslog, and only a foreground
    # `sudo asterisk -vvvcc -n` surfaced
    #
    #     Stasis initialization failed.  ASTERISK EXITING!
    #     Couldn't find manager DBGet in XML documentation …
    #
    # So the module seeds, and this re-establishes what is ours — the soundboard symlink and
    # the rendered auth file — on every start, rather than as a first-boot snapshot.
    preStart = lib.mkAfter ''
      set -eu

      SECRET=/run/secrets/pbx_pjsip_auth
      if ! [ -s "$SECRET" ]; then
        echo "pbx: $SECRET is missing or empty." >&2
        echo "pbx: the five SIP auth sections are sops.secrets.pbx_pjsip_auth," >&2
        echo "pbx: so refusing to start a PBX that cannot authenticate its two phones." >&2
        exit 1
      fi

      # A symlink into the store, so removing a WAV from the repo removes it from the
      # dial plan instead of leaving an orphan on a server nobody looks at. `sounds/` is
      # not in the package (no core sounds are built), so this creates it.
      mkdir -p /var/lib/asterisk/sounds/en_GB
      ln -sfn ${PBX_SOUNDS} /var/lib/asterisk/sounds/en_GB/custom

      install -m 0640 -o asterisk -g asterisk "$SECRET" \
        /var/lib/asterisk/pjsip_auth_secrets.conf

      chown -R asterisk:asterisk /var/lib/asterisk /var/spool/asterisk /var/log/asterisk
    '';

    # The module ships this unit with `restartIfChanged = false`, on the theory that a
    # telephony daemon should not drop a live call because a config file changed. On this
    # network that trade is backwards, and it was measured the hard way: after fixing six
    # parse errors and redeploying, `asterisk -rx "pjsip show endpoints"` still showed the
    # previous config and the journal still showed the previous errors, because nothing had
    # restarted — the deploy looked successful and had applied nothing. ~12 calls a month,
    # each a few minutes, against a change that silently does not land (and therefore against
    # the repo's own rule of deploying and reading the rendered result): **restart.**
    restartIfChanged = lib.mkForce true;

    # …and `restartIfChanged` alone is **not enough**, which is how an hour went by with a
    # fixed configuration deployed and a daemon still running the old one. `confFiles` reaches
    # the daemon through `environment.etc` symlinks, which are not a dependency of the unit,
    # so a dial-plan edit changes no part of the unit definition and systemd restarts
    # nothing: `just switch` reported success, `/etc/asterisk/pjsip.endpoint.conf` on disk
    # was right, and `asterisk -rx "pjsip show endpoints"` was still answering from the
    # previous config. Measured with a probe that got `401` from the stale process and
    # `200 OK` from the same command sixty seconds after `systemctl restart asterisk`.
    #
    # The trigger is a hash of the rendered config text. It is computed from strings already
    # in this module, so it costs nothing at eval and reads no derivation (AGENTS rule 22) —
    # and it cannot hash the SIP secret, which is deliberately not in `confFiles`: that
    # arrives at runtime from `/run/secrets` via `preStart`, so rotating a phone password
    # does not restart anyone's call.
    restartTriggers = [
      (builtins.hashString "sha256" (builtins.concatStringsSep "\n" (
        builtins.attrValues config.services.asterisk.confFiles
      )))
    ];

    # A daemon crash must still restart, under the fleet's never-give-up policy (AGENTS.md:
    # `Restart` belongs in serviceConfig, `StartLimitIntervalSec` on the unit).
    serviceConfig.Restart = "always";
    serviceConfig.RestartSec = "5s";
    startLimitIntervalSec = 0;
  };

  # `10.3.1.166` has to be answered by this box, because that IP is in two handsets' SIP
  # accounts and the router no longer leases it. Checked against the *same* option
  # networking.nix sets, so the two cannot drift: services1 is networkd-managed, so
  # `networking.interfaces` would say nothing at all here.
  assertions = [
    {
      assertion = lib.any (a: a == "${PBX_ADDRESS}/24") (
        config.systemd.network.networks."11-vlan225".networkConfig.Address or []
      );
      message = ''
        machines/services1/services/pbx.nix expects this box to answer ${PBX_ADDRESS}: it
        is the IP the two Snom handsets have in their SIP account, and the router is told
        not to lease it. Add it to the `Address` list of
        `systemd.network.networks."11-vlan225"` in machines/services1/networking.nix —
        a second address, exactly like the IdP's 10.3.1.36 — or move the handsets, which
        is the open item in docs/follow-ups.md.
      '';
    }
  ];
}
