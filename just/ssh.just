# JUST — ssh host keys and identities: the pin every ssh reads, including the nix daemon’s hop.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- the router's ssh host keys

# Print the host key lines a client needs to verify 10.3.1.1. Nothing ships them
# any more: the pin used to be /etc/dns-sync/known_hosts, read by a tool that
# ssh'd from services1 to edit the network's DNS, and that tool is gone
# (common/int-dns.nix and `just switch-gw` are how `*.int` gets a record now).
# `crates/network-status`'s remote probe mode still takes --known-hosts and
# refuses to connect without it, which is the point: a missing file is no ssh,
# never an unverified one. Run this after a router reinstall, keep the output in
# a file, and hand that file to whatever is connecting.
#
# As leigh-admin, not root: the router's root login is prohibit-password, and
# this recipe used to ssh in as root, which meant it could not regenerate the
# pin for the box it replaced. Keyscanning over the network (rather than reading
# /etc/ssh/ssh_host_*.pub on the box) is deliberate: it records the key a client
# is actually offered, and the client will connect over the same path. Types the
# router does not offer simply print nothing, so the output is what it presents
# today — two lines for gw, three for the OPNsense box it replaced.
router-known-hosts:
    ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes leigh-admin@10.3.1.1 'sh -c "ssh-keyscan -t ed25519,ecdsa,rsa 10.3.1.1 2>/dev/null"'

# The pinned host keys of the flake machines, in the form
# common/ssh-known-hosts.nix carries them (which is what
# `programs.ssh.knownHosts` turns into /etc/ssh/ssh_known_hosts, and therefore
# what the *nix daemon's* ssh reads when it runs a remote-build hop).  Run this
# after any box is reinstalled or rotates its host keys, paste the new lines into
# that DATA table, deploy; until then the hop to that box fails with
# `Host key verification failed` (docs/nix-dev-workflow.md,
# "Remote building: the hop is not the operator's ssh").
#
# Keyscanned over the wire rather than read from /etc/ssh/ssh_host_*.pub: the
# client connects to an address and is offered whatever that address answers now,
# which is the only thing that can make the hop work.  -T 5 because ssh-keyscan's
# one-second default drops lines when several addresses are queried at once, and
# a half-written pin is worse than a missing one.
known-hosts:
    #!/usr/bin/env bash
    set -euo pipefail
    addrs=$(nix eval --impure --raw --expr 'let h = import ./common/hosts.nix; in builtins.concatStringsSep " " [ h.router h.services1 h.services2 h.services3 h.aibox ]')
    ssh-keyscan -T 5 -t rsa,ed25519 ${addrs} 2>/dev/null | grep -v '^#'

# --- github.com's *host* keys, and which boxes can reach github -----------------
#
# `just known-hosts` above is the network's own machines; this is the other half of the
# same DATA table (`common/ssh-known-hosts.nix`), and it is what makes a bare `git pull
# origin` on a box get as far as GitHub at all: until github.com is in that table, the
# only thing that ever accepted it is whichever `~/.ssh/known_hosts` someone ssh'ed from
# by hand, which is the mutable-file shape a deploy cannot reproduce and a reinstall loses
# silently (measured: `Host key verification failed` on services3's bare `git ls-remote
# origin`, *before* GitHub was asked — docs/nix-dev-workflow.md).
#
# It prints two lists rather than one, and the verdict on their agreement, because a
# host key copied out of a scan alone is exactly the thing that table exists to avoid: GitHub
# *publishes* its keys (`api.github.com/meta`'s `ssh_keys`), so the honest paste is the
# lines the scan and the published list agree on.  Paste those into the table and deploy;
# the rendered file to read back is `/etc/ssh/ssh_known_hosts` (rule 21 — read the rendered
# file, never evaluate).
#
# Safe on gw: it sshes and curls, it never evaluates a machine or builds.
github-host-keys:
    #!/usr/bin/env bash
    set -euo pipefail
    scan=$(ssh-keyscan -T 10 github.com 2>/dev/null | grep -v '^#' || true)
    published=$(curl -fsS -m 15 -H 'Accept: application/vnd.github+json' \
        https://api.github.com/meta 2>/dev/null \
        | sed -n 's/.*\(\(ssh-[a-z0-9-]*\|ecdsa-sha2-nistp[0-9]*\) [A-Za-z0-9+/=]*\).*/github.com \1/p' || true)
    echo "scanned from this box's network ($(printf '%s\n' "$scan" | grep -c . ) lines):"
    printf '%s\n' "$scan"
    echo "published by GitHub ($(printf '%s\n' "$published" | grep -c . ) lines):"
    printf '%s\n' "$published"
    if [ -z "$scan" ]; then
        echo "the scan got nothing: github.com is not reachable over this box's uplink — scan it where it is, pin what GitHub publishes"
    elif diff <(printf '%s\n' "$scan" | sort) <(printf '%s\n' "$published" | sort) >/dev/null 2>&1; then
        echo "agree — paste these lines into common/ssh-known-hosts.nix, then deploy"
    else
        echo "DISAGREE — paste only GitHub's published lines and write down which differ (a rotation, not a mystery)"
    fi

# Which boxes can put a commit on GitHub themselves, and which need a relay.  One fact
# per box that no flake file can hold: the *credential* a box pushes with is runtime
# state in that box's `~/.ssh`, not config — this pinned nixpkgs has no option that
# writes a home file (`common/users.nix` says so), so the fleet's per-box GitHub keys are
# installed the way the hop key is (`machines/services3/README.md` is the worked example:
# `ssh-keygen` on the box + the public half registered as a repo-scoped deploy key, and
# why a deploy key rather than an account key: an account key on a bench box can reach
# every private repo the account can, including the one whose history carries the cameras'
# PMKs).  `git ls-remote origin HEAD` is the question that answers it, and a box that
# cannot reach GitHub is a box whose `git status -sb` reports a **stale** `origin/main`
# ref — an `ahead N` there is not an unpushed commit (`docs/network-upgrade-runbook.md`).
#
# Read `hostname` first (rule 27); it hops, so it is right typed on any box, and it
# unsets `SSH_AUTH_SOCK` for every hop because a dead agent socket makes a LAN ssh *hang*
# rather than fail (docs/nix-dev-workflow.md).
repo-status:
    #!/usr/bin/env bash
    set -euo pipefail
    addrs=$(nix eval --impure --raw --expr 'let h = import ./common/hosts.nix; in builtins.concatStringsSep " " [ h.router h.services1 h.services2 h.services3 h.aibox ]')
    for a in $addrs; do
        timeout 30 ssh -o BatchMode=yes -o ConnectTimeout=5 "leigh-admin@$a" \
            'cd ~/Projects/infrastructure-nix-flake 2>/dev/null || exit 1
             unset SSH_AUTH_SOCK
             rev=$(timeout 15 git ls-remote origin HEAD 2>/dev/null | cut -c-8 || true)
             printf "%-9s head=%s dirty=%s remotes=%s tracks=%s origin=%s\n" \
               "$(hostname)" "$(git log -1 --format=%h)" "$(git status --short | wc -l)" \
               "$(git config --get-regexp "^remote\..*\.url$" | cut -d. -f2 | paste -sd, -)" \
               "$(git status -sb | head -1 | cut -c4-)" \
               "${rev:+reaches}${rev:-NO-CREDENTIAL}"' \
            || echo "$a unreachable (or has no checkout)"
    done
