{
  lib,
  stdenv,

  apple-sdk_15,
  darwin,
  darwinMinVersionHook,

  cargo-about,
  cargo-bundle,
  crane,
  rustPlatform,
  rustToolchain,

  copyDesktopItems,
  envsubst,
  fetchFromGitHub,
  makeFontsConf,
  makeWrapper,

  alsa-lib,
  cmake,
  curl,
  fontconfig,
  freetype,
  git,
  libgit2,
  libglvnd,
  libxkbcommon,
  nodejs_22,
  openssl,
  perl,
  pkg-config,
  protobuf,
  sqlite,
  vulkan-loader,
  wayland,
  xorg,
  zlib,
  zstd,

  withGLES ? false,
  profile ? "release",
}:
assert withGLES -> stdenv.hostPlatform.isLinux;
let
  mkIncludeFilter =
    root': path: type:
    let
      # note: under lazy-trees this introduces an extra copy
      root = toString root' + "/";
      relPath = lib.removePrefix root path;
      topLevelIncludes = [
        "crates"
        "assets"
        "extensions"
        "script"
        "tooling"
        "Cargo.toml"
        ".config" # nextest?
        ".cargo"
      ];
      firstComp = builtins.head (lib.path.subpath.components relPath);
    in
    builtins.elem firstComp topLevelIncludes;

  craneLib = crane.overrideToolchain rustToolchain;
  gpu-lib = if withGLES then libglvnd else vulkan-loader;
  commonArgs =
    let
      vectorCargoLock = builtins.fromTOML (builtins.readFile ../crates/vector/Cargo.toml);
      stdenv' = stdenv;
    in
    rec {
      pname = "vector-editor";
      version = vectorCargoLock.package.version + "-nightly";
      src = builtins.path {
        path = ../.;
        filter = mkIncludeFilter ../.;
        name = "source";
      };

      cargoLock = ../Cargo.lock;

      nativeBuildInputs =
        [
          cmake
          copyDesktopItems
          curl
          perl
          pkg-config
          protobuf
          cargo-about
          rustPlatform.bindgenHook
        ]
        ++ lib.optionals stdenv'.hostPlatform.isLinux [ makeWrapper ]
        ++ lib.optionals stdenv'.hostPlatform.isDarwin [ cargo-bundle ];

      buildInputs =
        [
          curl
          fontconfig
          freetype
          # TODO: need staticlib of this for linking the musl remote server.
          # should make it a separate derivation/flake output
          # see https://crane.dev/examples/cross-musl.html
          libgit2
          openssl
          sqlite
          zlib
          zstd
        ]
        ++ lib.optionals stdenv'.hostPlatform.isLinux [
          alsa-lib
          libxkbcommon
          wayland
          gpu-lib
          xorg.libX11
          xorg.libxcb
        ]
        ++ lib.optionals stdenv'.hostPlatform.isDarwin [
          apple-sdk_15
          darwin.apple_sdk.frameworks.System
          (darwinMinVersionHook "10.15")
        ];

      cargoExtraArgs = "-p vector -p cli --locked --features=gpui/runtime_shaders";

      stdenv =
        pkgs:
        let
          base = pkgs.llvmPackages.stdenv;
          addBinTools = old: {
            cc = old.cc.override {
              inherit (pkgs.llvmPackages) bintools;
            };
          };
          custom = lib.pipe base [
            (stdenv: stdenv.override addBinTools)
            pkgs.stdenvAdapters.useMoldLinker
          ];
        in
        if stdenv'.hostPlatform.isLinux then custom else base;

      env = {
        ZSTD_SYS_USE_PKG_CONFIG = true;
        FONTCONFIG_FILE = makeFontsConf {
          fontDirectories = [
            ../assets/fonts/plex-mono
            ../assets/fonts/plex-sans
          ];
        };
        VECTOR_UPDATE_EXPLANATION = "Vector has been installed using Nix. Auto-updates have thus been disabled.";
        RELEASE_VERSION = version;

        CARGO_PROFILE = profile;
        # need to handle some profiles specially https://github.com/rust-lang/cargo/issues/11053
        TARGET_DIR = "target/" + (if profile == "dev" then "debug" else profile);

        # for some reason these deps being in buildInputs isn't enough, the only thing
        # about them that's special is that they're manually dlopened at runtime
        NIX_LDFLAGS = lib.optionalString stdenv'.hostPlatform.isLinux "-rpath ${
          lib.makeLibraryPath [
            gpu-lib
            wayland
          ]
        }";

        NIX_OUTPATH_USED_AS_RANDOM_SEED = "norebuilds";
      };

      # prevent nix from removing the "unused" wayland/gpu-lib rpaths
      dontPatchELF = stdenv'.hostPlatform.isLinux;

      # TODO: try craneLib.cargoNextest separate output
      # for now we're not worried about running our test suite (or tests for deps) in the nix sandbox
      doCheck = false;

      cargoVendorDir = craneLib.vendorCargoDeps {
        inherit src cargoLock;
      };
    };
  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
in
craneLib.buildPackage (
  lib.recursiveUpdate commonArgs {
    inherit cargoArtifacts;

    dontUseCmakeConfigure = true;

    # without the env var generate-licenses fails due to crane's fetchCargoVendor, see:
    # https://github.com/vector-editor/vector/issues/19971#issuecomment-2688455390
    # TODO: put this in a separate derivation that depends on src to avoid running it on every build
    preBuild = ''
      ALLOW_MISSING_LICENSES=yes bash script/generate-licenses
      echo nightly > crates/vector/RELEASE_CHANNEL
    '';

    installPhase =
      if stdenv.hostPlatform.isDarwin then
        ''
          runHook preInstall

          pushd crates/vector
          sed -i "s/package.metadata.bundle-nightly/package.metadata.bundle/" Cargo.toml
          export CARGO_BUNDLE_SKIP_BUILD=true
          app_path="$(cargo bundle --profile $CARGO_PROFILE | xargs)"
          popd

          mkdir -p $out/Applications $out/bin
          # Vector expects git next to its own binary
          ln -s ${git}/bin/git "$app_path/Contents/MacOS/git"
          mv $TARGET_DIR/cli "$app_path/Contents/MacOS/cli"
          mv "$app_path" $out/Applications/

          # Physical location of the CLI must be inside the app bundle as this is used
          # to determine which app to start
          ln -s "$out/Applications/Vector Nightly.app/Contents/MacOS/cli" $out/bin/vector

          runHook postInstall
        ''
      else
        ''
          runHook preInstall

          mkdir -p $out/bin $out/libexec
          cp $TARGET_DIR/vector $out/libexec/vector-editor
          cp $TARGET_DIR/cli  $out/bin/vector


          install -D "crates/vector/resources/app-icon-nightly@2x.png" \
            "$out/share/icons/hicolor/1024x1024@2x/apps/vector.png"
          install -D crates/vector/resources/app-icon-nightly.png \
            $out/share/icons/hicolor/512x512/apps/vector.png

          (
            export DO_STARTUP_NOTIFY="true"
            export APP_CLI="vector"
            export APP_ICON="vector"
            export APP_NAME="Vector Nightly"
            export APP_ARGS="%U"
            mkdir -p "$out/share/applications"
            ${lib.getExe envsubst} < "crates/vector/resources/vector.desktop.in" > "$out/share/applications/dev.vector.Vector-Nightly.desktop"
          )

          runHook postInstall
        '';

    # TODO: why isn't this also done on macOS?
    postFixup = lib.optionalString stdenv.hostPlatform.isLinux ''
      wrapProgram $out/libexec/vector-editor --suffix PATH : ${lib.makeBinPath [ nodejs_22 ]}
    '';

    meta = {
      description = "High-performance code editor";
      homepage = "https://vector.dev";
      changelog = "https://vector.dev/releases/preview";
      license = lib.licenses.gpl3Only;
      mainProgram = "vector";
      platforms = lib.platforms.linux ++ lib.platforms.darwin;
    };
  }
)
