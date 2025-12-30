{
  description = "High-performance code editor";

  inputs = {
    nixpkgs.url = "https://channels.nixos.org/nixpkgs-unstable/nixexprs.tar.xz";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    flake-compat.url = "github:edolstra/flake-compat";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      crane,
      ...
    }:
    let
      systems = [
        "x86_64-linux"
        "x86_64-darwin"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      mkVector =
        pkgs:
        let
          rustBin = rust-overlay.lib.mkRustBin { } pkgs;
        in
        pkgs.callPackage ./nix/build.nix {
          crane = crane.mkLib pkgs;
          rustToolchain = rustBin.fromRustupToolchainFile ./rust-toolchain.toml;
        };
    in
    rec {
      packages = forAllSystems (pkgs: rec {
        default = mkVector pkgs;
        debug = default.override { profile = "dev"; };
      });
      devShells = forAllSystems (pkgs: {
        default = pkgs.callPackage ./nix/shell.nix {
          vector-editor = packages.${pkgs.hostPlatform.system}.default;
        };
      });
      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
      overlays.default = final: _: {
        vector-editor = mkVector final;
      };
    };

  nixConfig = {
    extra-substituters = [
      "https://cache.garnix.io"
    ];
    extra-trusted-public-keys = [
      "cache.garnix.io:CTFPyKSLcx5RMJKfLo5EEPUObbA78b0YQ2DTCJXqr9g="
    ];
  };
}
