# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

{
  hive,
  nixosConfigurations ? { },
}:
let
  module = import ./module;

  mergedHive = {
    meta = { };

    defaults = { };
  }
  // hive
  # Map nixosConfigurations into nodes
  // (builtins.mapAttrs (name: value: {
    imports =
      value._module.args.modules
      # Include any custom stuff within `colmena`
      ++ [ hive.${name} or { } ];
  }) nixosConfigurations);

  nodeNames = builtins.filter (
    name:
    !builtins.elem name [
      "meta"
      "defaults"
      "-"
    ]
  ) (builtins.filter (name: builtins.hasAttr name hive) (builtins.attrNames mergedHive));

  resolveNixpkgs =
    value: help:
    # support `<nixpkgs>`
    if builtins.isPath value then
      import value { }
    # support npins sources passed directly
    else if value ? "outPath" then
      import value { }
    # support `import <nixpkgs> { }`
    else if builtins.isAttrs value then
      value
    else
      builtins.abort "${help} was not a path, { outPath, .. }, or attrset. Was type: ${builtins.typeOf value}";

  hiveGlobalNixpkgs =
    if mergedHive.meta ? "nixpkgs" then
      (resolveNixpkgs mergedHive.meta.nixpkgs "meta.nixpkgs")
    else
      builtins.abort "makeHive called without meta.nixpkgs specified.";

  getNodeNixpkgs =
    name:
    if mergedHive.meta ? "nodeNixpkgs" then
      if mergedHive.meta.nodeNixpkgs ? "${name}" then
        (resolveNixpkgs mergedHive.meta.nodeNixpkgs.${name} "meta.nodeNixpkgs.${name}")
      else
        hiveGlobalNixpkgs
    else
      hiveGlobalNixpkgs;

  nixpkgsIsFlake = nixpkgs: nixpkgs.lib.hasSuffix "-source" nixpkgs.path;

  evaluateNode =
    name:
    let
      nixpkgs = getNodeNixpkgs name;
      evalConfig = import (nixpkgs.path + "/nixos/lib/eval-config.nix");
    in
    evalConfig {
      modules = [
        module

        mergedHive.defaults
        mergedHive.${name}
      ]
      ++ (nixpkgs.lib.optional (nixpkgsIsFlake nixpkgs) {
        config.nixpkgs.flake.source = nixpkgs.lib.mkDefault nixpkgs.path;
      });
      system = null;
      specialArgs = {
        inherit name nodes;
      }
      // mergedHive.meta.specialArgs or { };
    };

  nodes = builtins.listToAttrs (
    map (name: {
      inherit name;
      value = evaluateNode name;
    }) nodeNames
  );

  getTopLevel = node: (evaluateNode node).config.system.build.toplevel.drvPath;
in
rec {
  inherit nodes;

  topLevels = builtins.mapAttrs (name: _: getTopLevel name) nodes;
  inspect = {
    _schema = 1;

    nodes = builtins.mapAttrs (_: v: v.config.deployment) nodes;
  };
}
