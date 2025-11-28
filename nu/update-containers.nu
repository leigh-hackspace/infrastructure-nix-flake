#!/usr/bin/env nu

# Test with
# sudo podman tag 35f50ba5c110 docker.io/library/eclipse-mosquitto:latest

# let PODMAN = "podman"
# let SYSTEMCTL = "systemctl"
# let CURL = "curl"
# let SLACK_URL = "https://hooks.slack.com/services/blah/blah/blah"

let PODMAN = "@PODMAN@"
let SYSTEMCTL = "@SYSTEMCTL@"
let CURL = "@CURL@"
let SLACK_URL = "@SLACK_URL@"

let containers: list<record> = ^$PODMAN ps --format=json | from json | select Names.0 Image Status

mut notification_list: list<string> = []

for container in $containers {
    let old_digest = get_digest $container.Image
    
    print $"(ansi bo)Updating ($container.Image) - ($old_digest)(ansi reset)"

    ^$PODMAN pull $container.Image # err> /dev/null

    let new_digest = get_digest $container.Image

    if $old_digest != $new_digest {
        ^$SYSTEMCTL restart $"podman-($container.'Names.0')"

        print $"(ansi bo)Updated ($old_digest) => ($new_digest)(ansi reset)"

        $notification_list = $notification_list | append $"🔄 Updated \"($container.Image)\" - ($old_digest) => ($new_digest)";
    } else {
        print $"(ansi bo)Unchanged(ansi reset)"
    }

    print ""
}

print $notification_list

if ($notification_list | length) > 0 {
    let data = { channel: "#infra-alerts", text: ($notification_list | str join "\n") } | to json

    ^$CURL -X POST -H 'Content-type: application/json' --data $data $SLACK_URL
}

def get_digest [image: string] {
  ^$PODMAN inspect $image | from json | get 0.Digest | str replace "sha256:" "" | str substring 0..12
}
