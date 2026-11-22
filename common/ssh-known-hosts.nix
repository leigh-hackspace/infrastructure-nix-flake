# DATA — attrset of the *host* keys a box must have pinned before any ssh it runs can
# succeed: the network's own machines, plus GitHub.  Read by `import` from
# `./build-farm.nix` and handed to `programs.ssh.knownHosts`.
#
# Why a table of host keys is in the flake at all: the remote-build hop is run by
# the **nix daemon**, i.e. as *root*, whose HOME is `/root`.  The operator's
# `~leigh-admin/.ssh/known_hosts` — the file that makes a manual
# `ssh -i ~/.ssh/agent-hop-key 10.3.1.32` work — is invisible to that ssh, so a
# delegating box whose root shell has never ssh'ed to the builder dies seconds
# in with `Host key verification failed` / `Failed to find a machine for remote
# build!` while every manual hop succeeds.  `programs.ssh.knownHosts` writes
# `/etc/ssh/ssh_known_hosts`, which *every* ssh reads through the
# `GlobalKnownHostsFile` line NixOS puts in `/etc/ssh/ssh_config`, so the hop
# depends on a store-pinned table rather than on whichever root shells happen to
# have visited the builder (which is exactly the state services1 is in today: its
# `/root/.ssh/known_hosts` is hand-made and no deploy can reproduce it).  Worth
# knowing before you try to point one ssh at another file: OpenSSH 10.5 has
# dropped `KnownHostsFile` as a `-o` option altogether, so `-o KnownHostsFile=…`
# is a hard error and the generated config file is the only lever.
#
# Pinned, on purpose: a machine that regenerates its host keys (a reinstall, a key
# rotation) now *fails* the hop until these lines are regenerated, so accepting a
# re-installed box is a deliberate edit rather than `StrictHostKeyChecking=accept-new`
# re-trusting whatever answers an address — and the router's own keys are pinned for
# the same reason.  Regenerate with
# `just known-hosts`, paste, deploy; keep all two key types per machine (a box
# offering only one still has to be recognised by the type it presents).
#
# GitHub is in this table for the same reason, and the reason it *had* to be is the
# reason it was missing until 2026-11-23: a box's own bare `git pull origin` died at
# `Host key verification failed` before it ever asked GitHub, because the only thing
# that had ever accepted github.com was whichever `~/.ssh/known_hosts` someone had
# ssh'ed from by hand — the mutable half this file exists to replace, and one a
# reinstall loses silently.  Pinning github.com makes the *host* half of that hop
# identical on every box, so the only remaining failure is the *identity* half, which
# is per box by construction: a box reaches GitHub only if it holds a credential of
# its own (`machines/services3/README.md` is the worked example — a per-box `~/.ssh/id_ed25519`
# registered as a repo-scoped deploy key — and `docs/follow-ups.md` keeps the boxes that
# still have none).  Regenerate with `just github-host-keys`, which prints the scanned
# lines and GitHub's own published list, and paste only where the two agree (the
# cross-check this file's services3 entry applies to a scanned *machine* key, from the
# other side).  Only `github.com` is here: nix's `github:` flake inputs and `gh api`
# go over HTTPS (curl, not ssh), and nothing on this network ssh'es to `api.github.com`.
let
  HOSTS = import ./hosts.nix;
in {
  "router-rsa" = {
    hostNames = [HOSTS.router];
    publicKey = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQDolwIGnLdCI8U9wHxiKxxJBxBRdsSTyDBc9rjSUl2wwUVB3C8tGcmCxKTQGciB9pt9Zl70dgG2QsoNUl2uNXkIfN4tTory1HJsq3jaJVMEwuuw9B/g7iLH3UXrvQLYScsz01pwWZqbdtuyxklitgG1OcW7JTJ8BZbBFcZEyi/FXwlF8JlsyU7iY24c2PgAJpX0n7fXExm82dFMy7A0aejlc7vrDrHcHixSbRN5QpWfXtrWaZYvFJkvf2x+EvT36RgxKmorKtpeSqvC5eksCRBaaIanafr9gv3WsSc8IFvItuVusFK2IsyyO8TQhVjZ9KqZ5973DWhF3rLMCU7vbR2+sPTuJNURrv8mSl4K0ivKJxyDD5mCnFIbY9pwXSeqO7NogMU5qovB4O5aTBLopAdUxEg8Bzh7nAHzIxPM1mUduYl504ZAgEkGOrLg02CCssq5TIiul8Zmwr9bcqOYadfwGguMuZfp/d6nwxbhdE00QHYquKTLSBVdz4OPbsHOSq4DyGtwDsv55psGDdAp619ThFWK1kbFF2AZ0lf7FwNKPad8d178CL0VnJF8rgJGXdDXPDW6QK07P/tJTjj+9Lp6aejXeICAq8CwJMlILTBfoZ4MQTK867EjBXXIwrc5Semwvzm2k2xxo/M+F1P3MtXw5mFM+EsX1VBMkePWrPRltw==";
  };
  "router-ed25519" = {
    hostNames = [HOSTS.router];
    publicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGJtLDxvMHNXZqciSHB2KsBqsaO4iyp0yo6t9LEp9u5S";
  };
  "services1-rsa" = {
    hostNames = [HOSTS.services1];
    publicKey = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQC4agk7ufrgg+mYTXpnXC4LSbonM+KtJ0hfjLBQnNMIBi3xzHJtdbVyTV58Ygo3i2t156wRFfrfedcUByom9YyuGLb1IjC78eMl88GK7yyY54/wQe+OpRTTq6I6iaL70frADYtnyuLzALQVmnjs09OBAmjYVnMa1w0O3qmnpI2INN0cG1H73yzo6KXdx4fzm5dT+PhUgNXQ4Svu51041OX0WGWQc+T6VaVI+RyMvOtvJjb/v+SGyfHffLlcq15uo1SIod4QabP+V5kyHhT9WBFnb2ZoI5y9va/zFlZjUTOooEEsXWcvxV1+LufOK+/0RiQbMRTcmN5xdVYgN/Dabt4Ui4IRe819TUxAtRawD7krTwQEic7dPcpYu4ZedaPCiJKZza+KUTCm0g0btGJNw98JORkbqIjFFL5RiA+ck3HdsZxg5n4czgo7YnnaG7B3vXT7Sr4YrRB3Aj2kENc6jT1ggFpf/q3ArZ3BmrbyClggsAYU4fTR+AL2fpwxG9Dpm+TSV14JOpsK4FBTq4BTizcYgGn5zCjpPRgQvzadfoSrxD0d1Mhdzb4HcBC8A91wA2sokdYUSeJL/2y9zxc2rBCxNe2F5gfNNl/OpAYg9vnCych6OgcRIe5BheXA6054Q7i3hq+roczmg/40vbAJ6lUm0mVPXkTfxefAynmyXnTe5Q==";
  };
  "services1-ed25519" = {
    hostNames = [HOSTS.services1];
    publicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAmKZ71T12NH4I+T9XI9TFUqFxtyt0LYZmEmMj4ioSQT";
  };
  "services2-rsa" = {
    hostNames = [HOSTS.services2];
    publicKey = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQDb4AWexoTQPcwAXG5amoG4AeBfSiz3IduOQx4GAfV/ddbp4hpZFrD0qSD1nIsAVtdep7kdkfvCkHYl4AI26wjVVJlRNmDKS5TiWyJ4EmOcdbuid4nQex45S6D0leRcoI/JdutMAntHeTRBHYVlX1ZyffYsj1uwLCEyGN6ueQitwXuaUtrMKkK2DNNIC5CElBVTqc67Uf+ri5OYq9YPO2EFqURTzjhYw8ACtep20EZvA44QlMZuHM2mDmFc8d7uFl7yQvOm17nnTfK0WgYuqTlVZdwn0rT85pv0T91NcrryNYSObZ7y6kl+BGI2v5CKNVwF5h6KQCruIDIk9VVXH+2Jre+RzNhrjdiXKBQigI37yzQ+uYSX3i3wmEs2sJ0p7OjDaoz8ipuSnvHa41uo612coGu1SXPQaWFoM0hBXAg3nPR+SrT5yfv7b3GBGatFtYQFRPvSCh/cOGeJYeilyLshLGzXXjgmNPvV/EnL6b7twHZaEr5LT0bHNbrhg1aeChcOJ6uRf9wKQaBMCkNR0N9vb760gy9s7HJXwkFl54UVix/nzEipXlyIzND2yg4hvgH8eDdBHK1GGCP5vBJpbHbJZiX//kcDqp1yQ3+lNAJ1XcQLQM9Bg/m7sDFtEwdiG4jC28dsnl4r8AHVZuHTR6kTWgO45rRzL+DK43KNJw2rqw==";
  };
  "services2-ed25519" = {
    hostNames = [HOSTS.services2];
    publicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICw1DA6most5SuQgeSajkpQXpWIhcKGIYdN00PWpK0xl";
  };
  # services3, scanned 2026-11-11 with `just known-hosts` and cross-checked against
  # `/etc/ssh/ssh_host_*.pub` on the box itself (`sudo cat`, hashes agree), because a
  # pinned key copied from a scan alone is exactly the thing this file exists to avoid.
  # Their comment field still reads `root@hp-z240-xeon-1240v6-nixos` — the name the
  # box had when these keys were generated in Aug 2025, three renames ago.  A comment is
  # not an identity; the bytes are the identity, and they are unchanged by the box being
  # renamed to services3, which is why the hop works before and after the first switch.
  "services3-rsa" = {
    hostNames = [HOSTS.services3];
    publicKey = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQC0BfNrOnoOrrrWSrWcsRhzmUMd2B620VeB9wxppvPBjZr8c64oKMOVo0PQsf5fagO6WCmoDVPamkUPSdJkLye2sZ3Bj8lQYK63Srr/tL6uGam6gBRD5VV1tV0KDfrimesJQ4zuf4fAyJXc5CYzpaq3LfRsCnOMwKqzLn6J0QWqa3/RyJ4BJTfz+R9Zlk8CZ3s/bd+ZnCy544m3FYSfVU3jZ3Ver+k3CQG6y8aaDqmlcH59MgjY52YylXeDcVI+jF0dOZeWCzqCsWa3eRNtHZhrQETi3qpwG2KAky4aauuYOuJJF/6AByn5LPcJExPaSGz2wHe0GN7G0gQFMd1FNTkRZL2zvTuYeCM2TXoygd927SSUHA9+aV1Jq+5cGZW+vUj9t9tldMNMJTFN0TGthUP72m9YmtWcoMi1BBqgaW/MlgwhAIHjyu0VszHuWmO50rMPJeCe4hFFnR4ICYmbGPc3wNfCFt8GbUY1Lc4+FiNmM39jJXn4U5fWWrsnAqNG+ZAsMyrPjaqU0+RcGHRjTRypzQjTYHxWFvHN8QJG21GitxoeUOQL+t1CIDcnbW0G24bLrkrFeF1c66MlxKVNwNwYQH0+2V1WdXpvXSIepU00xGuEMpureYsgCY4YUUKRxIDpRwJwbQx3o19dLruESph3tBgA1lzVZ9kZgYbsYsTo5w==";
  };
  "services3-ed25519" = {
    hostNames = [HOSTS.services3];
    publicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICEUwlnQnO5m1rTpxH/VKCXL9VoaROvBRY4eQvwHyACO";
  };
  "aibox-rsa" = {
    hostNames = [HOSTS.aibox];
    publicKey = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQCam5Ma/6m7t/ZBwOLOnFUdbMguNYbt6KZnDHTC7q5hsb5wbyH1rXznOd6LiN+8xUM/9zK/+TVpD1gW0w2YoekmlH+3qQ8hM4dG3GInsr57kMeWK6yhGLtv6Y/b5O2yXoQI14BwD6CCVt6cciPPdzRyL9AonowkDOY0IllRFe62+lY7VXdlM7+tqWzlSrpFg6Byb8MsTXSCgeXfl249aCFjkYXyr3rD9CEvy6l+q0rmo19CYUMYxXasZ+4gttBFcQzaV/kzB3uzGDapPHmkvpcm9UzDirSJWeWq4Y+1YdrcsmKJ6M7u81MktpzD4XhiWzXmi/LD5gGRTDqC7+HaIG6krJDT1lrCOFhbB5N/dSYwrWx0z8CoOmNa23nqHZQBL8OgMeY/lPjrr7rKf2UtZ9jyj/YxwL4tIndHyGkJGAfUp/MfVyTExWwtLysujla5uKcMRtcQLPegVv98dAmAn87td2ifAodkZels9AJ1ZGIRySC0vItUkmyzJ+lFrNlm8F2uXu/+9JWn/hrvTs/DSWs1BvlNHQNgXB5HRa4PM4PQPMu7fVxaLUSWBbzSqSSV1kgLkt5cuNK0rlOor1oMTyQoN/pM/DYCG0QRIR8j1of770DWs9AgOrJFnF5imAhw1fjSaB9YXeVumNXGEpnSbEseaq7SkbCcoIGjp3AXEX9Bdw==";
  };
  "aibox-ed25519" = {
    hostNames = [HOSTS.aibox];
    publicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINqBPb161BuBRJTd5QvrIAdOEEvbPHmTpVmtUv592DHA";
  };
  # GitHub's own keys, not scanned-only: each was read off `api.github.com/meta`'s
  # `ssh_keys` on 2026-11-23 and cross-checked, byte for byte, against `ssh-keyscan
  # github.com` run from services1 over the network — the mirror of the rule the
  # services3 entry above applies to a scanned machine key, which is that a host key
  # copied out of a scan alone is exactly what this table exists to avoid.  Three types,
  # not two: GitHub presents whichever of its three it negotiates, and an ssh that has
  # one of them pinned is refused the moment it presents another.
  "github-com-ed25519" = {
    hostNames = ["github.com"];
    publicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
  };
  "github-com-ecdsa-p256" = {
    hostNames = ["github.com"];
    publicKey = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEmKSENjQEezOmxkZMy7opKgwFB9nkt5YRrYMjNuG5N87uRgg6CLrbo5wAdT/y6v0mKV0U2w0WZ2YB/++Tpockg=";
  };
  "github-com-rsa" = {
    hostNames = ["github.com"];
    publicKey = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCj7ndNxQowgcQnjshcLrqPEiiphnt+VTTvDP6mHBL9j1aNUkY4Ue1gvwnGLVlOhGeYrnZaMgRK6+PKCUXaDbC7qtbW8gIkhL7aGCsOr/C56SJMy/BCZfxd1nWzAOxSDPgVsmerOBYfNqltV9/hWCqBywINIR+5dIg6JTJ72pcEpEjcYgXkE2YEFXV1JHnsKgbLWNlhScqb2UmyRkQyytRLtL+38TGxkxCflmO+5Z8CSSNY7GidjMIZ7Q4zMjA2n1nGrlTDkzwDCsw+wqFPGQA179cnfGWOWRVruj16z6XyvxvjJwbz0wQZ75XK5tKSb7FNyeIEs4TT4jk+S4dhPeAUC5y+bDYirYgM4GC7uEnztnZyaVWQ7B381AK4Qdrwt51ZqExKbQpTUNn+EjqoTwvqNj4kqx5QUCI0ThS/YkOxJCXmPUWZbhjpCg56i+2aB6CmK2JGhn57K5mj0MNdBXA4/WnwH6XoPWJzK5Nyu2zB3nAZp+S5hpQs+p1vN1/wsjk=";
  };
}
