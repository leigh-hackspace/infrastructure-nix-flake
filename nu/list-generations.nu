#!/usr/bin/env nu

let good_symlink = "/nix/var/nix/profiles/system-good"
let current = readlink "/run/current-system"
let booted = readlink "/run/booted-system"

# Attempt to resolve the "good" symlink if it exists, otherwise just use "/" which will always show as false
let good = if ($good_symlink | path type) == "symlink" {
    readlink $good_symlink
} else {
    "/"
}

ls /nix/var/nix/profiles | where { 
    ($in.name | path type) == "symlink" and ($in.name | str contains "link")
} | each { { 
    Generation: ($in.name | split row '-').1, 
    Date:       $in.modified,
    Good:       (if ((readlink $in.name) == $good)      { "✅" } else { "❌" })
    Current:    (if ((readlink $in.name) == $current)   { "✅" } else { "❌" })
    Booted:     (if ((readlink $in.name) == $booted)    { "✅" } else { "❌" })
} }
