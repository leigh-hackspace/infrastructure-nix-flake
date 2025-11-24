{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  CONFIG = import ./config.nix;
  slackWebhookUrl = lib.strings.trim (builtins.readFile CONFIG.SLACK_URL_FILE);
  update-containers = (
    pkgs.writeShellScriptBin "update-containers" ''
      set -euo pipefail

      # Temporary file to store update results
      updates_file=$(mktemp)
      trap "rm -f $updates_file" EXIT

      echo "Starting container update check..."

      # Get all running containers with their images
      containers=$(${pkgs.podman}/bin/podman ps -a --format="{{.Names}}|{{.Image}}" | sort)

      updated_containers=()

      while IFS='|' read -r container_name image; do
        echo "Checking $container_name ($image)..."
        
        # Get the current image ID that the tag points to
        old_image_id=$(${pkgs.podman}/bin/podman inspect "$image" --format='{{.Id}}' 2>/dev/null || echo "")
        
        # Pull the latest image
        if ${pkgs.podman}/bin/podman pull "$image" 2>&1; then
          # Get the new image ID after pulling
          new_image_id=$(${pkgs.podman}/bin/podman inspect "$image" --format='{{.Id}}' 2>/dev/null || echo "")
          
          # Check if the image actually changed
          if [ "$old_image_id" != "$new_image_id" ] && [ -n "$new_image_id" ] && [ -n "$old_image_id" ]; then
            echo "✓ $container_name: Image updated from $old_image_id to $new_image_id"
            
            # Get digest (SHA256) for both versions - this is the actual unique identifier
            old_digest=$(${pkgs.podman}/bin/podman inspect "$old_image_id" --format='{{index .RepoDigests 0}}' 2>/dev/null | sed 's/.*@sha256://' | cut -c1-12)
            new_digest=$(${pkgs.podman}/bin/podman inspect "$new_image_id" --format='{{index .RepoDigests 0}}' 2>/dev/null | sed 's/.*@sha256://' | cut -c1-12)
            
            # Fallback to image ID if digest isn't available
            [ -z "$old_digest" ] && old_digest=$(echo "$old_image_id" | sed 's/sha256://' | cut -c1-12)
            [ -z "$new_digest" ] && new_digest=$(echo "$new_image_id" | sed 's/sha256://' | cut -c1-12)
            
            # Get creation dates for additional context
            old_date=$(${pkgs.podman}/bin/podman inspect "$old_image_id" --format='{{.Created}}' 2>/dev/null | cut -d'T' -f1)
            new_date=$(${pkgs.podman}/bin/podman inspect "$new_image_id" --format='{{.Created}}' 2>/dev/null | cut -d'T' -f1)
            
            # Store update info
            echo "$container_name|$image|$old_digest|$new_digest|$old_date|$new_date" >> "$updates_file"
            updated_containers+=("$container_name")
            
            # Restart only this container's service
            service_name="podman-$container_name"
            if ${pkgs.systemd}/bin/systemctl list-units --all | grep -q "$service_name"; then
              echo "Restarting $service_name..."
              ${pkgs.systemd}/bin/systemctl restart "$service_name"
            fi
          else
            echo "○ $container_name: Already up to date"
          fi
        else
          echo "✗ $container_name: Failed to pull image"
        fi
      done <<< "$containers"

      # Send Slack notification if any containers were updated
      if [ -s "$updates_file" ]; then
        notification_text="🔄 *Container Updates Applied*\n\n"
        
        while IFS='|' read -r name image old_digest new_digest old_date new_date; do
          notification_text="$notification_text• *$name* (\`$image\`)\n"
          notification_text="$notification_text  \`$old_digest\` ($old_date) → \`$new_digest\` ($new_date)\n"
        done < "$updates_file"
        
        notification_text="$notification_text\n_Updated on $(${lib.getExe pkgs.hostname-debian}) at $(date)_"
        
        # Send to Slack
        ${pkgs.curl}/bin/curl -X POST \
          -H 'Content-type: application/json' \
          --data "{\"channel\":\"#infra-alerts\",\"text\":\"$notification_text\"}" \
          "${slackWebhookUrl}" || echo "Failed to send Slack notification"
        
        echo ""
        echo "Summary: Updated ''${#updated_containers[@]} container(s)"
      else
        echo ""
        echo "No containers needed updates"
        
        # Optional: Send a "no updates" notification
        # Uncomment if you want to be notified even when nothing updates
        # ${pkgs.curl}/bin/curl -X POST \
        #   -H 'Content-type: application/json' \
        #   --data "{\"channel\":\"#infra-alerts\",\"text\":\"✓ Container update check completed on $(${lib.getExe pkgs.hostname-debian}) - no updates needed\"}" \
        #   "${slackWebhookUrl}"
      fi
    ''
  );
in
{
  environment.systemPackages = [
    update-containers
  ];

  systemd.timers = {
    update-containers = {
      timerConfig = {
        Unit = "update-containers.service";
        OnCalendar = "*-*-* 02:00:00"; # Run everyday at 2am
        Persistent = true; # Run missed timers on boot
      };
      wantedBy = [ "timers.target" ];
    };
  };

  systemd.services = {
    update-containers = {
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${update-containers}/bin/update-containers";
      };
      # Add these for better logging
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
    };
  };

  virtualisation.podman = {
    enable = true;
    autoPrune.enable = true;
    dockerCompat = true;
    dockerSocket.enable = true;
    defaultNetwork.settings.dns_enabled = true;
  };

  virtualisation.oci-containers.backend = "podman";
}
