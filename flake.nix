{
  description = "Veil - Lightweight dynamic wallpaper daemon for Hyprland";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # Only for `devShells.msrv`: a Rust toolchain of an exact older version.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    {
      # `pkgs.veil`, built with the nixpkgs of whoever applies the overlay.
      overlays.default = final: prev: {
        veil = final.callPackage ./nix/package.nix { };
      };
    }
    // flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        # veil is source-available, so nixpkgs calls it unfree; allow it (and the
        # checks derived from it, whose names start with "veil")
        # and nothing else.
        pkgs = import nixpkgs {
          inherit system;
          config.allowUnfreePredicate = pkg: nixpkgs.lib.hasPrefix "veil" (nixpkgs.lib.getName pkg);
        };
        veil = pkgs.callPackage ./nix/package.nix { };

        # The oldest Rust veil builds with (`rust-version` in Cargo.toml).
        msrv = (pkgs.lib.importTOML ./Cargo.toml).package.rust-version;
        msrvToolchain = rust-overlay.packages.${system}."rust_${builtins.replaceStrings [ "." ] [ "_" ] (pkgs.lib.versions.majorMinor msrv)}_0";
      in
      {
        packages = {
          default = veil;
          inherit veil;
        };

        apps.default = {
          type = "app";
          program = "${veil}/bin/veil";
        };

        # `nix flake check`: the package (its build runs `cargo test`, with
        # the GStreamer plugins), clippy, rustfmt, and the plugin set the
        # wrapped binary is given.
        checks = {
          package = veil;

          clippy = veil.overrideAttrs (old: {
            pname = "veil-clippy";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
            buildPhase = ''
              runHook preBuild
              cargo clippy --offline --all-targets -- -D warnings
              runHook postBuild
            '';
            doCheck = false;
            installPhase = "touch $out";
            dontFixup = true;
          });

          fmt = pkgs.runCommand "veil-fmt" { nativeBuildInputs = [ pkgs.rustfmt ]; } ''
            cd ${veil.src}
            rustfmt --check --edition 2021 src/lib.rs src/main.rs tests/*.rs
            touch $out
          '';

          # The elements veil's pipelines use, looked up in the environment of
          # the wrapper: its own script, with `gst-inspect-1.0` in place of
          # the binary. Also checks where the registry goes by default, and
          # the allocator's arenas (a default the user can change).
          plugins = pkgs.runCommand "veil-plugins" { nativeBuildInputs = [ pkgs.gst_all_1.gstreamer ]; } ''
            sed 's|^exec .*|exec printenv MALLOC_ARENA_MAX|' ${veil}/bin/veil > arenas
            grep -q '^exec printenv' arenas
            chmod +x arenas
            test "$(env -u MALLOC_ARENA_MAX ./arenas)" = 1
            test "$(MALLOC_ARENA_MAX=4 ./arenas)" = 4

            sed 's|^exec .*|exec gst-inspect-1.0 "$@"|' ${veil}/bin/veil > inspect
            grep -q '^exec gst-inspect-1.0' inspect
            chmod +x inspect
            unset GST_PLUGIN_SYSTEM_PATH_1_0 GST_REGISTRY GST_REGISTRY_1_0
            export XDG_CACHE_HOME="$TMPDIR/cache"
            for element in playbin videoconvert videocrop waylandsink \
                matroskademux qtdemux avdemux_gif avdec_gif avdec_h264 vp8dec vp9dec; do
              ./inspect --exists "$element" || { echo "missing element: $element"; exit 1; }
            done
            test -s "$XDG_CACHE_HOME/veil/gstreamer-registry.bin"
            touch $out
          '';

          # The completions the package installs load in their shells, and
          # its man pages have no roff errors (mandoc's warnings are about
          # the empty date and the paragraph macros of clap_mangen).
          completions = pkgs.runCommand "veil-completions"
            {
              nativeBuildInputs = with pkgs; [ zsh fish nushell mandoc ];
            } ''
            share=${veil}/share
            bash -n $share/bash-completion/completions/veil.bash
            ${pkgs.bashInteractive}/bin/bash -c "source $share/bash-completion/completions/veil.bash && complete -p veil"
            zsh -n $share/zsh/site-functions/_veil
            HOME=$TMPDIR fish -c "source $share/fish/vendor_completions.d/veil.fish; complete -C 'veil daemon --pause-on '" | grep -q covered
            HOME=$TMPDIR nu --no-config-file -c "source $share/nushell/vendor/autoload/veil.nu; scope commands | where name == 'veil config check' | length" | grep -qx 1
            test -e $share/man/man1/veil.1.gz
            test -e $share/man/man1/veil-config-check.1.gz
            for page in $share/man/man1/*.1.gz; do zcat $page > $(basename $page .gz); done
            mandoc -T lint -W error *.1
            touch $out
          '';
        };

        devShells = rec {
          default = pkgs.mkShell {
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
          };

          # The default shell plus a compositor to run veil in: the headless
          # sway of `tests/headless.rs` and `scripts/bench/`, the screenshot
          # and client tools they drive, and what the benchmark needs to make
          # its clips (ffmpeg) and read the daemon's answers (jq).
          headless = pkgs.mkShell {
            inputsFrom = [ default ];
            packages = with pkgs; [
              sway
              grim
              foot
              swaylock
              jq
              ffmpeg-headless
            ];
            # The test windows (foot) need a font, wherever the shell runs.
            FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
          };

          # The default shell with the oldest Rust veil supports in place of
          # nixpkgs' toolchain, to check that `rust-version` is true.
          msrv = pkgs.mkShell {
            nativeBuildInputs = [
              pkgs.pkg-config
              msrvToolchain
            ];

            inherit (veil) buildInputs;
            inherit (default) shellHook;
          };
        };
      }
    );
}
