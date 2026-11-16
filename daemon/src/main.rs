//! sheels — a Caelestia-style desktop shell for driftwm, rendered by EWW.
//!
//! One binary, three roles:
//!   sheels daemon          the event-driven backend (one process)
//!   sheels listen <topic>  stream a topic as JSON lines (used by EWW deflisten)
//!   sheels <command...>    send a command to the daemon

mod app;
mod audio;
mod battery;
mod bluetooth;
mod brightness;
mod calc;
mod client;
mod config;
mod eww;
mod hub;
mod info;
mod launcher;
mod media;
mod network;
mod notifd;
mod recorder;
mod server;
mod dbus;
mod sysmon;
mod theme;
mod ui;
mod util;
mod wm;

const HELP: &str = "sheels — Caelestia-style shell for driftwm + EWW

USAGE:
  sheels daemon                     start the shell (backend + EWW)
  sheels listen <topic>             stream topic JSON (wm status media notifs sys launcher ui theme info)
  sheels get <topic>                print a topic once
  sheels toggle <panel>             launcher | dashboard | session | sidebar | utilities
  sheels open|close <panel>         (close with no panel closes everything)
  sheels volume up|down|mute|set <0-100>   sheels mic mute|up|down|set <0-100>
  sheels brightness up|down|set <0-100>
  sheels media play-pause|next|previous|stop|seek <s>|player <name>
  sheels notifs clear|dnd|dismiss <id>|action <id> <key>
  sheels wallpaper set <file>|random|list
  sheels scheme set <name> [flavour] [mode] | mode dark|light|toggle | variant <name> | list
  sheels wifi toggle|scan|connect <ssid> [password]|disconnect
  sheels bluetooth toggle|scan|connect <mac>|disconnect <mac>
  sheels lock | session <logout|shutdown|reboot|hibernate|suspend>
  sheels idle-inhibit toggle | gamemode toggle
  sheels reload                     reload config + EWW
  sheels quit
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        None | Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            0
        }
        Some("-V" | "--version") => {
            println!("sheels {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("daemon") => match app::run_daemon() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("sheels: {e:#}");
                1
            }
        },
        Some("listen") => client::listen(args.get(1).map(String::as_str).unwrap_or("")),
        _ => client::send(&args),
    };
    std::process::exit(code);
}
