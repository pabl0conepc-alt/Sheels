{ self }:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.sheels;
  sheels = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
  share = "${sheels}/share/sheels";
  state = "${config.xdg.stateHome}/sheels/theme";
  link = config.lib.file.mkOutOfStoreSymlink;
in
{
  options.programs.sheels = {
    enable = lib.mkEnableOption "Sheels (Caelestia-style shell for driftwm)";

    settings = lib.mkOption {
      type = lib.types.attrs;
      default = { };
      example = { bar.workspaces.shown = 7; services.weatherLocation = "Lisbon"; };
      description = "Written to ~/.config/sheels/shell.json (same keys as Caelestia's shell.json, camelCase).";
    };

    driftwmConfig = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Install Sheels' driftwm config (Caelestia keybinds) to ~/.config/driftwm/config.toml
        if none exists. It is copied (not linked) because sheels keeps its colour and
        wallpaper blocks in sync.
      '';
    };

    terminal = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Caelestia's terminal setup: foot + fish + starship + fastfetch + btop, themed by sheels.";
    };

    gtkTheme = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Follow the shell colour scheme in GTK 3/4 apps (caelestia-cli gtk.css template).";
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      home.packages = [ sheels ] ++ (with pkgs; [ material-symbols rubik nerd-fonts.caskaydia-cove nerd-fonts.jetbrains-mono ]);
      fonts.fontconfig.enable = true;

      xdg.configFile."sheels/shell.json" = lib.mkIf (cfg.settings != { }) {
        text = builtins.toJSON cfg.settings;
      };

      home.activation.sheelsDriftwm = lib.mkIf cfg.driftwmConfig (
        lib.hm.dag.entryAfter [ "writeBoundary" ] ''
          conf="${config.xdg.configHome}/driftwm/config.toml"
          if [ ! -e "$conf" ]; then
            run mkdir -p "$(dirname "$conf")"
            run install -m644 ${share}/config/driftwm/config.toml "$conf"
          fi
        ''
      );
    }

    (lib.mkIf cfg.terminal {
      home.packages = with pkgs; [ foot fish starship fastfetch btop eza zoxide ];
      xdg.configFile = {
        "foot/foot.ini".source = "${share}/config/foot/foot.ini";
        "fish/config.fish".source = "${share}/config/fish/config.fish";
        "fish/functions/fish_greeting.fish".source = "${share}/config/fish/functions/fish_greeting.fish";
        "starship.toml".source = "${share}/config/starship.toml";
        "fastfetch/config.jsonc".source = "${share}/config/fastfetch/config.jsonc";
        "btop/btop.conf".source = "${share}/config/btop/btop.conf";
        "btop/themes/caelestia.theme".source = link "${state}/btop.theme";
      };
    })

    (lib.mkIf cfg.gtkTheme {
      xdg.configFile."gtk-3.0/gtk.css".source = link "${state}/gtk.css";
      xdg.configFile."gtk-4.0/gtk.css".source = link "${state}/gtk.css";
    })
  ]);
}
