# The veil package. Used by the flake (`packages`, `overlays.default`) and by
# `shell.nix`, so the plugin set and the dependencies are written down once.
{
  lib,
  stdenv,
  rustPlatform,
  pkg-config,
  makeWrapper,
  installShellFiles,
  wayland,
  wayland-protocols,
  libxkbcommon,
  gst_all_1,
}:

let
  cargoToml = lib.importTOML ../Cargo.toml;

  # Everything a pipeline of veil asks for: `playbin` and `videoconvert`
  # (base), `videocrop` and the demuxers of the common containers (good),
  # `waylandsink` and the VA-API decoders (bad), and the software decoders,
  # among them `avdemux_gif`/`avdec_gif` for animated GIFs (libav).
  gstPlugins = with gst_all_1; [
    gstreamer
    gst-plugins-base
    gst-plugins-good
    gst-plugins-bad
    gst-libav
  ];

  gstPluginPath = lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" gstPlugins;
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  inherit (cargoToml.package) version;

  # Only what cargo reads, so editing the docs does not rebuild the package.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../tests
      # Installed, and read by `tests/systemd.rs`.
      ../contrib/systemd/veil.service
      # Installed, and read by a test of `src/config_file.rs`.
      ../contrib/config.example.toml
      # Installed, and checked against the protocol by `tests/ipc_reference.rs`.
      ../docs/ipc.md
      # Installed: the license travels with the binary.
      ../LICENSE
      ../ADDITIONAL-PERMISSIONS.md
      ../THIRD-PARTY-NOTICES
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    makeWrapper
    installShellFiles
  ];

  buildInputs = [
    wayland
    wayland-protocols
    libxkbcommon
  ]
  ++ gstPlugins;

  # Let the GStreamer tests run in the build instead of skipping: they need
  # the plugins, and a registry of their own (there is no writable home; the
  # cache directory is for the fonts some plugins load while being scanned).
  preCheck = ''
    export GST_PLUGIN_SYSTEM_PATH_1_0="${gstPluginPath}"
    export GST_REGISTRY_1_0="$TMPDIR/gstreamer-registry.bin"
    export XDG_CACHE_HOME="$TMPDIR/cache"
  '';

  # The systemd user unit, pointing at this package's binary (the wrapper).
  # NixOS picks it up from `systemd.packages`. The annotated configuration
  # file and the reference of the control socket go with the documentation.
  postInstall = ''
    install -Dm644 contrib/systemd/veil.service $out/lib/systemd/user/veil.service
    substituteInPlace $out/lib/systemd/user/veil.service \
      --replace-fail "ExecStart=/usr/bin/veil " "ExecStart=$out/bin/veil "
    install -Dm644 contrib/config.example.toml $out/share/doc/veil/config.example.toml
    install -Dm644 docs/ipc.md $out/share/doc/veil/ipc.md
    install -Dm644 LICENSE ADDITIONAL-PERMISSIONS.md THIRD-PARTY-NOTICES -t $out/share/doc/veil
  ''
  # The shell completions and the man pages come from the binary itself
  # (`veil completions`, and the hidden `veil man-pages`), so they cannot
  # describe another version. Not when cross-compiling: the binary does not
  # run on the machine that builds it.
  + lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
    installShellCompletion --cmd veil \
      --bash <($out/bin/veil completions bash) \
      --zsh <($out/bin/veil completions zsh) \
      --fish <($out/bin/veil completions fish) \
      --nushell <($out/bin/veil completions nushell)
    $out/bin/veil man-pages "$TMPDIR/man"
    installManPage "$TMPDIR"/man/*.1
  '';

  # GStreamer finds its plugins through GST_PLUGIN_SYSTEM_PATH_1_0 only; the
  # binary alone would find the core elements and nothing else. The registry
  # (GStreamer's cache of what each plugin offers) is veil's own unless one
  # was chosen: the shared one in ~/.cache is rewritten by every GStreamer
  # program with its own plugin set, and a plugin it once failed to load
  # stays left out for as long as its file is unchanged, which in the Nix
  # store is for ever.
  # One allocator arena unless the user chose a number: glibc keeps what the
  # threads of a torn-down video pipeline freed in arenas of their own, and
  # more arenas keep more; with one the memory stays flat over video
  # switches, and playback costs the same (docs/known-issues.md, entry 3).
  postFixup = ''
    wrapProgram $out/bin/veil \
      --prefix GST_PLUGIN_SYSTEM_PATH_1_0 : "${gstPluginPath}" \
      --set-default MALLOC_ARENA_MAX 1 \
      --run 'if [ -z "''${GST_REGISTRY_1_0-}''${GST_REGISTRY-}" ]; then export GST_REGISTRY_1_0="''${XDG_CACHE_HOME:-$HOME/.cache}/veil/gstreamer-registry.bin"; fi'
  '';

  passthru = {
    inherit gstPlugins gstPluginPath;
  };

  meta = {
    description = cargoToml.package.description;
    homepage = cargoToml.package.repository;
    # PolyForm Strict 1.0.0 plus ADDITIONAL-PERMISSIONS.md: source-available, not
    # free software, so nixpkgs calls it unfree.
    license = {
      shortName = "polyform-strict-1.0.0";
      fullName = "PolyForm Strict License 1.0.0";
      url = "https://polyformproject.org/licenses/strict/1.0.0";
      free = false;
      redistributable = false;
    };
    mainProgram = "veil";
    platforms = lib.platforms.linux;
  };
}
