{
  callPackage,
  wire-small-dev,
  nix,
  nodejs,
  pnpmConfigHook,
  fetchPnpmDeps,
  pnpm,
  stdenv,
  mode ? "unstable",
  ...
}:
let
  optionsDoc = callPackage ./options.nix { };
  pkg = builtins.fromJSON (builtins.readFile ./package.json);
in
stdenv.mkDerivation (finalAttrs: {
  inherit (pkg) version;
  pname = pkg.name;
  nativeBuildInputs = [
    wire-small-dev
    nodejs
    pnpmConfigHook
    pnpm
    nix
  ];
  src = ./.;
  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs) pname version src;
    fetcherVersion = 4;
    hash = "sha256-mtSmorB2h/foe3X4yOsoTX/4fAw/bOCZIcUdIR34eXs=";
  };
  patchPhase = ''
    cat ${optionsDoc} >> ./reference/module.md
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
