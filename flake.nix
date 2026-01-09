{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    fenix.url = "github:nix-community/fenix";
    fenix.inputs.nixpkgs.follows = "nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      fenix,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };

        devRustToolchain =
          with fenix.packages.${system};
          combine [
            stable.cargo
            stable.rustc
            stable.clippy
            stable.rustfmt
          ];

        buildRustToolchain =
          with fenix.packages.${system};
          combine [
            stable.cargo
            stable.rustc
          ];
      in
      {
        devShells.default = (pkgs.mkShell.override { stdenv = pkgs.stdenv; }) {
          buildInputs = with pkgs; [
            devRustToolchain
            rust-analyzer
          ];

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [ pkgs.libiconv ];
        };
      }
    );
}
