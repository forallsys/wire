# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

{
  lib,
  name,
  ...
}:
let
  inherit (lib) types;
in
{
  imports =
    let
      inherit (lib) mkAliasOptionModule;
    in
    [
      (mkAliasOptionModule [ "deployment" "targetHost" ] [ "deployment" "target" "hosts" ])
      (mkAliasOptionModule [ "deployment" "targetUser" ] [ "deployment" "target" "user" ])
      (mkAliasOptionModule [ "deployment" "targetPort" ] [ "deployment" "target" "port" ])
    ];

  options.deployment = {
    target = lib.mkOption {
      type = types.submodule {
        imports = [
          (lib.mkAliasOptionModule [ "host" ] [ "hosts" ])
        ];
        options = {
          hosts = lib.mkOption {
            type = types.coercedTo types.str lib.singleton (types.listOf types.str);
            description = "IPs or hostnames to attempt to connect to. They are tried in order.";
            default = lib.singleton name;
            apply = lib.unique;
          };
          user = lib.mkOption {
            type = types.str;
            description = "User to use for SSH. The user must be atleast `wheel` and must use an SSH key or similar
            non-interactive login method. More information can be found at https://wire.forall.systems/guides/non-root-user";
            default = "root";
          };
          port = lib.mkOption {
            type = types.int;
            default = 22;
            description = "SSH port to use.";
          };
        };
      };
      description = "Describes the target for this node";
      default = { };
    };

    buildOnTarget = lib.mkOption {
      type = types.bool;
      default = false;
      description = "Whether to build the system on the target host or not.";
    };

    allowLocalDeployment = lib.mkOption {
      type = types.bool;
      default = true;
      description = "Whether to allow or deny this node being applied to localhost when the host's hostname matches the
      node's name.";
    };

    tags = lib.mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Tags for node.";
      example = [
        "arm"
        "cloud"
      ];
    };

    privilegeEscalationCommand = lib.mkOption {
      type = types.listOf types.str;
      description = "Command to elevate.";
      default = [
        "sudo"
        "--"
      ];
    };

    replaceUnknownProfiles = lib.mkOption {
      type = types.bool;
      description = "No-op, colmena compatibility";
      default = true;
    };

    sshOptions = lib.mkOption {
      type = types.listOf types.str;
      description = "No-op, colmena compatibility";
      default = [ ];
    };

    _keys = lib.mkOption {
      internal = true;
      readOnly = true;
    };

    _hostPlatform = lib.mkOption {
      internal = true;
      readOnly = true;
    };

    keys = lib.mkOption {
      type = types.attrsOf (
        types.submodule (
          {
            name,
            config,
            ...
          }:
          {
            imports =
              let
                inherit (lib) mkAliasOptionModule;
              in
              [
                (mkAliasOptionModule [ "keyFile" ] [ "source" ])
                (mkAliasOptionModule [ "keyCommand" ] [ "source" ])
                (mkAliasOptionModule [ "text" ] [ "source" ])
              ];
            options = {
              name = lib.mkOption {
                type = types.str;
                default = name;
                description = "Filename of the secret.";
              };
              destDir = lib.mkOption {
                type = types.path;
                default = "/run/keys/";
                description = "Destination directory for the secret. Change this to something other than `/run/keys/` for keys to persist past reboots.";
              };
              path = lib.mkOption {
                internal = true;
                type = types.path;
                default =
                  if lib.hasSuffix "/" config.destDir then
                    "${config.destDir}${config.name}"
                  else
                    "${config.destDir}/${config.name}";
                description = "Path that the key is deployed to.";
              };
              service = lib.mkOption {
                internal = true;
                type = types.str;
                default = "${config.name}-key.service";
                description = "Name of the systemd service that represents this key.";
              };
              group = lib.mkOption {
                type = types.str;
                default = "root";
                description = "Group to own the key. If this group does not exist this will silently fail and the key will be owned by gid 0.";
              };
              user = lib.mkOption {
                type = types.str;
                default = "root";
                description = "User to own the key. If this user does not exist this will silently fail and the key will be owned by uid 0.";
              };
              permissions = lib.mkOption {
                type = types.str;
                default = "0600";
                description = "Unix Octal permissions, in string format, for the key.";
              };
              source = lib.mkOption {
                type = types.oneOf [
                  types.str
                  types.path
                  (types.listOf types.str)
                ];
                description = "Source of the key. Either a path to a file, a literal string, or a command to generate the key.";
              };
              uploadAt = lib.mkOption {
                type = types.enum [
                  "pre-activation"
                  "post-activation"
                ];
                default = "pre-activation";
                description = "When to upload the key. Either `pre-activation` or `post-activation`.";
              };
              environment = lib.mkOption {
                type = types.attrsOf types.str;
                default = { };
                description = "Key-Value environment variables to use when creating the key if the key source is a command.";
              };
            };
          }
        )
      );
      description = "Secrets to be deployed to the node.";
      default = { };
      example = {
        "wireless.env" = {
          source = [
            "gpg"
            "--decrypt"
            "secrets/wireless.env.gpg"
          ];
          destDir = "/etc/keys/";
        };

        "arbfile.txt" = {
          source = ./arbfile.txt;
          destDir = "/etc/arbs/";
        };

        "arberfile.txt" = {
          source = ''
            Hello World
          '';
          destDir = "/etc/arbs/";
        };
      };
    };
  };
}
