{
  lib,
  nixosOptionsDoc,
  runCommand,
  ...
}:
let
  eval = lib.evalModules {
    modules = [
      ../runtime/module/options.nix
      {
        options._module.args = lib.mkOption {
          internal = true;
        };
      }
    ];
    specialArgs = {
      name = "‹node name›";
      nodes = { };
    };
  };

  options = nixosOptionsDoc {
    inherit (eval) options;
  };
in
runCommand "options-doc.md" { } ''
  cat ${options.optionsCommonMark} > $out
  sed -i -e '/\*Declared by:\*/,+1d' $out
''
