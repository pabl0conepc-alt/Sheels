{
  description = "Sheels — Caelestia Shell recreated for driftwm with EWW + a Rust daemon";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    driftwm = {
      url = "github:malbiruk/driftwm";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, driftwm, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in
    {
      packages = forAll (pkgs: rec {
        sheels = pkgs.callPackage ./nix/package.nix { };
        default = sheels;
      });

      overlays.default = final: _prev: { sheels = final.callPackage ./nix/package.nix { }; };

      # `sheels`: the shell only (use it if you already import driftwm's module).
      # `default`: driftwm's module + the shell.
      nixosModules.sheels = import ./nix/nixos.nix { sheelsFor = pkgs: self.packages.${pkgs.stdenv.hostPlatform.system}.default; };
      nixosModules.default = { imports = [ driftwm.nixosModules.default self.nixosModules.sheels ]; };
      homeManagerModules.default = import ./nix/home.nix { inherit self; };
      homeModules.default = self.homeManagerModules.default;

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy pkg-config eww ];
        };
      });
    };
}
