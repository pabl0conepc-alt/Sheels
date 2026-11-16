# NixOS module for configurations without flakes:
#
#   imports = [ "${builtins.fetchTarball "https://github.com/pabl0conepc-alt/Sheels/archive/main.tar.gz"}/nix/module.nix" ];
#   programs.sheels.enable = true;
#
# driftwm itself must be enabled separately (programs.driftwm.enable from
# driftwm's own module), as in its README.
import ./nixos.nix { sheelsFor = pkgs: pkgs.callPackage ./package.nix { }; }
