{
  lib,
  rustPlatform,
  makeWrapper,
  pkg-config,
  eww,
  pulseaudio,
  brightnessctl,
  networkmanager,
  curl,
  wl-clipboard,
  cliphist,
  grim,
  slurp,
  wf-recorder,
  hyprlock,
  hyprpicker,
  fuzzel,
  swayidle,
  libqalculate,
  power-profiles-daemon,
  xdg-utils,
  ddcutil,
  procps,
  systemd,
  material-symbols,
  rubik,
  adwaita-icon-theme,
  hicolor-icon-theme,
}:
let
  # EWW + two small input-widget props (`:onkeypress`, `:autofocus`) so the
  # launcher gets Esc/arrow keys and focus like Caelestia's.
  eww' = eww.overrideAttrs (old: {
    patches = (old.patches or [ ]) ++ [ ./eww-sheels.patch ];
    # gtk-rs crates need a lot of memory per rustc: at most two at a time so
    # the build also works on machines with little RAM.
    preBuild = (old.preBuild or "") + ''
      export NIX_BUILD_CORES=$(( NIX_BUILD_CORES > 2 ? 2 : NIX_BUILD_CORES ))
    '';
  });
  runtimeDeps = [
    eww'
    pulseaudio # pactl (works with pipewire-pulse)
    brightnessctl
    networkmanager # nmcli
    curl
    wl-clipboard
    cliphist
    grim
    slurp
    wf-recorder
    hyprlock
    hyprpicker
    fuzzel
    swayidle
    libqalculate # qalc for the launcher calculator
    power-profiles-daemon # powerprofilesctl
    xdg-utils
    ddcutil
    procps # pidof
    systemd # systemd-inhibit, systemctl
  ];
in
rustPlatform.buildRustPackage {
  pname = "sheels";
  version = "1.0.0";

  src = lib.cleanSourceWith {
    src = ../.;
    filter = path: _type: !(lib.hasInfix "/target" path);
  };
  cargoRoot = "daemon";
  buildAndTestSubdir = "daemon";
  cargoLock.lockFile = ../daemon/Cargo.lock;

  nativeBuildInputs = [ makeWrapper pkg-config ];

  postInstall = ''
    share=$out/share/sheels
    mkdir -p $share $out/share/fonts/truetype
    cp -r eww schemes templates config $share/
    cp fonts/*.ttf $out/share/fonts/truetype/
    install -Dm755 bin/sheels-emoji bin/sheels-clipboard -t $out/bin
    for b in $out/bin/sheels $out/bin/sheels-emoji $out/bin/sheels-clipboard; do
      wrapProgram $b \
        --prefix PATH : ${lib.makeBinPath runtimeDeps}:$out/bin \
        --set-default SHEELS_SHARE $share \
        --set-default SHEELS_FONT_DIRS $out/share/fonts:${material-symbols}/share/fonts:${rubik}/share/fonts \
        --suffix XDG_DATA_DIRS : ${adwaita-icon-theme}/share:${hicolor-icon-theme}/share
    done
  '';

  meta = {
    description = "Caelestia Shell clone for the driftwm compositor, rendered with EWW and driven by a Rust daemon";
    homepage = "https://github.com/pabl0conepc-alt/Sheels";
    license = lib.licenses.gpl3Only;
    platforms = lib.platforms.linux;
    mainProgram = "sheels";
  };
}
