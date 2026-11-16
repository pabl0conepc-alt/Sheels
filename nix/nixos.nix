# NixOS module. `sheelsFor pkgs` returns the package. It does not import the
# driftwm module itself (the flake's `nixosModules.default` adds it), so it
# also works when driftwm is already set up some other way.
{ sheelsFor }:
{ config, lib, pkgs, options, ... }:
let
  cfg = config.programs.sheels;
  sheels = sheelsFor pkgs;
in
{

  options.programs.sheels = {
    enable = lib.mkEnableOption "Sheels, a Caelestia-style shell for driftwm";

    terminal = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Install fish, starship, fastfetch, btop, eza and zoxide (Caelestia's terminal; `sheels setup` installs their configs).";
    };

    greeter = {
      enable = lib.mkEnableOption "a Caelestia-styled graphical login (greetd + ReGreet) that starts driftwm";
      wallpaper = lib.mkOption {
        type = lib.types.path;
        default = "${sheels}/share/sheels/eww/assets/wallpaper.jpg";
        description = "Background of the login screen.";
      };
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      programs = lib.optionalAttrs (options.programs ? driftwm) { driftwm.enable = lib.mkDefault true; };
      # The driftwm autostart (idle/lock, clipboard history) and the default
      # terminal binding run outside the sheels wrapper: put them on PATH.
      environment.systemPackages = [ sheels ] ++ (with pkgs; [ swayidle wl-clipboard cliphist foot ])
        # Caelestia's terminal setup (configs installed by `sheels setup`).
        ++ lib.optionals cfg.terminal (with pkgs; [ fish starship fastfetch btop eza zoxide ]);

      fonts.packages = with pkgs; [
        sheels # Google Sans Flex (bundled)
        material-symbols
        rubik
        nerd-fonts.caskaydia-cove
        nerd-fonts.jetbrains-mono
      ];

      # Services the shell talks to (all optional, all event driven).
      networking.networkmanager.enable = lib.mkDefault true;
      hardware.bluetooth.enable = lib.mkDefault true;
      services.power-profiles-daemon.enable = lib.mkDefault true;
      services.upower.enable = lib.mkDefault true;
      services.pipewire = {
        enable = lib.mkDefault true;
        pulse.enable = lib.mkDefault true;
      };
      # Lock screen authentication.
      security.pam.services.hyprlock = { };
      xdg.portal = {
        enable = true;
        extraPortals = [ pkgs.xdg-desktop-portal-gtk ];
      };
    }

    (lib.mkIf cfg.greeter.enable {
      # ReGreet runs in cage under greetd; sessions come from the driftwm
      # wayland-session file installed by the driftwm module.
      services.displayManager.regreet = {
        enable = true;
        settings = {
          background = { path = cfg.greeter.wallpaper; fit = "Cover"; };
          GTK.application_prefer_dark_theme = true;
          appearance.greeting_msg = "Welcome back";
        };
        font = { name = "Google Sans Flex"; size = 12; package = sheels; };
        extraCss = builtins.readFile ../config/regreet.css;
      };
    })
  ]);
}
