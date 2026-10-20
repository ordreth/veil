{ pkgs ? import <nixpkgs> {} }:

# The same shell as the flake's `devShells.default`, for `nix-shell`.
let
  veil = pkgs.callPackage ./nix/package.nix { };
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    pkg-config
    rustc
    cargo
    clippy
    rustfmt
    rust-analyzer
    cargo-deny
    cargo-about
  ];

  inherit (veil) buildInputs;

  shellHook = ''
    export GST_PLUGIN_SYSTEM_PATH_1_0="${veil.gstPluginPath}"
  '';
}
