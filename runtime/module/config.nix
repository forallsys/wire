# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

{
  pkgs,
  lib,
  config,
  ...
}:
{
  config = {
    system.activationScripts.setup-wire-rollback.text = ''
      mkdir -p /var/lib/wire-rollback
      chmod 700 /var/lib/wire-rollback
    '';

    systemd = {
      paths = lib.mapAttrs' (
        _name: value:
        lib.nameValuePair "${value.name}-key" {
          description = "Monitor changes to ${value.path}. You should Require ${value.service} instead of this.";
          pathConfig = {
            PathExists = value.path;
            PathChanged = value.path;
            Unit = "${value.name}-key.service";
          };
        }
      ) config.deployment.keys;

      services = {
        wire-rollback = {
          enable = false;
          description = "Rolls back if `/var/lib/wire-rollback/heartbeat` is not created in 30 seconds after this service starts.";
          documentation = [
            "https://wire.althaea.zone/guides/rollback"
          ];
          path = [
            pkgs.coreutils
            config.nix.package
          ];
          wantedBy = [ "multi-user.target" ];
          script = ''
            set -euo pipefail

            goal=$(<"/var/lib/wire-rollback/goal")

            case $goal in
                "check" | "switch" | "boot" | "test" | "dry-activate")
                    echo "<5>using goal $goal"
                    ;;
                *)
                    echo "<3>'$goal' is not a valid goal."
                    exit 1
                    ;;
            esac

            sleep 30

            if [ -f "/var/lib/wire-rollback/heartbeat" ]; then
                exit 0
            fi

            echo "<1>/var/lib/wire-rollback/heartbeat does not exist, rolling back system"

            ls -la /nix/var/nix/profiles

            # set current system
            nix-env --rollback --profile /nix/var/nix/profiles/system
            # get the path to the system we are now rolling back to
            system=$(readlink -f /nix/var/nix/profiles/system)

            echo "<5>rolling back to $system"

            # switch to the system using goal
            "$system/bin/switch-to-configuration $goal"
          '';
          unitConfig = {
            ConditionPathExists = [
              "/var/lib/wire-rollback/goal"
              "!/var/lib/wire-rollback/heartbeat"
            ];
          };
          serviceConfig = {
            Type = "oneshot";
            Restart = "no";
            StateDirectory = "wire-rollback";
            NotifyAccess = "all";
            RemainAfterExit = "yes";

            ExecStopPost = "${pkgs.coreutils}/bin/rm -f /var/lib/wire-rollback/goal";
          };
        };
      }
      // (lib.mapAttrs' (
        _name: value:
        lib.nameValuePair "${value.name}-key" {
          description = "Service that requires ${value.path}";
          path = [
            pkgs.inotify-tools
            pkgs.coreutils
          ];
          script = ''
            MSG="Key ${value.path} exists."
            systemd-notify --ready --status="$MSG"

            echo "waiting to fail if the key is removed..."

            while inotifywait -e delete_self "${value.path}"; do
              MSG="Key ${value.path} no longer exists."

              systemd-notify --status="$MSG"
              echo $MSG

              exit 1
            done
          '';
          unitConfig = {
            ConditionPathExists = value.path;
          };
          serviceConfig = {
            Type = "simple";
            Restart = "no";
            NotifyAccess = "all";
            RemainAfterExit = "yes";
          };
        }
      ) config.deployment.keys);
    };

    deployment = {
      _keys = lib.mapAttrsToList (
        _: value:
        value
        // {
          source = {
            # Attach type to internally tag serde enum
            t = builtins.replaceStrings [ "path" "string" "list" ] [ "Path" "String" "Command" ] (
              builtins.typeOf value.source
            );
            c = value.source;
          };
        }
      ) config.deployment.keys;

      _hostPlatform = config.nixpkgs.hostPlatform.system;
    };
  };
}
