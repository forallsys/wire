{
  lib,
  nixosOptionsDoc,
  runCommand,
  wire-small-dev,
  wire-diagnostics-md,
  nix,
  nodejs,
  pnpm,
  stdenv,
  mode ? "unstable",
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

  optionsMd =
    (nixosOptionsDoc {
      inherit (eval) options;
    }).optionsCommonMark;

  optionsDoc = runCommand "options-doc.md" { } ''
    cat ${optionsMd} > $out
    sed -i -e '/\*Declared by:\*/,+1d' $out
  '';

  pkg = builtins.fromJSON (builtins.readFile ./package.json);
in
stdenv.mkDerivation (finalAttrs: {
  inherit (pkg) version;
  pname = pkg.name;
  nativeBuildInputs = [
    wire-small-dev
    nodejs
    pnpm.configHook
    nix
  ];
  src = ./.;
  pnpmDeps = pnpm.fetchDeps {
    inherit (finalAttrs) pname version src;
    fetcherVersion = 1;
    hash = "sha256-fZOSXnl5wtvzU5IXpTKQkDwkKwnGfjiv+4HsECon4kc=";
  };
  patchPhase = ''
    cat ${optionsDoc} >> ./reference/module.md
    cat ${wire-diagnostics-md} >> ./reference/errors.md
    wire inspect --markdown-help > ./reference/cli.md
  '';
  buildPhase = "pnpm run build > build.log 2>&1";
  installPhase = "cp .vitepress/dist -r $out";
  doCheck = true;
  checkPhase = ''
    nix-instantiate --eval --strict ./snippets > /dev/null
  '';
  DEBUG = "*";
  MODE = mode;
})
