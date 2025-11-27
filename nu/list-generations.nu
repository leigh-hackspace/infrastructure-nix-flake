#!/usr/bin/env nu

ls /nix/var/nix/profiles | where { 
    ($in.name | path type) == "symlink" and ($in.name | str contains "link")
} | each { { 
    Generation: ($in.name | split row '-').1, 
    Date:       $in.modified,
    Good:       (if ((readlink $in.name) == (readlink "/nix/var/nix/profiles/system-good")) { "✅" } else { "❌" })
    Current:    (if ((readlink $in.name) == (readlink "/run/current-system")) { "✅" } else { "❌" })
    Booted:     (if ((readlink $in.name) == (readlink "/run/booted-system")) { "✅" } else { "❌" })
} }