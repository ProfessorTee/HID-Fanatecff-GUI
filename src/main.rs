//! Fanatec Tuning – Desktop-App für den hid-fanatecff-Treiber mit automatischen Profilen pro Spiel.
//!
//! Liest und schreibt die Werte unter /sys/class/ftec_tuning/<gerät>/. Der Hintergrunddienst
//! (`--daemon`, Autostart) erkennt laufende Spiele, setzt deren Profil und zeigt ein Tray-Icon zum
//! Profilwechsel; das Fenster ist ein eigener Prozess, der nur läuft, solange es offen ist.
//! Zustand zwischen beiden: Profile/Spiele in ~/.config/fanatec-tuning, erkanntes Spiel und aktives
//! Profil in $XDG_RUNTIME_DIR/fanatec-tuning-status.json.
use egui_software_backend::{App, SoftwareBackend, SoftwareBackendAppConfiguration};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const HELP_DE: &str = "Fanatec Tuning – Desktop-App für den hid-fanatecff-Treiber mit automatischen Profilen pro Spiel.

  fanatec-tuning                 Desktop-App öffnen. Von woanders gestartet installiert bzw.
                                 aktualisiert sich das Programm dabei selbst.
  fanatec-tuning --daemon        Spielerkennung + Tray-Icon im Hintergrund (für den Autostart)
  fanatec-tuning --next          nächstes Profil des laufenden Spiels (für Tastenkürzel)
  fanatec-tuning --apply NAME    Profil NAME setzen
  fanatec-tuning --install       nach ~/.local/bin + Startmenü + Autostart
  fanatec-tuning --launch        installieren/aktualisieren falls nötig, dann starten
  fanatec-tuning --uninstall     alles wieder entfernen (Profile bleiben)
  sudo fanatec-tuning --udev     Schreibrechte einrichten (einmalig)
";
const HELP_EN: &str = "Fanatec Tuning – desktop app for the hid-fanatecff driver with automatic per-game profiles.

  fanatec-tuning                 open the desktop app. Started from elsewhere it installs or
                                 updates itself.
  fanatec-tuning --daemon        game detection + tray icon in the background (for autostart)
  fanatec-tuning --next          next profile of the running game (for a keyboard shortcut)
  fanatec-tuning --apply NAME    apply profile NAME
  fanatec-tuning --install       install to ~/.local/bin + start menu + autostart
  fanatec-tuning --launch        install/update if needed, then start
  fanatec-tuning --uninstall     remove everything again (profiles are kept)
  sudo fanatec-tuning --udev     set up write permissions (once)
";

const ICON_PNG: &[u8] = include_bytes!("../icon.png");
const UDEV_PATH: &str = "/etc/udev/rules.d/99-fanatec-tuning.rules";
const UDEV_RULE: &str = "# Erlaubt normalen Benutzern, das Fanatec-Tuning-Menü (hid-fanatecff) zu ändern.\n\
    ACTION==\"add\", SUBSYSTEM==\"ftec_tuning\", RUN+=\"/bin/sh -c 'chmod 666 /sys%p/[A-Z]* /sys%p/advanced_mode'\"\n";
const MODLOAD_PATH: &str = "/etc/modules-load.d/fanatec.conf";
const POLL: Duration = Duration::from_secs(2);
const LAUNCHER_KEY: &str = "X-Fanatec-Launcher="; // markiert die Ein-Klick-.desktop-Datei

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn runtime_dir() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()))
}

static CFG_DIR: LazyLock<PathBuf> = LazyLock::new(|| home().join(".config/fanatec-tuning"));
static BIN_PATH: LazyLock<PathBuf> = LazyLock::new(|| home().join(".local/bin/fanatec-tuning"));
static DESKTOP_PATH: LazyLock<PathBuf> = LazyLock::new(|| home().join(".local/share/applications/fanatec-tuning.desktop"));
static AUTOSTART_PATH: LazyLock<PathBuf> = LazyLock::new(|| home().join(".config/autostart/fanatec-tuning.desktop"));
static ICON_PATH: LazyLock<PathBuf> = LazyLock::new(|| home().join(".local/share/icons/hicolor/256x256/apps/fanatec-tuning.png"));
static OLD_ICON_PATH: LazyLock<PathBuf> = LazyLock::new(|| home().join(".local/share/icons/hicolor/scalable/apps/fanatec-tuning.svg"));
static STATUS_PATH: LazyLock<PathBuf> = LazyLock::new(|| runtime_dir().join("fanatec-tuning-status.json"));

// German UI on a German system, English otherwise.
static DE: LazyLock<bool> = LazyLock::new(|| {
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .is_some_and(|v| v.starts_with("de"))
});

fn t(de: &'static str, en: &'static str) -> &'static str {
    if *DE { de } else { en }
}

fn tf(de: String, en: String) -> String {
    if *DE { de } else { en }
}

// ------------------------------------------------------------------------------ Werte -----

#[derive(Clone, Copy, PartialEq)]
enum Group {
    Basic,
    Effects,
    Natural,
    Expert,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Group::Basic => t("Grundlagen", "Basics"),
            Group::Effects => t("Effekte aus dem Spiel", "Effects from the game"),
            Group::Natural => t("Natürliches Gefühl", "Natural feel"),
            Group::Expert => t("Experte (nur ändern, wenn du weißt, was es tut)", "Expert (only change if you know what it does)"),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Slider,
    Toggle,
    Number,
}

struct Param {
    key: &'static str,
    label: &'static str,
    desc: &'static str,
    lo: i64,
    hi: i64,
    step: i64,
    group: Group,
    kind: Kind,
}

static PARAMS: LazyLock<Vec<Param>> = LazyLock::new(|| {
    use Group::*;
    use Kind::*;
    let p = |key, label, desc, lo, hi, step, group, kind| Param { key, label, desc, lo, hi, step, group, kind };
    vec![
        p("SEN", t("Lenkwinkel", "Steering angle"), t("Maximaler Lenkeinschlag in Grad (90–2520)", "Maximum steering lock in degrees (90–2520)"), 90, 2520, 10, Basic, Slider),
        p("FF", "Force Feedback", t("Gibt die Kraft der Base ans Spiel frei (empfohlen: 100)", "Share of the base's force the game may use (recommended: 100)"), 0, 100, 1, Basic, Slider),
        p("FFS", "FF-Scaling", t("PEAK: kurze Kraftspitzen voll ausgeben (empfohlen) · LINEAR: gleichmäßig", "PEAK: pass short force peaks at full strength (recommended) · LINEAR: even"), 0, 1, 1, Basic, Toggle),
        p("FUL", "Full Force (Boost Kit)", t("An = volle 8 Nm mit Boost Kit · aus = Standard 5 Nm", "On = full 8 Nm with Boost Kit · off = standard 5 Nm"), 0, 100, 100, Basic, Toggle),
        p("FEI", t("Effekt-Intensität", "Effect intensity"), t("Wie hart Effekte wie Curbs durchkommen (empfohlen: 100)", "How hard effects like kerbs come through (recommended: 100)"), 0, 100, 10, Effects, Slider),
        p("FOR", t("Kraft-Effekte", "Force effects"), t("Absolute Stärke der physikalischen Kräfte (empfohlen: 100)", "Absolute strength of the physical forces (recommended: 100)"), 0, 120, 1, Effects, Slider),
        p("SPR", t("Feder-Effekte", "Spring effects"), t("Rückstellkraft – steuern Spiele meist selbst (empfohlen: 100)", "Centering force – usually controlled by the game (recommended: 100)"), 0, 120, 1, Effects, Slider),
        p("DPR", t("Dämpfer-Effekte", "Damper effects"), t("Dämpfung aus der Simulation (100, ggf. im Spiel reduzieren)", "Damping from the simulation (100, reduce in game if needed)"), 0, 120, 1, Effects, Slider),
        p("SHO", "Vibration", t("Vibrationsmotoren (z. B. im Lenkrad) an/aus", "Vibration motors (e.g. in the wheel) on/off"), 0, 100, 100, Effects, Toggle),
        p("NDP", t("Natürliche Dämpfung", "Natural damping"), t("Verhindert Aufschaukeln/Oszillieren (Standard: 50, sportlich: 10–20)", "Prevents oscillation (default: 50, sporty: 10–20)"), 0, 100, 1, Natural, Slider),
        p("NFR", t("Natürliche Reibung", "Natural friction"), t("Mechanischer Widerstand – bei DD meist aus (0)", "Mechanical resistance – usually off on DD (0)"), 0, 100, 1, Natural, Slider),
        p("NIN", t("Natürliche Trägheit", "Natural inertia"), t("Simuliert Lenkradträgheit – meist aus (0)", "Simulates wheel inertia – usually off (0)"), 0, 100, 1, Natural, Slider),
        p("INT", t("Glättung", "Smoothing"), t("Filtert das Rohsignal, macht es weicher (Fanatec-Standard: 11, direkter: 1)", "Filters the raw signal, makes it softer (Fanatec default: 11, more direct: 1)"), 0, 20, 1, Natural, Slider),
        p("SLOT", "Tuning-Slot", t("Speicherplatz der Base (1-5)", "Memory slot of the base (1-5)"), 1, 5, 1, Expert, Number),
        p("ACP", "ACP", t("Vermutlich Modus der Analog-Paddles (AP)", "Probably the analog paddle mode (AP)"), 0, 10, 1, Expert, Number),
        p("advanced_mode", "Advanced Mode", t("Erweitertes Tuning-Menü", "Extended tuning menu"), 0, 1, 1, Expert, Number),
    ]
});

fn toggle_labels(key: &str) -> (&'static str, &'static str) {
    match key {
        "FFS" => ("LINEAR", "PEAK"),
        "FUL" => (t("aus – 5 Nm", "off – 5 Nm"), t("an – 8 Nm", "on – 8 Nm")),
        _ => (t("aus", "off"), t("an", "on")),
    }
}

fn is_profile_key(key: &str) -> bool {
    PARAMS.iter().any(|p| p.key == key && p.group != Group::Expert)
}

fn default_profiles() -> Vec<(&'static str, Value)> {
    vec![
        (t("Empfohlen", "Recommended"), json!({"SEN": 900, "FF": 100, "FFS": 1, "FUL": 100, "FEI": 100, "FOR": 100, "SPR": 100, "DPR": 100, "NDP": 15, "NFR": 0, "NIN": 0, "INT": 1})),
        ("Rally", json!({"SEN": 540, "FF": 80, "FEI": 100, "FOR": 100, "SPR": 100, "DPR": 50, "NDP": 10, "NFR": 0, "NIN": 5, "INT": 2, "SHO": 100})),
        ("GT", json!({"SEN": 900, "FF": 90, "FEI": 100, "FOR": 100, "SPR": 100, "DPR": 50, "NDP": 15, "NFR": 0, "NIN": 0, "INT": 3, "SHO": 100})),
        ("Drift", json!({"SEN": 1080, "FF": 70, "FEI": 90, "FOR": 100, "SPR": 100, "DPR": 40, "NDP": 5, "NFR": 0, "NIN": 0, "INT": 2, "SHO": 100})),
    ]
}

// ------------------------------------------------------------------------------ Wheel -----

fn perm_hint() -> &'static str {
    t("Keine Schreibrechte auf die Tuning-Dateien. Einmalig ausführen: \
       sudo ~/.local/bin/fanatec-tuning --udev  – danach die Base aus- und wieder einschalten.",
      "No write permission on the tuning files. Run once: \
       sudo ~/.local/bin/fanatec-tuning --udev  – then switch the base off and on again.")
}

fn tuning_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("FTEC_DIR") {
        return Some(d.into());
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir("/sys/class/ftec_tuning").ok()?.flatten().map(|e| e.path()).collect();
    found.sort();
    found.into_iter().next()
}

fn read_all() -> Option<HashMap<String, i64>> {
    let d = tuning_dir()?;
    Some(PARAMS.iter().filter_map(|p| {
        Some((p.key.to_string(), std::fs::read_to_string(d.join(p.key)).ok()?.trim().parse().ok()?))
    }).collect())
}

fn write_value(key: &str, value: i64) -> Result<(), String> {
    if !PARAMS.iter().any(|p| p.key == key) {
        return Err(tf(format!("Unbekannter Wert: {key}"), format!("Unknown value: {key}")));
    }
    let d = tuning_dir().ok_or(t("Kein Fanatec-Wheel gefunden. Ist die Base an?", "No Fanatec wheel found. Is the base on?"))?;
    std::fs::write(d.join(key), format!("{value}\n")).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => perm_hint().to_string(),
        _ => format!("{key} = {value} ") + &tf(format!("wurde abgelehnt ({e}). Wert außerhalb des erlaubten Bereichs?"),
                                               format!("was rejected ({e}). Value out of range?")),
    })
}

fn is_writable() -> bool {
    tuning_dir().is_some_and(|d| {
        let path = std::ffi::CString::new(d.join("FF").into_os_string().into_encoded_bytes()).unwrap();
        unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
    })
}

fn icon_name() -> String {
    // Absoluter Pfad: funktioniert sofort, ohne auf den Icon-Cache von KDE zu warten
    if ICON_PATH.exists() { ICON_PATH.to_string_lossy().into() } else { "input-gaming".into() }
}

fn notify(title: &str, body: &str) {
    let _ = Command::new("notify-send").args(["-a", "Fanatec Tuning", "-i", &icon_name(), "-t", "3000", title, body])
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

// ---------------------------------------------------------------------------- Dateien -----

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_json(path: &Path, data: &impl Serialize) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(data)?)?;
    std::fs::rename(tmp, path)
}

/// Alle Profile. Neue Standardprofile werden einmalig ergänzt, gelöschte Standardprofile bleiben gelöscht.
fn load_profiles() -> Map<String, Value> {
    let path = CFG_DIR.join("profiles.json");
    let seeded_path = CFG_DIR.join("seeded.json");
    let mut profiles = read_json(&path).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let mut seeded: Vec<String> = read_json(&seeded_path).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let same = ["Empfohlen", "Recommended"]; // one default profile, named per UI language
    let missing: Vec<_> = default_profiles().into_iter().filter(|(n, _)| {
        !seeded.iter().any(|s| s == n) && !(same.contains(n) && seeded.iter().any(|s| same.contains(&s.as_str())))
    }).collect();
    if !missing.is_empty() {
        for (n, v) in missing {
            profiles.entry(n).or_insert(v);
            seeded.push(n.into());
        }
        seeded.sort();
        let _ = write_json(&path, &profiles);
        let _ = write_json(&seeded_path, &seeded);
    }
    profiles
}

fn save_profiles(profiles: &Map<String, Value>) -> Result<(), String> {
    write_json(&CFG_DIR.join("profiles.json"), profiles).map_err(|e| e.to_string())
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Game {
    name: String,
    exe: String,
    #[serde(default)]
    profiles: Vec<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Games {
    games: Vec<Game>,
    #[serde(default)]
    after_exit: String,
}

fn load_games() -> Games {
    let path = CFG_DIR.join("games.json");
    read_json(&path).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_else(|| {
        let g = |name: &str, exe: &str, p: &str| Game { name: name.into(), exe: exe.into(), profiles: vec![p.into()] };
        let games = Games { games: vec![g("Assetto Corsa", "acs.exe", "GT"), g("Assetto Corsa EVO", "AssettoCorsaEVO.exe", "GT"),
                                        g("Assetto Corsa Rally", "acr.exe", "Rally")], after_exit: String::new() };
        let _ = write_json(&path, &games);
        games
    })
}

fn save_games(data: &Games) -> Result<(), String> {
    let mut games = vec![];
    for g in &data.games {
        let (name, exe): (String, String) = (g.name.trim().chars().take(60).collect(), g.exe.trim().chars().take(120).collect());
        if name.is_empty() || exe.is_empty() {
            return Err(t("Jedes Spiel braucht einen Namen und eine .exe", "Every game needs a name and an .exe").into());
        }
        games.push(Game { name, exe, profiles: g.profiles.iter().filter(|p| !p.trim().is_empty()).cloned().collect() });
    }
    write_json(&CFG_DIR.join("games.json"), &Games { games, after_exit: data.after_exit.clone() }).map_err(|e| e.to_string())
}

// --------------------------------------------------------------------- Profile setzen -----

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
struct Status {
    game: Option<String>,    // Name des erkannten Spiels
    profile: Option<String>, // zuletzt gesetztes Profil
    #[serde(default)]
    message: String, // letzte Meldung (z. B. Fehler)
}

fn status() -> Status {
    read_json(&STATUS_PATH).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn update_status(f: impl FnOnce(&mut Status)) {
    let mut s = status();
    f(&mut s);
    let _ = write_json(&STATUS_PATH, &s);
}

/// Setzt ein Profil; Fehler einzelner Werte kommen als Warnungen zurück, fehlende Rechte als Fehler.
fn apply_profile(name: &str, reason: &str) -> Result<Vec<String>, String> {
    let profiles = load_profiles();
    let profile = profiles.get(name).and_then(Value::as_object)
        .ok_or_else(|| tf(format!("Profil '{name}' gibt es nicht"), format!("Profile '{name}' does not exist")))?;
    let mut errors = vec![];
    for (key, value) in profile {
        if is_profile_key(key) {
            if let Err(e) = write_value(key, value.as_i64().unwrap_or(0)) {
                if e == perm_hint() {
                    update_status(|s| s.message = e.clone());
                    return Err(e);
                }
                errors.push(e);
            }
        }
    }
    update_status(|s| (s.profile, s.message) = (Some(name.into()), errors.join(" | ")));
    if !reason.is_empty() {
        let sen = profile.get("SEN").and_then(Value::as_i64).map_or(String::new(), |s| format!(" · {s}°"));
        notify(&tf(format!("Profil „{name}“"), format!("Profile \"{name}\"")), &format!("{reason}{sen}"));
    }
    Ok(errors)
}

/// Nächstes Profil des laufenden Spiels (ohne Spiel: alle Profile durchgehen).
fn next_profile() -> Result<String, String> {
    let st = status();
    let games = load_games();
    let profiles = load_profiles();
    let game = games.games.iter().find(|g| Some(&g.name) == st.game.as_ref());
    let mut names: Vec<String> = game.map(|g| g.profiles.iter().filter(|p| profiles.contains_key(*p)).cloned().collect()).unwrap_or_default();
    if names.is_empty() {
        names = profiles.keys().cloned().collect();
    }
    if names.is_empty() {
        return Err(t("Keine Profile vorhanden", "No profiles available").into());
    }
    let idx = st.profile.and_then(|p| names.iter().position(|n| *n == p)).map_or(0, |i| (i + 1) % names.len());
    let reason = format!("{}/{}", idx + 1, names.len()) + &st.game.map_or(String::new(), |g| format!(" – {g}"));
    apply_profile(&names[idx], &reason)?;
    Ok(names[idx].clone())
}

// --------------------------------------------------------------------- Spielerkennung -----

/// Dateinamen (klein geschrieben) aller laufenden Programme des Benutzers.
/// Bei Wine/Proton steht im argv[0] der Windows-Pfad, z. B. C:\...\acs.exe.
fn running_exes() -> Vec<String> {
    use std::os::unix::fs::MetadataExt;
    let uid = unsafe { libc::getuid() };
    let mut names = vec![];
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) || e.metadata().map(|m| m.uid()).ok() != Some(uid) {
            continue;
        }
        let Ok(cmdline) = std::fs::read(e.path().join("cmdline")) else { continue };
        let argv0 = String::from_utf8_lossy(cmdline.split(|b| *b == 0).next().unwrap_or_default()).replace('\\', "/");
        let name = argv0.rsplit('/').next().unwrap_or_default().to_lowercase();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

const WINE_SYSTEM_EXES: [&str; 20] = [
    "explorer.exe", "services.exe", "winedevice.exe", "plugplay.exe", "svchost.exe", "rpcss.exe", "tabtip.exe", "conhost.exe",
    "wineboot.exe", "start.exe", "steam.exe", "steamwebhelper.exe", "steamerrorreporter.exe", "crashpad_handler.exe",
    "rundll32.exe", "winemenubuilder.exe", "cmd.exe", "mscorsvw.exe", "unitycrashhandler64.exe", "crashreportclient.exe",
];

/// Laufende .exe-Programme ohne Wine-/Steam-Hilfsprozesse (für die Auswahl im Fenster).
fn running_game_exes() -> Vec<String> {
    let mut v: Vec<String> = running_exes().into_iter().filter(|n| n.ends_with(".exe") && !WINE_SYSTEM_EXES.contains(&n.as_str())).collect();
    v.sort();
    v
}

/// Ein Durchlauf der Spielerkennung; `last` = zuletzt erkanntes Spiel.
fn monitor_step(last: &mut Option<String>) {
    let games = load_games();
    let running = running_exes();
    let found = games.games.iter().find(|g| running.contains(&g.exe.to_lowercase()));
    let name = found.map(|g| g.name.clone());
    if name == *last {
        return;
    }
    *last = name.clone();
    update_status(|s| s.game = name.clone());
    let result = if let Some(g) = found {
        let profiles = load_profiles();
        match g.profiles.iter().find(|p| profiles.contains_key(*p)) {
            Some(start) if tuning_dir().is_some() => apply_profile(start, &tf(format!("{} erkannt", g.name), format!("{} detected", g.name))),
            _ => Ok(vec![]),
        }
    } else if !games.after_exit.is_empty() && tuning_dir().is_some() {
        apply_profile(&games.after_exit, t("Spiel beendet", "game exited"))
    } else {
        Ok(vec![])
    };
    if let Err(e) = result {
        update_status(|s| s.message = e);
    }
}

// ------------------------------------------------------------------ Dienst + Tray -----

/// Einzelinstanz-Sperre im Runtime-Verzeichnis; gilt, solange die Datei offen ist.
fn lock(name: &str) -> Option<File> {
    let f = File::create(runtime_dir().join(name)).ok()?;
    (unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(f)
}

fn daemon_running() -> bool {
    lock("fanatec-tuning-daemon.lock").is_none()
}

fn spawn_self(args: &[&str]) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe).args(args).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn();
    }
}

struct Tray {
    profiles: Vec<String>,
    active: Option<String>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "fanatec-tuning".into()
    }

    fn title(&self) -> String {
        "Fanatec Tuning".into()
    }

    fn icon_theme_path(&self) -> String {
        home().join(".local/share/icons").to_string_lossy().into()
    }

    fn icon_name(&self) -> String {
        if ICON_PATH.exists() { "fanatec-tuning" } else { "input-gaming" }.into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let title = "Fanatec Tuning".to_string() + &self.active.as_ref().map_or(String::new(), |a| format!(" – {a}"));
        ksni::ToolTip { title, ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        spawn_self(&[]);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let names = self.profiles.clone();
        vec![
            StandardItem { label: t("Profil", "Profile").into(), enabled: false, ..Default::default() }.into(),
            RadioGroup {
                selected: self.active.as_ref().and_then(|a| self.profiles.iter().position(|p| p == a)).unwrap_or(usize::MAX),
                select: Box::new(move |this: &mut Self, i| {
                    if apply_profile(&names[i], "").is_ok() {
                        this.active = Some(names[i].clone());
                    }
                }),
                options: self.profiles.iter().map(|p| RadioItem { label: p.clone(), ..Default::default() }).collect(),
            }.into(),
            MenuItem::Separator,
            StandardItem { label: t("Fenster anzeigen", "Show window").into(), icon_name: "window".into(),
                           activate: Box::new(|_| spawn_self(&[])), ..Default::default() }.into(),
            StandardItem { label: t("Beenden (Spielerkennung aus)", "Quit (stops game detection)").into(), icon_name: "application-exit".into(),
                           activate: Box::new(|_| std::process::exit(0)), ..Default::default() }.into(),
        ]
    }
}

fn run_daemon() {
    use ksni::blocking::TrayMethods;
    let Some(_lock) = lock("fanatec-tuning-daemon.lock") else { return }; // läuft schon
    if tuning_dir().is_none() {
        println!("{}", t("Hinweis: Kein Fanatec-Wheel gefunden – ist die Base eingeschaltet?", "Note: no Fanatec wheel found – is the base switched on?"));
    } else if !is_writable() {
        println!("{}{}", t("Hinweis: ", "Note: "), perm_hint());
    }
    update_status(|s| s.game = None);
    let names = |p: Map<String, Value>| p.keys().cloned().collect::<Vec<_>>();
    let mut shown = (names(load_profiles()), status().profile);
    let tray = Tray { profiles: shown.0.clone(), active: shown.1.clone() }.spawn().ok(); // ohne Tray läuft die Erkennung trotzdem
    let mut last = None;
    loop {
        monitor_step(&mut last); // der Wächter darf nie abstürzen: alle Fehler landen in der Statusmeldung
        let now = (names(load_profiles()), status().profile);
        if now != shown {
            shown = now.clone();
            if let Some(h) = &tray {
                h.update(move |t: &mut Tray| (t.profiles, t.active) = now);
            }
        }
        std::thread::sleep(POLL);
    }
}

fn cli_profile(next: bool, name: Option<&str>) {
    let result = if next { next_profile() } else { apply_profile(name.unwrap(), t("per Befehl", "via command")).map(|_| name.unwrap().into()) };
    match result {
        Ok(p) => println!("{} {p}", t("Profil gesetzt:", "Profile applied:")),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

// -------------------------------------------------------------------- Installation -----

fn desktop_dir() -> PathBuf {
    std::fs::read_to_string(home().join(".config/user-dirs.dirs")).ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("XDG_DESKTOP_DIR=").map(|v| v.trim().trim_matches('"').replace("$HOME", &home().to_string_lossy()))))
        .map(PathBuf::from).unwrap_or_else(|| home().join("Desktop"))
}

/// Ein-Klick-Starter auf dem Schreibtisch und in Downloads bekommen das eigene Icon.
fn fix_launcher_icons() {
    for folder in [desktop_dir(), home().join("Downloads")] {
        let mut files = vec![];
        for e in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                files.extend(std::fs::read_dir(e.path()).into_iter().flatten().flatten().map(|e| e.path()));
            } else {
                files.push(e.path());
            }
        }
        for path in files.into_iter().filter(|p| p.extension().is_some_and(|x| x == "desktop")) {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if !text.contains(&format!("\n{LAUNCHER_KEY}")) {
                continue;
            }
            let want = format!("Icon={}", icon_name());
            let fixed: Vec<String> = text.split('\n').map(|l| if l.starts_with("Icon=") { want.clone() } else { l.into() }).collect();
            if fixed.join("\n") != text {
                let _ = std::fs::write(&path, fixed.join("\n"));
            }
        }
    }
}

fn write_desktop(path: &Path, exec: &str, name: &str, comment: &str, extra: &str) {
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, format!("[Desktop Entry]\nType=Application\nName={name}\nComment={comment}\nExec={exec}\nIcon={}\nTerminal=false\n{extra}", icon_name()));
}

/// Beendet andere laufende Instanzen (Dienst/Fenster, auch die alte Python-Version), damit die neue greift.
fn stop_other_instances() {
    let (me, parent) = (std::process::id(), unsafe { libc::getppid() } as u32);
    let bin = BIN_PATH.to_string_lossy().to_string();
    let mut stopped = vec![];
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        if pid == me || pid == parent {
            continue;
        }
        let exe = std::fs::read_link(e.path().join("exe")).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let argv: Vec<String> = std::fs::read(e.path().join("cmdline")).unwrap_or_default().split(|b| *b == 0)
            .filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into()).collect();
        let base = |s: &str| s.rsplit('/').next().unwrap_or("").to_string();
        let python = argv.len() >= 2 && base(&argv[0]).starts_with("python") && base(&argv[1]) == "fanatec-tuning";
        if exe == bin || exe == format!("{bin} (deleted)") || python {
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            stopped.push(pid);
        }
    }
    for _ in 0..30 {
        if !stopped.iter().any(|p| Path::new(&format!("/proc/{p}")).exists()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn which(cmd: &str) -> bool {
    std::env::var("PATH").unwrap_or_default().split(':').any(|d| Path::new(d).join(cmd).is_file())
}

fn install() {
    let me = std::env::current_exe().and_then(|p| p.canonicalize()).unwrap_or_default();
    let _ = std::fs::create_dir_all(BIN_PATH.parent().unwrap());
    stop_other_instances();
    if me != BIN_PATH.canonicalize().unwrap_or_default() {
        let new = BIN_PATH.with_extension("new");
        if let Err(e) = std::fs::copy(&me, &new).and_then(|_| std::fs::rename(&new, &*BIN_PATH)) {
            eprintln!("{}: {e}", BIN_PATH.display());
            std::process::exit(1);
        }
    }
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&*BIN_PATH, std::fs::Permissions::from_mode(0o755));
    let _ = std::fs::create_dir_all(ICON_PATH.parent().unwrap());
    let _ = std::fs::write(&*ICON_PATH, ICON_PNG);
    let _ = std::fs::remove_file(&*OLD_ICON_PATH);
    let bin = BIN_PATH.to_string_lossy();
    write_desktop(&DESKTOP_PATH, &bin, "Fanatec Tuning",
                  t("Tuning-Menü der Fanatec-Wheelbase einstellen", "Adjust the tuning menu of the Fanatec wheel base"), "Categories=Game;Settings;\n");
    write_desktop(&AUTOSTART_PATH, &format!("{bin} --daemon"), t("Fanatec Tuning (Hintergrund)", "Fanatec Tuning (background)"),
                  t("Erkennt Spiele und lädt passende Wheel-Profile", "Detects games and loads matching wheel profiles"), "X-KDE-autostart-phase=2\n");
    let _ = std::fs::create_dir_all(&*CFG_DIR);
    let _ = std::fs::write(CFG_DIR.join("installed"), env!("CARGO_PKG_VERSION"));
    println!("{} {bin}", t("Installiert:", "Installed:"));
    println!("{}", t("Startmenü-Eintrag „Fanatec Tuning“ angelegt.", "Start menu entry \"Fanatec Tuning\" created."));
    println!("{}", t("Autostart eingerichtet – ab dem nächsten Login läuft die Spielerkennung automatisch.",
                     "Autostart set up – game detection runs automatically from the next login."));
    let _ = Command::new(&*BIN_PATH).arg("--daemon").stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn();
    println!("{}", t("Hintergrunddienst gestartet.", "Background service started."));
    if let Some(tool) = ["kbuildsycoca6", "kbuildsycoca5"].into_iter().find(|t| which(t)) {
        let _ = Command::new(tool).stdout(Stdio::null()).stderr(Stdio::null()).spawn(); // KDE-Menü sofort aktualisieren
    }
    if tuning_dir().is_some() && !is_writable() {
        println!("{}\n  sudo ~/.local/bin/fanatec-tuning --udev", t("\nNoch keine Schreibrechte. Einmalig ausführen:", "\nNo write permission yet. Run once:"));
    }
}

/// Von woanders gestartet oder neue Version: installieren bzw. aktualisieren, dann die installierte App starten.
fn launch(args: &[String]) {
    let me = std::env::current_exe().and_then(|p| p.canonicalize()).unwrap_or_default();
    let installed = std::fs::read_to_string(CFG_DIR.join("installed")).unwrap_or_default();
    let fresh = !BIN_PATH.exists() || !DESKTOP_PATH.exists();
    if fresh || me != BIN_PATH.canonicalize().unwrap_or_default() || installed.trim() != env!("CARGO_PKG_VERSION") {
        install();
        notify(&format!("Fanatec Tuning {}", if fresh { t("installiert", "installed") } else { t("aktualisiert", "updated") }),
               t("Ab jetzt auch im Startmenü – die Spielerkennung läuft im Hintergrund.", "Now also in the start menu – game detection runs in the background."));
    }
    fix_launcher_icons();
    let err = Command::new(&*BIN_PATH).args(args.iter().filter(|a| *a != "--launch")).env("FTEC_NO_SELFINSTALL", "1").exec();
    eprintln!("{err}");
}

fn uninstall() {
    for p in [&*BIN_PATH, &*DESKTOP_PATH, &*AUTOSTART_PATH, &*ICON_PATH] {
        if std::fs::remove_file(p).is_ok() {
            println!("{} {}", t("Entfernt:", "Removed:"), p.display());
        }
    }
    stop_other_instances();
    println!("{} {}", t("Deine Profile und Spiele bleiben erhalten:", "Your profiles and games are kept:"), CFG_DIR.display());
}

fn install_udev() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("{}  sudo ~/.local/bin/fanatec-tuning --udev", t("Bitte mit sudo ausführen, z. B.:", "Please run with sudo, e.g.:"));
        std::process::exit(1);
    }
    let _ = std::fs::create_dir_all(Path::new(UDEV_PATH).parent().unwrap());
    if let Err(e) = std::fs::write(UDEV_PATH, UDEV_RULE) {
        eprintln!("{UDEV_PATH}: {e}");
        std::process::exit(1);
    }
    let _ = Command::new("udevadm").args(["control", "--reload-rules"]).status();
    let _ = Command::new("udevadm").args(["trigger", "--subsystem-match=ftec_tuning", "--action=add"]).status();
    println!("{} {UDEV_PATH}", t("udev-Regel angelegt:", "udev rule created:"));
    println!("{}", t("Falls das Tool noch keine Rechte hat: Base einmal aus- und wieder einschalten.",
                     "If the tool still has no permission: switch the base off and on once."));
}

// --------------------------------------------------------------------- Einrichtung -----

/// Befehl, der `script` sichtbar in einem Terminal ausführt.
fn terminal_cmd(script: &str) -> Option<Vec<String>> {
    let terms: [(&str, &[&str]); 5] = [("konsole", &["--hold", "-e"]), ("kitty", &["--hold"]), ("alacritty", &["--hold", "-e"]),
                                       ("gnome-terminal", &["--"]), ("xterm", &["-hold", "-e"])];
    let (term, args) = terms.into_iter().find(|(t, _)| which(t))?;
    Some([term].iter().chain(args).chain(&["sh", "-c", script]).map(|s| s.to_string()).collect())
}

/// Offene Einrichtungsschritte: (Text, Knopf-Text + Befehl).
type Issue = (String, Option<(&'static str, Vec<String>)>);

fn setup_issues() -> Vec<Issue> {
    let mut issues: Vec<Issue> = vec![];
    if std::env::var("FTEC_DIR").is_ok() {
        return issues;
    }
    let loaded = Path::new("/sys/module/hid_fanatec").is_dir();
    let available = loaded || Command::new("modinfo").args(["-n", "hid-fanatec"]).stdout(Stdio::null()).stderr(Stdio::null())
        .status().is_ok_and(|s| s.success());
    if !available {
        match ["paru", "yay"].into_iter().find(|h| which(h)) {
            Some(helper) => {
                let script = format!("{helper} -S --needed hid-fanatecff-dkms && sudo modprobe hid-fanatec && \
                                      echo hid-fanatec | sudo tee {MODLOAD_PATH} >/dev/null && echo && echo {}",
                                     t("Fertig – Fenster kann geschlossen werden.", "Done – you can close this window."));
                issues.push((t("Der Fanatec-Treiber (hid-fanatecff) ist nicht installiert.", "The Fanatec driver (hid-fanatecff) is not installed.").into(),
                             terminal_cmd(&script).map(|c| (t("Treiber installieren", "Install driver"), c))));
            }
            None => issues.push((t("Der Fanatec-Treiber (hid-fanatecff) ist nicht installiert. Installiere das AUR-Paket \
                                    hid-fanatecff-dkms (z. B. mit paru) oder siehe github.com/gotzl/hid-fanatecff.",
                                   "The Fanatec driver (hid-fanatecff) is not installed. Install the AUR package \
                                    hid-fanatecff-dkms (e.g. with paru) or see github.com/gotzl/hid-fanatecff.").into(), None)),
        }
        return issues;
    }
    if !loaded || !Path::new(MODLOAD_PATH).exists() {
        issues.push((t("Der Fanatec-Treiber wird noch nicht automatisch geladen.", "The Fanatec driver is not loaded automatically yet.").into(),
                     Some((t("Treiber aktivieren", "Enable driver"),
                           ["pkexec", "sh", "-c", &format!("modprobe hid-fanatec; echo hid-fanatec > {MODLOAD_PATH}")].map(String::from).to_vec()))));
    }
    if tuning_dir().is_none() {
        if loaded {
            issues.push((t("Kein Fanatec-Wheel gefunden – ist die Base eingeschaltet und per USB verbunden? \
                            Nach dem Aktivieren des Treibers die Base einmal aus- und einschalten.",
                           "No Fanatec wheel found – is the base switched on and connected via USB? \
                            After enabling the driver, switch the base off and on once.").into(), None));
        }
    } else if !is_writable() {
        issues.push((t("Noch keine Schreibrechte für das Tuning-Menü.", "No write permission for the tuning menu yet.").into(),
                     Some((t("Rechte einrichten", "Set up permissions"), vec!["pkexec".into(), BIN_PATH.to_string_lossy().into(), "--udev".into()]))));
    }
    issues
}

// ------------------------------------------------------------------------- Fenster -----

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x3d, 0xae, 0xe9);
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x27, 0xae, 0x60);
const RED: egui::Color32 = egui::Color32::from_rgb(0xda, 0x44, 0x53);
const ORANGE: egui::Color32 = egui::Color32::from_rgb(0xf6, 0x74, 0x00);

#[derive(PartialEq)]
enum Page {
    Tuning,
    Games,
}

enum Dialog {
    SaveProfile(String),
    DeleteProfile(String),
    NewGame(String),
    DeleteGame(usize),
}

struct Gui {
    values: HashMap<String, i64>,
    connected: bool,
    writable: bool,
    profiles: Vec<String>,
    games: Games,
    status: Status,
    refresh_at: Instant,
    issues: Vec<Issue>,
    issues_at: Instant,
    local: HashMap<String, i64>,               // Wert im Widget, solange er noch nicht geschrieben ist
    pending: HashMap<String, (i64, Instant)>, // Schreiben nach kurzer Pause (Mausrad, Pfeiltasten)
    page: Page,
    game_row: usize,
    game_profile_row: Option<usize>,
    assign: String,
    name_buf: String,
    exe_buf: String,
    dialog: Option<Dialog>,
    message: Option<(String, Instant)>,
}

impl Gui {
    fn new() -> Self {
        let mut g = Gui {
            values: HashMap::new(), connected: false, writable: false, profiles: vec![], games: load_games(), status: Status::default(),
            refresh_at: Instant::now(), issues: vec![], issues_at: Instant::now(), local: HashMap::new(), pending: HashMap::new(),
            page: Page::Tuning, game_row: 0, game_profile_row: None, assign: String::new(), name_buf: String::new(), exe_buf: String::new(),
            dialog: None, message: None,
        };
        g.refresh();
        g.select_game(0);
        if std::env::var("FTEC_PAGE").is_ok_and(|p| p == "games") {
            g.page = Page::Games; // nur für Screenshot-Tests
        }
        g
    }

    fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now() + Duration::from_secs(5)));
    }

    fn refresh(&mut self) {
        self.connected = tuning_dir().is_some();
        self.values = read_all().unwrap_or_default();
        self.writable = is_writable();
        self.profiles = load_profiles().keys().cloned().collect();
        self.status = status();
        self.refresh_at = Instant::now() + POLL;
    }

    /// Spiele neu einlesen, außer der Nutzer tippt gerade in einem Feld.
    fn reload_games(&mut self, ctx: &egui::Context) {
        let games = load_games();
        if games != self.games && ctx.memory(|m| m.focused().is_none()) {
            self.games = games;
            self.select_game(self.game_row.min(self.games.games.len().saturating_sub(1)));
        }
    }

    fn select_game(&mut self, row: usize) {
        self.game_row = row;
        self.game_profile_row = None;
        let g = self.games.games.get(row);
        self.name_buf = g.map_or(String::new(), |g| g.name.clone());
        self.exe_buf = g.map_or(String::new(), |g| g.exe.clone());
    }

    fn store_games(&mut self, ok: String) {
        match save_games(&self.games) {
            Ok(()) => self.say(ok),
            Err(e) => self.say(format!("⚠ {e}")),
        }
        self.games = load_games();
        self.select_game(self.game_row.min(self.games.games.len().saturating_sub(1)));
    }

    fn apply(&mut self, name: &str) {
        match apply_profile(name, "") {
            Ok(w) if !w.is_empty() => self.say(format!("⚠ {}", w.join(" | "))),
            Ok(_) => self.say(tf(format!("Profil „{name}“ gesetzt"), format!("Profile \"{name}\" applied"))),
            Err(e) => self.say(format!("⚠ {e}")),
        }
        self.refresh();
    }

    fn write_due(&mut self) {
        let now = Instant::now();
        let due: Vec<(String, i64)> = self.pending.iter().filter(|(_, (_, at))| *at <= now).map(|(k, (v, _))| (k.clone(), *v)).collect();
        for (key, v) in due {
            self.pending.remove(&key);
            self.local.remove(&key);
            match write_value(&key, v) {
                Ok(()) => {
                    self.values.insert(key.clone(), v);
                    self.say(format!("{key} = {v}"));
                }
                Err(e) => self.say(format!("⚠ {e}")),
            }
        }
    }

    fn send(&mut self, key: &str, v: i64, delay_ms: u64) {
        self.local.insert(key.into(), v);
        self.pending.insert(key.into(), (v, Instant::now() + Duration::from_millis(delay_ms)));
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Fanatec Tuning").size(22.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.connected {
                    ui.colored_label(GREEN, t("• Wheel verbunden", "• Wheel connected"));
                } else {
                    ui.colored_label(RED, t("• Kein Wheel gefunden", "• No wheel found"));
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(t("Profil:", "Profile:")).strong());
            for name in self.profiles.clone() {
                let active = self.status.profile.as_ref() == Some(&name);
                let r = ui.selectable_label(active, &name).on_hover_text(if active { t("Aktives Profil", "Active profile") } else { t("Klicken zum Anwenden", "Click to apply") });
                if r.clicked() {
                    self.apply(&name);
                }
                r.context_menu(|ui| {
                    if ui.button(t("Löschen …", "Delete …")).clicked() {
                        self.dialog = Some(Dialog::DeleteProfile(name.clone()));
                        ui.close();
                    }
                });
            }
            if ui.button(t("💾 Speichern als …", "💾 Save as …")).on_hover_text(t("Aktuelle Werte als Profil speichern", "Save current values as a profile")).clicked() {
                self.dialog = Some(Dialog::SaveProfile(String::new()));
            }
        });
        if let Some(game) = &self.status.game {
            let profile = self.status.profile.as_ref().map_or(String::new(), |p| format!("{}{p}", t("  ·  Profil: ", "  ·  Profile: ")));
            ui.label(egui::RichText::new(format!("▶ {game} {}{profile}", t("läuft", "running"))).color(GREEN).strong());
        }
        if !self.issues.is_empty() {
            egui::Frame::new().stroke(egui::Stroke::new(1.0_f32, ORANGE)).corner_radius(6).inner_margin(10).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.colored_label(ORANGE, t("Einrichtung", "Setup"));
                for (text, action) in self.issues.clone() {
                    ui.horizontal(|ui| {
                        if let Some((label, cmd)) = action {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(label).clicked() {
                                    let _ = Command::new(&cmd[0]).args(&cmd[1..]).spawn().map(|mut c| std::thread::spawn(move || c.wait()));
                                    self.issues_at = Instant::now() + Duration::from_secs(3);
                                    self.say(t("Einrichtung läuft … ggf. Passwort eingeben", "Setting up … enter your password if asked"));
                                }
                                ui.add(egui::Label::new(format!("⚠ {text}")).wrap());
                            });
                        } else {
                            ui.add(egui::Label::new(format!("⚠ {text}")).wrap());
                        }
                    });
                }
            });
        }
    }

    fn tuning_page(&mut self, ui: &mut egui::Ui) {
        let enabled = self.connected && self.writable;
        for group in [Group::Basic, Group::Effects, Group::Natural] {
            card(ui, group.label(), |ui| self.params(ui, group, enabled));
        }
        egui::CollapsingHeader::new(Group::Expert.label()).default_open(false).show(ui, |ui| self.params(ui, Group::Expert, enabled));
        ui.add_space(6.0);
        ui.label(egui::RichText::new(t("Änderungen werden sofort in den aktuellen Slot der Base geschrieben und dort gespeichert.",
                                       "Changes are written to the base's current slot right away and stored there.")).small().weak());
    }

    fn params(&mut self, ui: &mut egui::Ui, group: Group, enabled: bool) {
        egui::Grid::new(group.label()).num_columns(2).spacing([16.0, 10.0]).min_col_width(250.0).show(ui, |ui| {
            for p in PARAMS.iter().filter(|p| p.group == group) {
                ui.vertical(|ui| {
                    ui.set_max_width(250.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(p.label).strong());
                        ui.label(egui::RichText::new(format!("({})", p.key)).small().weak());
                    });
                    ui.add(egui::Label::new(egui::RichText::new(p.desc).small().weak()).wrap());
                });
                let present = self.values.contains_key(p.key);
                let mut v = self.local.get(p.key).or(self.values.get(p.key)).copied().unwrap_or(p.lo);
                ui.add_enabled_ui(present && enabled, |ui| match p.kind {
                    Kind::Toggle => {
                        let (off, on) = toggle_labels(p.key);
                        let mut checked = v >= p.hi;
                        let label = if checked { on } else { off };
                        if ui.checkbox(&mut checked, label).changed() {
                            self.send(p.key, if checked { p.hi } else { p.lo }, 0);
                        }
                    }
                    Kind::Slider => {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(120.0);
                            let r = ui.add(egui::Slider::new(&mut v, p.lo..=p.hi).step_by(p.step as f64).show_value(false));
                            let d = ui.add(egui::DragValue::new(&mut v).range(p.lo..=p.hi).speed(p.step as f64));
                            let dragging = r.dragged() || d.dragged();
                            if r.changed() || d.changed() {
                                // beim Ziehen erst am Ende schreiben, Mausrad/Tasten nach kurzer Pause
                                self.local.insert(p.key.into(), v);
                                if !dragging {
                                    self.send(p.key, v, 350);
                                }
                            }
                            if r.drag_stopped() || d.drag_stopped() {
                                self.send(p.key, v, 0);
                            }
                        });
                    }
                    Kind::Number => {
                        let d = ui.add(egui::DragValue::new(&mut v).range(p.lo..=p.hi).speed(p.step as f64));
                        if d.changed() && !d.dragged() || d.drag_stopped() {
                            self.send(p.key, v, 0);
                        } else if d.changed() {
                            self.local.insert(p.key.into(), v);
                        }
                    }
                });
                ui.end_row();
            }
        });
    }

    fn games_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            ui.vertical(|left| {
            left.set_width(240.0);
            card(left, t("Spiele", "Games"), |ui| {
                for (i, g) in self.games.games.clone().iter().enumerate() {
                    let running = self.status.game.as_ref() == Some(&g.name);
                    let text = if running { format!("▶ {}", g.name) } else { g.name.clone() };
                    if ui.selectable_label(i == self.game_row, text).on_hover_text(&g.exe).clicked() {
                        self.select_game(i);
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button(t("➕ Neues Spiel", "➕ New game")).clicked() {
                        self.dialog = Some(Dialog::NewGame(String::new()));
                    }
                    if ui.add_enabled(self.game_row < self.games.games.len(), egui::Button::new(t("➖ Entfernen", "➖ Remove"))).clicked() {
                        self.dialog = Some(Dialog::DeleteGame(self.game_row));
                    }
                });
            });
            });
            ui.vertical(|right| {
                if self.game_row >= self.games.games.len() {
                    right.label(egui::RichText::new(t("Noch kein Spiel angelegt.", "No game added yet.")).weak());
                    return;
                }
                card(right, t("Spiel", "Game"), |ui| self.game_form(ui));
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(t("Nach Spielende:", "After the game exits:"));
            let cur = self.games.after_exit.clone();
            let label = |n: &str| if n.is_empty() { t("nichts ändern", "change nothing").to_string() } else { tf(format!("Profil „{n}“ laden"), format!("load profile \"{n}\"")) };
            let mut pick = None;
            egui::ComboBox::from_id_salt("after").selected_text(label(&cur)).show_ui(ui, |ui| {
                for n in [String::new()].into_iter().chain(self.profiles.clone()) {
                    if ui.selectable_label(n == cur, label(&n)).clicked() {
                        pick = Some(n);
                    }
                }
            });
            if let Some(n) = pick {
                self.games.after_exit = n;
                self.store_games(t("Gespeichert", "Saved").into());
            }
        });
    }

    fn game_form(&mut self, ui: &mut egui::Ui) {
        let row = self.game_row;
        egui::Grid::new("game").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label("Name:");
            let n = ui.add(egui::TextEdit::singleline(&mut self.name_buf).desired_width(330.0));
            ui.end_row();
            ui.label(t("Programm:", "Program:"));
            let mut picked = None;
            let e = ui.horizontal(|ui| {
                let e = ui.add(egui::TextEdit::singleline(&mut self.exe_buf).hint_text(t("z. B. acs.exe", "e.g. acs.exe")).desired_width(150.0));
                ui.menu_button(t("Laufendes Programm …", "Running program …"), |ui| {
                    let exes = running_game_exes();
                    if exes.is_empty() {
                        ui.label(t("Es läuft gerade kein Windows-Programm.\nStarte das Spiel und versuche es dann nochmal.",
                                   "No Windows program is running right now.\nStart the game and try again."));
                    }
                    for exe in exes {
                        if ui.button(&exe).clicked() {
                            picked = Some(exe);
                            ui.close();
                        }
                    }
                }).response.on_hover_text(t("Spiel starten, dann hier die .exe aus der Liste wählen", "Start the game, then pick its .exe from the list here"));
                e
            }).inner;
            ui.end_row();
            if let Some(exe) = picked {
                self.exe_buf = exe;
            }
            let g = &self.games.games[row];
            let (name, exe) = (self.name_buf.trim().to_string(), self.exe_buf.trim().to_string());
            let committed = n.lost_focus() || e.lost_focus() || exe != g.exe && !e.has_focus();
            if committed && !name.is_empty() && !exe.is_empty() && (name != g.name || exe != g.exe) {
                (self.games.games[row].name, self.games.games[row].exe) = (name, exe);
                self.store_games(t("Gespeichert", "Saved").into());
            }

            ui.label(t("Profile:", "Profiles:"));
            ui.vertical(|ui| {
                for (i, p) in self.games.games[row].profiles.clone().iter().enumerate() {
                    let text = if i == 0 { format!("★ {p}") } else { format!("    {p}") };
                    if ui.selectable_label(self.game_profile_row == Some(i), text).clicked() {
                        self.game_profile_row = Some(i);
                    }
                }
            });
            ui.end_row();
            ui.label("");
            ui.horizontal_wrapped(|ui| {
                if !self.profiles.contains(&self.assign) {
                    self.assign = self.profiles.first().cloned().unwrap_or_default();
                }
                egui::ComboBox::from_id_salt("assign").selected_text(self.assign.clone()).show_ui(ui, |ui| {
                    for p in &self.profiles {
                        ui.selectable_value(&mut self.assign, p.clone(), p);
                    }
                });
                if ui.button(t("➕ Zuordnen", "➕ Assign")).clicked() && !self.assign.is_empty() && !self.games.games[row].profiles.contains(&self.assign) {
                    let p = self.assign.clone();
                    self.games.games[row].profiles.push(p.clone());
                    self.store_games(tf(format!("„{p}“ zugeordnet"), format!("\"{p}\" assigned")));
                }
                let sel = self.game_profile_row.filter(|i| *i < self.games.games[row].profiles.len());
                if ui.add_enabled(sel.is_some_and(|i| i > 0), egui::Button::new(t("★ Startprofil", "★ Start profile"))).clicked() {
                    let profiles = &mut self.games.games[row].profiles;
                    let p = profiles.remove(sel.unwrap());
                    profiles.insert(0, p.clone());
                    self.store_games(tf(format!("„{p}“ ist jetzt das Startprofil"), format!("\"{p}\" is now the start profile")));
                }
                if ui.add_enabled(sel.is_some(), egui::Button::new(t("➖ Entfernen", "➖ Remove"))).on_hover_text(t("Profil aus diesem Spiel entfernen", "Remove profile from this game")).clicked() {
                    let p = self.games.games[row].profiles.remove(sel.unwrap());
                    self.store_games(tf(format!("„{p}“ entfernt"), format!("\"{p}\" removed")));
                }
            });
            ui.end_row();
        });
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(t(
            "Das Profil mit ★ wird beim Spielstart geladen. Mit einem Tastenkürzel für „fanatec-tuning --next“ schaltest du im Spiel durch alle Profile dieses Spiels.",
            "The profile with ★ is loaded when the game starts. A keyboard shortcut for \"fanatec-tuning --next\" cycles through this game's profiles in game.")).small().weak()).wrap());
    }

    fn dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.dialog else { return };
        let mut close = false;
        let mut action: Option<Box<dyn FnOnce(&mut Gui)>> = None;
        let modal = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
            ui.set_width(340.0);
            let buttons = |ui: &mut egui::Ui, ok: &str, enabled: bool| -> (bool, bool) {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ok = ui.add_enabled(enabled, egui::Button::new(ok)).clicked();
                    (ok, ui.button(t("Abbrechen", "Cancel")).clicked())
                }).inner
            };
            match dialog {
                Dialog::SaveProfile(name) => {
                    ui.heading(t("Profil speichern", "Save profile"));
                    ui.label(t("Name für die aktuellen Werte:", "Name for the current values:"));
                    let r = ui.text_edit_singleline(name);
                    r.request_focus();
                    let n: String = name.trim().chars().take(40).collect();
                    let exists = self.profiles.contains(&n);
                    if exists {
                        ui.colored_label(ORANGE, tf(format!("Profil „{n}“ gibt es schon und wird überschrieben."), format!("Profile \"{n}\" already exists and will be overwritten.")));
                    }
                    let (ok, cancel) = buttons(ui, if exists { t("Überschreiben", "Overwrite") } else { t("Speichern", "Save") }, !n.is_empty());
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !n.is_empty();
                    if ok || enter {
                        action = Some(Box::new(move |g: &mut Gui| {
                            let values = read_all().unwrap_or_default();
                            let mut profiles = load_profiles();
                            let v: Map<String, Value> = PARAMS.iter().filter(|p| is_profile_key(p.key))
                                .filter_map(|p| Some((p.key.to_string(), json!(values.get(p.key)?)))).collect();
                            profiles.insert(n.clone(), Value::Object(v));
                            match save_profiles(&profiles) {
                                Ok(()) => g.say(tf(format!("Profil „{n}“ gespeichert"), format!("Profile \"{n}\" saved"))),
                                Err(e) => g.say(format!("⚠ {e}")),
                            }
                        }));
                    }
                    close = ok || cancel || enter;
                }
                Dialog::DeleteProfile(name) => {
                    ui.heading(t("Profil löschen", "Delete profile"));
                    ui.label(tf(format!("Profil „{name}“ löschen?\nEs wird auch aus allen Spielen entfernt."),
                                format!("Delete profile \"{name}\"?\nIt is also removed from all games.")));
                    let (ok, cancel) = buttons(ui, t("Löschen", "Delete"), true);
                    if ok {
                        let n = name.clone();
                        action = Some(Box::new(move |g: &mut Gui| {
                            let mut profiles = load_profiles();
                            profiles.remove(&n);
                            let _ = save_profiles(&profiles);
                            for game in &mut g.games.games {
                                game.profiles.retain(|p| *p != n);
                            }
                            if g.games.after_exit == n {
                                g.games.after_exit.clear();
                            }
                            g.store_games(tf(format!("Profil „{n}“ gelöscht"), format!("Profile \"{n}\" deleted")));
                        }));
                    }
                    close = ok || cancel;
                }
                Dialog::NewGame(name) => {
                    ui.heading(t("Neues Spiel", "New game"));
                    ui.label(t("Name des Spiels:", "Name of the game:"));
                    let r = ui.text_edit_singleline(name);
                    r.request_focus();
                    let n = name.trim().to_string();
                    let (ok, cancel) = buttons(ui, t("Anlegen", "Add"), !n.is_empty());
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !n.is_empty();
                    if ok || enter {
                        action = Some(Box::new(move |g: &mut Gui| {
                            g.games.games.push(Game { name: n, exe: t("neues-spiel.exe", "new-game.exe").into(), profiles: vec![] });
                            g.game_row = g.games.games.len() - 1;
                            g.store_games(t("Spiel angelegt – jetzt noch das Programm eintragen", "Game added – now enter its program").into());
                        }));
                    }
                    close = ok || cancel || enter;
                }
                Dialog::DeleteGame(row) => {
                    let row = *row;
                    let name = self.games.games.get(row).map_or(String::new(), |g| g.name.clone());
                    ui.heading(t("Spiel entfernen", "Remove game"));
                    ui.label(tf(format!("„{name}“ entfernen?"), format!("Remove \"{name}\"?")));
                    let (ok, cancel) = buttons(ui, t("Entfernen", "Remove"), true);
                    if ok {
                        action = Some(Box::new(move |g: &mut Gui| {
                            if row < g.games.games.len() {
                                g.games.games.remove(row);
                                g.game_row = row.saturating_sub(1);
                                g.store_games(t("Spiel entfernt", "Game removed").into());
                            }
                        }));
                    }
                    close = ok || cancel;
                }
            }
        });
        if close || modal.should_close() {
            self.dialog = None;
        }
        if let Some(a) = action {
            a(self);
            self.refresh();
        }
    }
}

/// Abgerundeter Abschnitt mit kleiner Überschrift.
fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(10).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(title.to_uppercase()).size(11.5).strong().color(ACCENT));
        ui.add_space(2.0);
        body(ui);
    });
    ui.add_space(6.0);
}

fn style(ctx: &egui::Context) {
    use egui::{Color32, CornerRadius, Stroke};
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(0x15, 0x17, 0x1c);
    v.faint_bg_color = Color32::from_rgb(0x1f, 0x22, 0x29);
    v.extreme_bg_color = Color32::from_rgb(0x0f, 0x11, 0x15);
    v.selection.bg_fill = Color32::from_rgb(0x1d, 0x6f, 0x9c);
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.slider_trailing_fill = true;
    let w = &mut v.widgets;
    for (wv, fill) in [(&mut w.inactive, 0x2a), (&mut w.hovered, 0x36), (&mut w.active, 0x40), (&mut w.open, 0x30)] {
        wv.bg_fill = Color32::from_gray(fill);
        wv.weak_bg_fill = Color32::from_gray(fill);
        wv.corner_radius = CornerRadius::same(4);
        wv.bg_stroke = Stroke::NONE;
    }
    w.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.noninteractive.corner_radius = CornerRadius::same(4);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.interact_size.y = 24.0;
    });
}

impl App for Gui {
    fn ui(&mut self, ui: &mut egui::Ui, _backend: &mut SoftwareBackend) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        self.write_due();
        if now >= self.refresh_at {
            self.refresh();
            self.reload_games(&ctx);
        }
        if now >= self.issues_at {
            self.issues = setup_issues();
            self.issues_at = now + Duration::from_secs(10);
        }
        if self.message.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.message = None;
        }
        egui::Panel::bottom("status").show_inside(ui, |ui| {
            ui.label(self.message.as_ref().map_or("", |m| m.0.as_str()));
        });
        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.header(ui);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.page, Page::Tuning, egui::RichText::new("⚙ Tuning").size(15.0));
                ui.selectable_value(&mut self.page, Page::Games, egui::RichText::new(t("🎮 Spiele", "🎮 Games")).size(15.0));
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| match self.page {
                Page::Tuning => self.tuning_page(ui),
                Page::Games => self.games_page(ui),
            });
        });
        self.dialog(&ctx);
        let soon = self.pending.values().map(|(_, at)| *at).chain([self.refresh_at]).min().unwrap();
        ctx.request_repaint_after(soon.saturating_duration_since(Instant::now()).max(Duration::from_millis(16)));
    }

    fn on_exit(&mut self, _ctx: &egui::Context) {
        for (_, at) in self.pending.values_mut() {
            *at = Instant::now();
        }
        self.write_due();
    }
}

fn run_gui() {
    let Some(_lock) = lock("fanatec-tuning-window.lock") else { return }; // Fenster ist schon offen
    if !daemon_running() {
        spawn_self(&["--daemon"]);
    }
    let mut config = SoftwareBackendAppConfiguration::default();
    config.viewport_builder = egui::ViewportBuilder::default().with_title("Fanatec Tuning").with_app_id("fanatec-tuning")
        .with_inner_size([820.0, 780.0]);
    let _ = egui_software_backend::run_app_with_software_backend(config, |ctx| {
        style(&ctx);
        Gui::new()
    });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    if has("--launch") {
        return launch(&args);
    }
    if has("--install") {
        return install();
    }
    if has("--uninstall") {
        return uninstall();
    }
    if has("--udev") {
        return install_udev();
    }
    if has("--next") {
        return cli_profile(true, None);
    }
    if let Some(i) = args.iter().position(|a| a == "--apply") {
        let Some(name) = args.get(i + 1) else {
            eprintln!("{} fanatec-tuning --apply NAME", t("Bitte Profilnamen angeben:", "Please give a profile name:"));
            std::process::exit(1);
        };
        return cli_profile(false, Some(name));
    }
    if has("-h") || has("--help") {
        return print!("{}", if *DE { HELP_DE } else { HELP_EN });
    }
    if has("--daemon") {
        return run_daemon();
    }
    let me = std::env::current_exe().and_then(|p| p.canonicalize()).unwrap_or_default();
    if me != BIN_PATH.canonicalize().unwrap_or_default() && std::env::var("FTEC_NO_SELFINSTALL").is_err() {
        return launch(&args); // Selbstinstallation beim Start von woanders
    }
    run_gui();
}
