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

      services = lib.mapAttrs' (
        _name: value:
        lib.nameValuePair "${value.name}-key" {
          description = "Service that requires ${value.path}";
          path = [
            pkgs.inotify-tools
            pkgs.coreutils
            pkgs.systemd
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
            AssertPathExists = value.path;
          };
          serviceConfig = {
            Type = "simple";
            Restart = "no";
            NotifyAccess = "all";
            RemainAfterExit = "yes";
          };
        }
      ) config.deployment.keys;
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
