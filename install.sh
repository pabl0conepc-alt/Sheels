#!/usr/bin/env bash
# Sheels installer for NixOS — one command, also used to update:
#
#   curl -fsSL https://raw.githubusercontent.com/pabl0conepc-alt/Cealestia-on-rust/main/install.sh | bash
#
# It adds /etc/nixos/sheels.nix (imported from configuration.nix, a backup is
# kept), rebuilds the system with little memory, then runs `sheels setup` for
# your user (driftwm keybinds, terminal, wallpapers folder).
set -euo pipefail

REF="${SHEELS_REF:-main}"
URL="https://github.com/pabl0conepc-alt/Cealestia-on-rust/archive/${REF}.tar.gz"
CONF=/etc/nixos/configuration.nix
MOD=/etc/nixos/sheels.nix

say() { printf '\033[1;36m::\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[ -f /etc/NIXOS ] || die "this installer is for NixOS (see the README for other setups)"
[ -f "$CONF" ] || die "$CONF not found"
[ "$(id -u)" -ne 0 ] || die "run it as your user (it asks for sudo when needed)"

# Already installed through a flake input? Then only update that input.
if grep -qE 'sheels(-flake)?\.nixosModules|nixosModules\.sheels' /etc/nixos/*.nix 2>/dev/null \
   && ! grep -q 'sheels.nix' "$CONF"; then
  say "Sheels comes from your flake: updating it"
  (cd /etc/nixos && sudo nix flake update "$(grep -oE '\bsheels(-flake)?\b(\.url)?' flake.nix | head -1 | sed 's/\.url//')" ) || true
else
  if [ ! -f "$MOD" ]; then
    say "writing $MOD"
    # driftwm too, unless the system already sets it up.
    DRIFT=false
    if ! grep -qE 'driftwm' "$CONF" /etc/nixos/flake.nix 2>/dev/null; then
      say "driftwm not found in your config: adding it as well"
      DRIFT=true
    fi
    sudo tee "$MOD" >/dev/null <<EOF
# Added by the Sheels installer. Remove this file and its line in
# configuration.nix to uninstall.
{ pkgs, lib, ... }:
let
  system = pkgs.stdenv.hostPlatform.system;
  withDriftwm = $DRIFT;
  drift = builtins.fetchTarball "https://github.com/malbiruk/driftwm/archive/main.tar.gz";
  driftPkg = ((import "\${drift}/flake.nix").outputs {
    self = drift;
    nixpkgs = { lib = pkgs.lib; legacyPackages.\${system} = pkgs; };
  }).packages.\${system}.default;
in
{
  imports = [ "\${builtins.fetchTarball "$URL"}/nix/module.nix" ]
    ++ lib.optional withDriftwm "\${drift}/nix/nixos-module.nix";
  programs = { sheels.enable = true; } // lib.optionalAttrs withDriftwm {
    driftwm = { enable = true; package = driftPkg; };
  };
}
EOF
  fi
  if ! grep -q './sheels.nix' "$CONF"; then
    grep -q './hardware-configuration.nix' "$CONF" \
      || die "could not find ./hardware-configuration.nix in $CONF: add ./sheels.nix to your imports by hand"
    sudo cp "$CONF" "$CONF.bak-sheels"
    say "adding ./sheels.nix to the imports of $CONF (backup: $CONF.bak-sheels)"
    sudo sed -i '0,/\.\/hardware-configuration\.nix/s//&\n      .\/sheels.nix/' "$CONF"
  fi
fi

say "rebuilding (first time takes a while: Sheels and its EWW are compiled)"
FLAGS=(--option cores 2 --option tarball-ttl 0)
if [ -f /etc/nixos/flake.nix ]; then
  sudo nixos-rebuild switch --impure "${FLAGS[@]}"
else
  sudo nixos-rebuild switch "${FLAGS[@]}"
fi

say "setting up your user"
/run/current-system/sw/bin/sheels setup

say "done — log out and pick driftwm on the login screen"
