//! Colour schemes and wallpaper — a Rust port of the relevant parts of
//! caelestia-cli (`scheme`, `wallpaper`, material generator, smart scheme).
//!
//! Outputs, all regenerated on change:
//! * `<eww dir>/_theme.scss`  → EWW recompiles its CSS live
//! * `theme` topic            → colours available to yuck expressions
//! * driftwm config block     → window borders / title bars / wallpaper
//! * templates                → ~/.local/state/sheels/theme/* (foot, gtk, hyprlock, btop…)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, bail};
use material_colors::color::Rgb;
use material_colors::dynamic_color::{DynamicScheme, Role, Variant};
use material_colors::hct::Hct;
use material_colors::quantize::{Quantizer, QuantizerCelebi};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::app;
use crate::util::{cache_dir, expand, share_dir, state_dir, write_if_changed};

pub const VARIANTS: &[(&str, &str, &str, &str)] = &[
    ("vibrant", "Vibrant", "sentiment_very_dissatisfied", "A high chroma palette. The primary palette's chroma is at maximum."),
    ("tonalspot", "Tonal Spot", "android", "Default for Material theme colours. A pastel palette with a low chroma."),
    ("expressive", "Expressive", "compare_arrows", "A medium chroma palette. The primary palette's hue is different from the seed colour, for variety."),
    ("fidelity", "Fidelity", "compare", "Matches the seed colour, even if the seed colour is very bright (high chroma)."),
    ("content", "Content", "sentiment_calm", "Almost identical to fidelity."),
    ("fruitsalad", "Fruit Salad", "nutrition", "A playful theme - the seed colour's hue does not appear in the theme."),
    ("rainbow", "Rainbow", "looks", "A playful theme - the seed colour's hue does not appear in the theme."),
    ("neutral", "Neutral", "contrast", "Close to greyscale, a hint of chroma."),
    ("monochrome", "Monochrome", "filter_b_and_w", "All colours are greyscale, no chroma."),
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SchemeState {
    pub name: String,
    pub flavour: String,
    pub mode: String,
    pub variant: String,
    pub wallpaper: String,
    pub colours: BTreeMap<String, String>,
}

impl Default for SchemeState {
    fn default() -> Self {
        SchemeState {
            name: "dynamic".into(),
            flavour: "default".into(),
            mode: "dark".into(),
            variant: "tonalspot".into(),
            wallpaper: String::new(),
            colours: BTreeMap::new(),
        }
    }
}

#[derive(Default)]
pub struct Theme {
    st: Mutex<SchemeState>,
}

// ---------------------------------------------------------------- colour maths

fn hex(rgb: Rgb) -> String {
    format!("{:02x}{:02x}{:02x}", rgb.red, rgb.green, rgb.blue)
}

fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_start_matches('#');
    if s.len() < 6 {
        return None;
    }
    let v = u32::from_str_radix(&s[..6], 16).ok()?;
    Some(Rgb::from_u32(v))
}

fn sanitize(d: f64) -> f64 {
    let d = d % 360.0;
    if d < 0.0 { d + 360.0 } else { d }
}

fn diff_degrees(a: f64, b: f64) -> f64 {
    180.0 - ((a - b).abs() - 180.0).abs()
}

fn rotation_direction(from: f64, to: f64) -> f64 {
    if sanitize(to - from) <= 180.0 { 1.0 } else { -1.0 }
}

/// caelestia-cli `harmonize`.
fn harmonize(from: Hct, to: Hct, tone_boost: f64) -> Hct {
    let d = diff_degrees(from.get_hue(), to.get_hue());
    let rot = (d * 0.8).min(100.0);
    let hue = sanitize(from.get_hue() + rot * rotation_direction(from.get_hue(), to.get_hue()));
    Hct::from(hue, from.get_chroma(), (from.get_tone() * (1.0 + tone_boost)).clamp(0.0, 100.0))
}

fn lighten(c: Hct, amount: f64) -> Hct {
    let diff = (100.0 - c.get_tone()) * amount;
    Hct::from(c.get_hue(), c.get_chroma() + diff / 5.0, c.get_tone() + diff)
}

fn darken(c: Hct, amount: f64) -> Hct {
    let diff = c.get_tone() * amount;
    Hct::from(c.get_hue(), (c.get_chroma() - diff / 5.0).max(0.0), c.get_tone() - diff)
}

fn grayscale(c: Hct, light: bool) -> Hct {
    let c = if light { darken(c, 0.35) } else { lighten(c, 0.65) };
    Hct::from(c.get_hue(), 0.0, c.get_tone())
}

const DARK_TERM: [u32; 16] = [
    0x282828, 0xCC241D, 0x98971A, 0xD79921, 0x458588, 0xB16286, 0x689D6A, 0xA89984, 0x928374, 0xFB4934, 0xB8BB26,
    0xFABD2F, 0x83A598, 0xD3869B, 0x8EC07C, 0xEBDBB2,
];
const LIGHT_TERM: [u32; 16] = [
    0xFDF9F3, 0xFF6188, 0xA9DC76, 0xFC9867, 0xFFD866, 0xF47FD4, 0x78DCE8, 0x333034, 0x121212, 0xFF6188, 0xA9DC76,
    0xFC9867, 0xFFD866, 0xF47FD4, 0x78DCE8, 0x333034,
];

fn camel(snake: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for c in snake.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Caelestia's primary colour picker (score.py) on top of Celebi quantisation.
pub fn source_colour(pixels: &[Rgb]) -> Rgb {
    let q = QuantizerCelebi::quantize(pixels, 128);
    let total: f64 = q.color_to_count.values().map(|v| *v as f64).sum::<f64>().max(1.0);
    let mut hue_pop = [0f64; 360];
    let hcts: Vec<(Hct, u32)> = q.color_to_count.iter().map(|(c, n)| (Hct::new(*c), *n)).collect();
    for (h, n) in &hcts {
        hue_pop[(h.get_hue() as usize).min(359)] += *n as f64;
    }
    let mut excited = [0f64; 360];
    for hue in 0..360i32 {
        let p = hue_pop[hue as usize] / total;
        for i in (hue - 14)..(hue + 16) {
            excited[i.rem_euclid(360) as usize] += p;
        }
    }
    let mut scored: Vec<(Hct, f64)> = hcts
        .iter()
        .map(|(h, _)| {
            let prop = excited[(h.get_hue().round() as i32).rem_euclid(360) as usize];
            let ps = prop * 100.0 * 0.7;
            let cw = if h.get_chroma() < 48.0 { 0.1 } else { 0.3 };
            (*h, ps + (h.get_chroma() - 48.0) * cw)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for cutoff in (0..=20).rev() {
        let c = cutoff as f64;
        if let Some((h, _)) = scored.iter().find(|(h, _)| h.get_chroma() > c && h.get_tone() > c * 3.0) {
            return material_colors::dislike::fix_if_disliked(*h).into();
        }
    }
    scored.first().map(|(h, _)| material_colors::dislike::fix_if_disliked(*h).into()).unwrap_or(Rgb::from_u32(0x4285F4))
}

fn variant_of(v: &str) -> Variant {
    match v {
        "vibrant" => Variant::Vibrant,
        "expressive" => Variant::Expressive,
        "fidelity" => Variant::Fidelity,
        "content" => Variant::Content,
        "fruitsalad" => Variant::FruitSalad,
        "rainbow" => Variant::Rainbow,
        "neutral" => Variant::Neutral,
        "monochrome" => Variant::Monochrome,
        _ => Variant::TonalSpot,
    }
}

/// caelestia-cli `gen_scheme`.
pub fn gen_scheme(primary: Rgb, variant: &str, light: bool, flavour: &str) -> BTreeMap<String, String> {
    let scheme = DynamicScheme::by_variant(primary, &variant_of(variant), !light, Some(0.0));
    let mut hct: BTreeMap<String, Hct> = BTreeMap::new();
    for role in Role::ALL {
        let name = camel(role.name());
        hct.insert(name, scheme.get_hct(role.color()));
    }
    // Backwards-compatible names used by Caelestia's templates.
    for c in ["primary", "secondary", "tertiary", "neutral"] {
        if let Some(v) = hct.get(&format!("{c}PaletteKeyColor")).copied() {
            hct.insert(format!("{c}_paletteKeyColor"), v);
        }
    }
    if let Some(v) = hct.get("neutralVariantPaletteKeyColor").copied() {
        hct.insert("neutral_variant_paletteKeyColor".into(), v);
    }
    let key = hct.get("primaryPaletteKeyColor").copied().unwrap_or_else(|| Hct::new(primary));
    let mono = variant == "monochrome";
    let terms = if light { LIGHT_TERM } else { DARK_TERM };
    for (i, c) in terms.iter().enumerate() {
        let h = Hct::new(Rgb::from_u32(*c));
        let boost = if i < 8 { 0.35 } else { 0.2 } * if light { -1.0 } else { 1.0 };
        hct.insert(format!("term{i}"), if mono { grayscale(h, light) } else { harmonize(h, key, boost) });
    }
    if variant == "neutral" {
        for v in hct.values_mut() {
            *v = Hct::from(v.get_hue(), (v.get_chroma() - 15.0).max(0.0), v.get_tone());
        }
    }
    if flavour == "hard" {
        let keys: Vec<String> = hct.keys().filter(|k| *k == "background" || k.starts_with("surface")).cloned().collect();
        for k in keys {
            let v = hct[&k];
            hct.insert(k, if light { lighten(v, 0.4) } else { darken(v, 0.8) });
        }
        let t0 = hct["term0"];
        hct.insert("term0".into(), if light { lighten(t0, 0.4) } else { darken(t0, 0.9) });
    }
    let mut out: BTreeMap<String, String> = hct.into_iter().map(|(k, v)| (k, hex(v.into()))).collect();
    let ext: [(&str, &str, &str); 4] = [
        ("success", "4F6354", "B5CCBA"),
        ("onSuccess", "FFFFFF", "213528"),
        ("successContainer", "D1E8D5", "374B3E"),
        ("onSuccessContainer", "0C1F13", "D1E9D6"),
    ];
    for (k, l, d) in ext {
        out.insert(k.into(), (if light { l } else { d }).to_lowercase());
    }
    out
}

fn load_pixels(path: &Path, max: u32) -> Option<(Vec<Rgb>, image::RgbImage)> {
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let small = img.thumbnail(max, max).to_rgb8();
    let px = small.pixels().map(|p| Rgb::new(p[0], p[1], p[2])).collect();
    Some((px, small))
}

/// caelestia-cli smart options: variant from colourfulness, mode from tone.
fn smart_opts(img: &image::RgbImage) -> (String, String) {
    let n = img.pixels().len().max(1) as f64;
    let (mut rg, mut yb) = (Vec::with_capacity(n as usize), Vec::with_capacity(n as usize));
    let (mut sr, mut sg, mut sb) = (0f64, 0f64, 0f64);
    for p in img.pixels() {
        let (r, g, b) = (p[0] as f64, p[1] as f64, p[2] as f64);
        rg.push((r - g).abs());
        yb.push((0.5 * (r + g) - b).abs());
        sr += r;
        sg += g;
        sb += b;
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let sd = |v: &[f64], m: f64| (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt();
    let (mrg, myb) = (mean(&rg), mean(&yb));
    let colourful = (sd(&rg, mrg).powi(2) + sd(&yb, myb).powi(2)).sqrt() + 0.3 * (mrg.powi(2) + myb.powi(2)).sqrt();
    let variant = if colourful < 10.0 {
        "neutral"
    } else if colourful < 20.0 {
        "content"
    } else {
        "tonalspot"
    };
    let avg = Rgb::new((sr / n) as u8, (sg / n) as u8, (sb / n) as u8);
    let mode = if Hct::new(avg).get_tone() > 60.0 { "light" } else { "dark" };
    (variant.into(), mode.into())
}

// ---------------------------------------------------------------- schemes on disk

fn schemes_dir() -> PathBuf {
    share_dir().join("schemes")
}

/// (name, flavour) for every preset plus "dynamic".
pub fn list_schemes() -> Vec<(String, String)> {
    let mut out = vec![("dynamic".to_string(), "default".to_string()), ("dynamic".to_string(), "hard".to_string())];
    let mut dirs: Vec<PathBuf> = vec![schemes_dir(), crate::util::config_dir().join("schemes")];
    dirs.dedup();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut names: Vec<_> = rd.flatten().filter(|e| e.path().is_dir()).collect();
        names.sort_by_key(|e| e.file_name());
        for n in names {
            let Ok(fl) = std::fs::read_dir(n.path()) else { continue };
            let mut fls: Vec<_> = fl.flatten().filter(|e| e.path().is_dir()).collect();
            fls.sort_by_key(|e| e.file_name());
            for f in fls {
                out.push((n.file_name().to_string_lossy().into(), f.file_name().to_string_lossy().into()));
            }
        }
    }
    out
}

fn scheme_file(name: &str, flavour: &str, mode: &str) -> Option<PathBuf> {
    for base in [crate::util::config_dir().join("schemes"), schemes_dir()] {
        let dir = base.join(name).join(flavour);
        let want = dir.join(format!("{mode}.txt"));
        if want.exists() {
            return Some(want);
        }
        // Scheme only ships the other mode.
        for alt in ["dark.txt", "light.txt"] {
            let p = dir.join(alt);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn read_scheme_file(p: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.to_string(), it.next()?.trim_start_matches('#').to_lowercase()))
        })
        .collect()
}

/// A few swatches for the launcher's scheme list.
pub fn scheme_preview(name: &str, flavour: &str) -> Vec<String> {
    if name == "dynamic" {
        let st = app().theme.st.lock().unwrap();
        return ["primary", "secondary", "tertiary", "surface"].iter().map(|k| format!("#{}", st.colours.get(*k).cloned().unwrap_or_default())).collect();
    }
    let Some(p) = scheme_file(name, flavour, "dark") else { return vec![] };
    let c = read_scheme_file(&p);
    ["primary", "secondary", "tertiary", "surface"].iter().map(|k| format!("#{}", c.get(*k).cloned().unwrap_or_default())).collect()
}

// ---------------------------------------------------------------- wallpapers

pub fn wallpaper_dir() -> PathBuf {
    expand(&crate::app::cfg().paths.wallpaper_dir)
}

pub fn list_wallpapers() -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![wallpaper_dir()];
    let mut depth = 0;
    while let Some(d) = stack.pop() {
        depth += 1;
        if depth > 64 {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .map(|x| matches!(x.to_string_lossy().to_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp"))
                .unwrap_or(false)
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn thumbnail_path(p: &Path) -> PathBuf {
    let key = format!(
        "{}:{}",
        p.to_string_lossy(),
        std::fs::metadata(p).and_then(|m| m.modified()).map(|t| format!("{t:?}")).unwrap_or_default()
    );
    cache_dir().join("thumbs").join(format!("{:016x}.jpg", crate::util::fnv1a(key.as_bytes())))
}

/// Generate missing launcher thumbnails, one at a time on a blocking thread.
pub async fn ensure_thumbnails() {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let walls = list_wallpapers();
    let mut made = false;
    for w in walls {
        let t = thumbnail_path(&w);
        if t.exists() {
            continue;
        }
        let ok = tokio::task::spawn_blocking(move || -> bool {
            let Ok(r) = image::ImageReader::open(&w).and_then(|r| r.with_guessed_format()) else { return false };
            let Ok(img) = r.decode() else { return false };
            let th = img.resize_to_fill(280 * 2, 200 * 2, image::imageops::FilterType::Triangle).to_rgb8();
            let _ = std::fs::create_dir_all(t.parent().unwrap());
            th.save(&t).is_ok()
        })
        .await
        .unwrap_or(false);
        made |= ok;
    }
    RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    if made {
        // Refresh the launcher so new thumbnails show up.
        let _ = app().launcher.command("set", &[app().hub.get("launcher").and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["query"].as_str().map(str::to_string)).unwrap_or_default()]).await;
    }
}

// ---------------------------------------------------------------- theme service

impl Theme {
    pub fn current_scheme_id(&self) -> String {
        let s = self.st.lock().unwrap();
        format!("{} {}", s.name, s.flavour)
    }
    pub fn current_variant(&self) -> String {
        self.st.lock().unwrap().variant.clone()
    }
    pub fn current_wallpaper(&self) -> String {
        self.st.lock().unwrap().wallpaper.clone()
    }

    fn state_file() -> PathBuf {
        state_dir().join("scheme.json")
    }

    pub async fn init(&self) {
        let loaded: SchemeState =
            std::fs::read_to_string(Self::state_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        *self.st.lock().unwrap() = loaded;
        {
            // First run: Caelestia's bundled wallpaper + dynamic scheme.
            let mut st = self.st.lock().unwrap();
            if st.wallpaper.is_empty() || !Path::new(&st.wallpaper).exists() {
                let def = share_dir().join("eww/assets/wallpaper.jpg");
                if def.exists() {
                    st.wallpaper = def.to_string_lossy().into_owned();
                    st.colours.clear();
                }
            }
        }
        if self.st.lock().unwrap().colours.is_empty() {
            self.recompute(false).await;
        }
        self.apply_current().await;
    }

    /// Recompute the colours from scheme name/flavour/mode/variant (+ wallpaper).
    async fn recompute(&self, smart: bool) {
        let st = self.st.lock().unwrap().clone();
        let mut st2 = st.clone();
        if st.name == "dynamic" {
            let wall = PathBuf::from(&st.wallpaper);
            let smart_on = smart && crate::app::cfg().services.smart_scheme;
            let res = tokio::task::spawn_blocking(move || -> Option<(Rgb, String, String)> {
                let (px, img) = load_pixels(&wall, 128)?;
                let src = source_colour(&px);
                let (v, m) = smart_opts(&img);
                Some((src, v, m))
            })
            .await
            .ok()
            .flatten();
            let (src, v, m) = res.unwrap_or((Rgb::from_u32(0xa8627b), st.variant.clone(), st.mode.clone()));
            if smart_on {
                st2.variant = v;
                st2.mode = m;
            }
            st2.colours = gen_scheme(src, &st2.variant, st2.mode == "light", &st2.flavour);
        } else {
            match scheme_file(&st.name, &st.flavour, &st.mode) {
                Some(p) => {
                    st2.colours = read_scheme_file(&p);
                    if p.file_stem().map(|s| s != st.mode.as_str()).unwrap_or(false) {
                        st2.mode = p.file_stem().unwrap().to_string_lossy().into();
                    }
                }
                None => {
                    st2.colours = gen_scheme(Rgb::from_u32(0xa8627b), &st.variant, st.mode == "light", &st.flavour);
                }
            }
        }
        *self.st.lock().unwrap() = st2;
    }

    pub async fn apply_current(&self) {
        let st = self.st.lock().unwrap().clone();
        if let Ok(s) = serde_json::to_string_pretty(&st) {
            let _ = crate::util::write_atomic(&Self::state_file(), s.as_bytes());
        }
        let c = |k: &str| st.colours.get(k).cloned().unwrap_or_else(|| "ff00ff".into());
        // 1. EWW scss
        let cfg = crate::app::cfg();
        let mut scss = String::from("// Generated by sheels — do not edit.\n");
        for (k, v) in &st.colours {
            if k.contains("_paletteKeyColor") {
                continue;
            }
            scss.push_str(&format!("${k}: #{v};\n"));
        }
        scss.push_str(&format!("$light: {};\n", st.mode == "light"));
        scss.push_str(&format!("$border-thickness: {}px;\n", cfg.border.thickness));
        scss.push_str(&format!("$border-rounding: {}px;\n", cfg.border.rounding));
        scss.push_str(&format!("$transparency: {};\n", cfg.appearance.transparency));
        scss.push_str(&format!(
            "$t-base: {};\n$t-layers: {};\n",
            if cfg.appearance.transparency { cfg.appearance.transparency_base - if st.mode == "light" { 0.1 } else { 0.0 } } else { 1.0 },
            if cfg.appearance.transparency { cfg.appearance.transparency_layers } else { 1.0 }
        ));
        scss.push_str(&format!("$font-scale: {};\n$anim-scale: {};\n", cfg.appearance.font_scale, cfg.appearance.anim_scale));
        let eww_dir = app().eww.dir.clone();
        let _ = std::fs::create_dir_all(&eww_dir);
        let _ = write_if_changed(&eww_dir.join("_theme.scss"), &scss);
        // GTK key bindings must bypass the SCSS compiler: plain CSS import.
        let keys = format!("@import url(\"file://{}\");\n", share_dir().join("eww/keys.css").display());
        let _ = write_if_changed(&eww_dir.join("_keys.scss"), &keys);

        // 2. Logo (two-tone) for the bar/dashboard/lock.
        let logo = include_str!("../../eww/assets/logo.svg.in").replace("{{top}}", &c("primary")).replace("{{bottom}}", &c("onSurface"));
        let logo_path = cache_dir().join("logo.svg");
        let _ = write_if_changed(&logo_path, &logo);

        // 3. Topic for yuck expressions.
        let colours: serde_json::Map<String, Value> =
            st.colours.iter().map(|(k, v)| (k.clone(), Value::from(format!("#{v}")))).collect();
        app().hub.publish(
            "theme",
            &json!({
                "name": st.name, "flavour": st.flavour, "mode": st.mode, "variant": st.variant,
                "light": st.mode == "light", "wallpaper": st.wallpaper, "c": colours,
                "logo": logo_path.to_string_lossy(),
            }),
        );

        // 4. Templates and driftwm.
        render_templates(&st);
        if cfg.driftwm.manage_config {
            sync_driftwm(&st, &expand(&cfg.driftwm.config_path)).await;
        }
    }

    pub async fn set_wallpaper(&self, path: &str) -> Result<()> {
        let p = expand(path);
        if !p.is_file() {
            bail!("no such wallpaper: {}", p.display());
        }
        let p = std::fs::canonicalize(&p).unwrap_or(p);
        {
            let mut st = self.st.lock().unwrap();
            st.wallpaper = p.to_string_lossy().into_owned();
        }
        let dynamic = self.st.lock().unwrap().name == "dynamic";
        if dynamic {
            self.recompute(true).await;
        }
        self.apply_current().await;
        Ok(())
    }

    pub async fn wallpaper_cmd(&self, sub: &str, arg: &str) -> Result<String> {
        match sub {
            "set" | "-f" => {
                self.set_wallpaper(arg).await?;
            }
            "random" | "-r" => {
                let walls = list_wallpapers();
                if walls.is_empty() {
                    bail!("no wallpapers in {}", wallpaper_dir().display());
                }
                let cur = self.current_wallpaper();
                let candidates: Vec<&PathBuf> = walls.iter().filter(|w| w.to_string_lossy() != cur).collect();
                let pick = candidates.get((crate::util::now_ms() as usize) % candidates.len().max(1)).copied().unwrap_or(&walls[0]);
                self.set_wallpaper(&pick.to_string_lossy()).await?;
            }
            "get" | "" => return Ok(self.current_wallpaper()),
            "list" => return Ok(list_wallpapers().iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n")),
            _ => bail!("wallpaper set <file>|random|get|list"),
        }
        Ok(String::new())
    }

    pub async fn scheme_cmd(&self, a: &[String]) -> Result<String> {
        let arg = |i: usize| a.get(i).map(String::as_str).unwrap_or("");
        match arg(0) {
            "set" => {
                let name = arg(1);
                if name.is_empty() {
                    bail!("scheme set <name> [flavour] [mode]");
                }
                {
                    let mut st = self.st.lock().unwrap();
                    st.name = name.to_string();
                    st.flavour = if arg(2).is_empty() { "default".into() } else { arg(2).to_string() };
                    if name != "dynamic" && scheme_file(name, &st.flavour, "dark").is_none() {
                        // Pick the first flavour available.
                        if let Some((_, f)) = list_schemes().into_iter().find(|(n, _)| n == name) {
                            st.flavour = f;
                        }
                    }
                    if !arg(3).is_empty() {
                        st.mode = arg(3).to_string();
                    }
                }
                self.recompute(arg(1) == "dynamic" && arg(3).is_empty()).await;
            }
            "mode" => {
                {
                    let mut st = self.st.lock().unwrap();
                    st.mode = match arg(1) {
                        "light" => "light".into(),
                        "dark" => "dark".into(),
                        _ => if st.mode == "dark" { "light".into() } else { "dark".into() },
                    };
                }
                self.recompute(false).await;
            }
            "variant" => {
                let v = arg(1);
                if !VARIANTS.iter().any(|x| x.0 == v) {
                    bail!("unknown variant {v}");
                }
                self.st.lock().unwrap().variant = v.to_string();
                self.recompute(false).await;
            }
            "list" => {
                return Ok(list_schemes().into_iter().map(|(n, f)| format!("{n} {f}")).collect::<Vec<_>>().join("\n"));
            }
            "get" | "" => {
                let s = self.st.lock().unwrap();
                return Ok(format!("{} {} {} {}", s.name, s.flavour, s.mode, s.variant));
            }
            _ => bail!("scheme set|mode|variant|list|get"),
        }
        self.apply_current().await;
        Ok(String::new())
    }
}

// ---------------------------------------------------------------- outputs

fn render(template: &str, st: &SchemeState) -> String {
    render_with(template, st, false)
}

/// `{{ name.hex }}` / `{{ name.rgb }}` (sheels) and `{{ $name }}` (caelestia-cli
/// templates, `hash` decides whether a leading '#' is added).
fn render_with(template: &str, st: &SchemeState, hash: bool) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let expr = rest[start + 2..start + end].trim();
        let val = if expr == "mode" {
            Some(st.mode.clone())
        } else if expr == "share" {
            Some(share_dir().to_string_lossy().into_owned())
        } else if expr == "face" {
            Some(expand(&crate::app::cfg().paths.face).to_string_lossy().into_owned())
        } else if expr == "wallpaper" {
            Some(st.wallpaper.clone())
        } else if let Some(name) = expr.strip_prefix('$') {
            st.colours.get(name).map(|h| if hash { format!("#{h}") } else { h.clone() })
        } else if let Some((name, fmt)) = expr.rsplit_once('.') {
            st.colours.get(name).map(|h| match fmt {
                "hex" => h.clone(),
                "rgb" => {
                    let c = parse_hex(h).unwrap_or(Rgb::new(0, 0, 0));
                    format!("{}, {}, {}", c.red, c.green, c.blue)
                }
                _ => h.clone(),
            })
        } else {
            None
        };
        match val {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + end + 2]),
        }
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    out
}

fn render_templates(st: &SchemeState) {
    let out_dir = state_dir().join("theme");
    let _ = std::fs::create_dir_all(&out_dir);
    for dir in [share_dir().join("templates"), crate::util::config_dir().join("templates")] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if let Ok(t) = std::fs::read_to_string(e.path()) {
                // Same choice as caelestia-cli's gen_replace(hash=…).
                let fname = e.file_name().to_string_lossy().to_string();
                let hash = matches!(fname.as_str(), "pandora.json" | "btop.theme" | "nvtop.colors" | "htop.theme" | "gtk.css" | "thunar.css" | "qtdark.colors" | "qtlight.colors" | "warp.yaml" | "cava.conf");
                let mut out = render_with(&t, st, hash);
                if e.file_name() == "hyprlock.conf" {
                    out = scale_lock(&out);
                }
                let _ = write_if_changed(&out_dir.join(e.file_name()), &out);
            }
        }
    }
    apply_terms(st);
}

fn osc(c: &str, idx: &str) -> String {
    let c = if c.len() >= 6 { c } else { "000000" };
    format!("\x1b]{idx};rgb:{}/{}/{}\x1b\\", &c[0..2], &c[2..4], &c[4..6])
}

/// caelestia-cli `gen_sequences` + `apply_terms`: recolour every open
/// terminal instantly and keep the sequences for new shells (fish config
/// cats ~/.local/state/sheels/sequences.txt).
fn apply_terms(st: &SchemeState) {
    let c = |k: &str| st.colours.get(k).cloned().unwrap_or_default();
    let mut seq = String::new();
    seq += &osc(&c("onSurface"), "10");
    seq += &osc(&c("surface"), "11");
    seq += &osc(&c("secondary"), "12");
    seq += &osc(&c("secondary"), "17");
    for i in 0..16 {
        let v = c(&format!("term{i}"));
        if !v.is_empty() {
            seq += &osc(&v, &format!("4;{i}"));
        }
    }
    seq += &osc(&c("primary"), "4;16");
    seq += &osc(&c("secondary"), "4;17");
    seq += &osc(&c("tertiary"), "4;18");
    let path = state_dir().join("sequences.txt");
    if !write_if_changed(&path, &seq).unwrap_or(false) {
        return;
    }
    let uid = unsafe { libc::getuid() };
    if let Ok(rd) = std::fs::read_dir("/dev/pts") {
        for e in rd.flatten() {
            if !e.file_name().to_string_lossy().chars().all(|ch| ch.is_ascii_digit()) {
                continue;
            }
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            if e.metadata().map(|m| m.uid() != uid).unwrap_or(true) {
                continue;
            }
            if let Ok(mut f) = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY).open(e.path()) {
                use std::io::Write;
                let _ = f.write_all(seq.as_bytes());
            }
        }
    }
}

/// Caelestia sizes its lock to 70% of the screen height (16:9). The template
/// is authored for 1080p; scale every geometry/font number to the monitor.
fn scale_lock(conf: &str) -> String {
    let h = app().ui_active_height() as f64;
    let k = if h > 0.0 { h / 1080.0 } else { 1.0 };
    if (k - 1.0).abs() < 0.01 {
        return conf.to_string();
    }
    conf.lines()
        .map(|l| {
            let t = l.trim_start();
            let key = t.split('=').next().unwrap_or("").trim();
            if matches!(key, "size" | "position" | "font_size" | "rounding" | "line_spacing") {
                let (lhs, rhs) = l.split_once('=').unwrap();
                let nums: Vec<String> = rhs
                    .split(',')
                    .map(|n| match n.trim().parse::<f64>() {
                        Ok(v) if v >= 0.0 || key == "position" => format!("{}", (v * k).round() as i64),
                        _ => n.trim().to_string(),
                    })
                    .collect();
                format!("{lhs}= {}", nums.join(", "))
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const BEGIN: &str = "# >>> sheels managed (do not edit inside) >>>";
const END: &str = "# <<< sheels managed <<<";

/// Keep the driftwm config's managed block in sync (decoration colours and
/// the wallpaper) and hot-reload it.
async fn sync_driftwm(st: &SchemeState, path: &Path) {
    let Ok(cfg) = std::fs::read_to_string(path) else { return };
    let (Some(b), Some(e)) = (cfg.find(BEGIN), cfg.find(END)) else { return };
    if e < b {
        return;
    }
    let c = |k: &str| st.colours.get(k).cloned().unwrap_or_default();
    let mut block = format!("{BEGIN}\n");
    // Values only (no table headers): the block lives inside the user's
    // [decorations] table in the shipped config.
    block.push_str(&format!("bg_color = \"#{}\"\n", c("surfaceContainer")));
    block.push_str(&format!("fg_color = \"#{}\"\n", c("onSurface")));
    block.push_str(&format!("border_color = \"#{}\"\n", c("outlineVariant")));
    block.push_str(&format!("border_color_focused = \"#{}\"\n", c("primary")));
    block.push_str(END);
    let new_cfg = format!("{}{}{}", &cfg[..b], block, &cfg[e + END.len()..]);

    // Wallpaper: a second managed block for [background].
    let new_cfg = sync_background(&new_cfg, st);
    if new_cfg != cfg && crate::util::write_atomic(path, new_cfg.as_bytes()).is_ok() {
        let _ = crate::wm::action("reload-config").await;
    }
}

const BG_BEGIN: &str = "# >>> sheels wallpaper >>>";
const BG_END: &str = "# <<< sheels wallpaper <<<";

fn sync_background(cfg: &str, st: &SchemeState) -> String {
    let (Some(b), Some(e)) = (cfg.find(BG_BEGIN), cfg.find(BG_END)) else { return cfg.to_string() };
    if st.wallpaper.is_empty() || e < b {
        return cfg.to_string();
    }
    let block = format!("{BG_BEGIN}\ntype = \"wallpaper\"\npath = \"{}\"\n{BG_END}", st.wallpaper.replace('"', "\\\""));
    format!("{}{}{}", &cfg[..b], block, &cfg[e + BG_END.len()..])
}

pub async fn toggle_gamemode() {
    // Game mode: no animations/blur/shadows in driftwm + shell, like Caelestia.
    let on = {
        let mut m = app().misc.lock().unwrap();
        m.gamemode = !m.gamemode;
        m.gamemode
    };
    app().publish_status();
    let path = state_dir().join("gamemode");
    if on {
        let _ = std::fs::write(&path, "1");
    } else {
        let _ = std::fs::remove_file(&path);
    }
    app().ui.set_misc("gamemode", Value::Bool(on));
}

pub async fn screenshot(kind: &str) -> Result<String> {
    let dir = crate::util::home().join("Pictures/Screenshots");
    let _ = std::fs::create_dir_all(&dir);
    let name = format!("{}/screenshot-{}.png", dir.display(), crate::util::now_ms());
    // Let the panels slide away first.
    tokio::time::sleep(std::time::Duration::from_millis(450)).await;
    let ok = match kind {
        "region" => {
            let geom = crate::util::run(&["slurp", "-d"], 60000).await.unwrap_or_default();
            if geom.trim().is_empty() {
                return Ok(String::new());
            }
            crate::util::run_ok(&["grim", "-g", geom.trim(), &name], 10000).await
        }
        "window" => crate::wm::request(&json!({"Screenshot": {"target": {"Window": {}}, "scale": 1.0, "path": name}})).await.is_ok(),
        _ => crate::util::run_ok(&["grim", &name], 10000).await,
    };
    if !ok {
        bail!("screenshot failed");
    }
    crate::util::spawn_sh(&format!("wl-copy < {}", crate::util::shell_quote(&name)));
    app().notifs.internal("Screenshot taken", &format!("Saved to {name} and copied to the clipboard"), "screenshot_monitor", false);
    Ok(name)
}
