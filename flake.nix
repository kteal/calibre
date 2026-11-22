{
  description = "Native Rust access to existing Calibre libraries";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay/5aa410a1df0e4e9c25f3818320651c2bddf66aff";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "calibre";
            version = "0.1.0";
            src = nixpkgs.lib.cleanSource self;

            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "--all-features" ];
            cargoTestFlags = [ "--all-features" ];

            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.sqlite ];

            postCheck = ''
              RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps --offline
              cargo package --offline --locked
            '';

            installPhase = ''
              runHook preInstall
              mkdir -p "$out/share/cargo-package"
              mkdir -p "$out/share/doc/calibre"
              cp target/package/calibre-0.1.0.crate "$out/share/cargo-package/"
              cp -r target/doc "$out/share/doc/calibre/rustdoc"
              runHook postInstall
            '';
          };
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          package = self.packages.${system}.default;
          formatting =
            pkgs.runCommand "calibre-formatting"
              {
                nativeBuildInputs = [
                  pkgs.cargo
                  pkgs.rustfmt
                ];
                src = nixpkgs.lib.cleanSource self;
              }
              ''
                cp -r "$src" source
                chmod -R u+w source
                cd source
                cargo fmt --check
                touch "$out"
              '';
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rustToolchain = pkgs.rust-bin.stable."1.100.0".minimal.override {
            extensions = [
              "clippy"
              "rustfmt"
            ];
          };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustToolchain
              cargo-deny
              nixfmt
              pkg-config
              rust-analyzer
              sqlite
              taplo
            ];
          };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
