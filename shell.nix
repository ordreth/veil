{ pkgs ? import <nixpkgs> {} }:

# The same shell as the flake's `devShells.headless`, for `nix-shell`: the
# build and its tools, and a compositor to run veil in, for the headless sway
# of `tests/headless.rs` and `scripts/bench/`.
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

  # The headless sway, the screenshot and client tools the tests drive, what
  # the benchmark needs to make its clips (ffmpeg) and read the daemon's
  # answers (jq), and the linters of the scripts and workflows.
  packages = with pkgs; [
    sway
    grim
    foot
    swaylock
    jq
    ffmpeg-headless
    shellcheck
    actionlint
  ];

  # The test windows (foot) need a font, wherever the shell runs.
  FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };

  shellHook = ''
    export GST_PLUGIN_SYSTEM_PATH_1_0="${veil.gstPluginPath}"
  '';
}
