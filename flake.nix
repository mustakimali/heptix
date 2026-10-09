{
  description = "heptix - the missing haptic feedback for Linux!";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in {
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "heptix";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.makeWrapper ];
          # Audio sinks are found on PATH at runtime; add pipewire as a default.
          postInstall = ''
            wrapProgram $out/bin/heptix --suffix PATH : ${pkgs.lib.makeBinPath [ pkgs.pipewire pkgs.alsa-utils ]}
          '';
          meta.mainProgram = "heptix";
        };
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.pipewire pkgs.alsa-utils ];
        };
      });
    };
}
