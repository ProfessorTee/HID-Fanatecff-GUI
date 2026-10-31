# HID-Fanatecff-GUI (Fanatec Tuning)

[English](README.md) · **Deutsch**

**Grafische Oberfläche für das Tuning-Menü von Fanatec-Wheelbases unter Linux**, aufbauend auf dem
Kerneltreiber [hid-fanatecff](https://github.com/gotzl/hid-fanatecff). Lenkwinkel, Force Feedback,
Dämpfung, Glättung usw. lassen sich per Schieberegler einstellen. Dazu kommen Profile, die beim Start
eines Spiels automatisch geladen werden.

Die Oberfläche folgt der Systemsprache: Deutsch auf deutschen Systemen, sonst Englisch.

## Funktionen

- **Alle Tuning-Werte** der Base (SEN, FF, FFS, FUL, FEI, FOR, SPR, DPR, SHO, NDP, NFR, NIN, INT, Slot, …)
  mit verständlichen Namen und Empfehlungen. Änderungen landen sofort im aktuellen Slot der Base.
- **Profile** speichern und laden. Mitgeliefert: *Empfohlen*, *GT*, *Rally*, *Drift*.
- **Automatische Spielerkennung:** Ein Hintergrunddienst erkennt laufende Spiele (auch unter Proton) an
  ihrer `.exe` und setzt das zugeordnete Profil. Nach Spielende optional ein anderes Profil.
- **Profilwechsel im Spiel** per Tastenkürzel auf `fanatec-tuning --next`.
- **Tray-Icon** des Hintergrunddienstes: Profil wechseln, Fenster öffnen.
- **Ein Programm** (Rust, ~3 MB), keine Abhängigkeiten außer dem Treiber. Das Fenster läuft nur, solange es offen ist.

## Installation

**Ein Klick:** `Fanatec-Tuning.desktop` herunterladen und doppelklicken (bei KDE ggf. erst
„Ausführen erlauben“). Die Datei lädt das aktuelle Release von GitHub, installiert es nach `~/.local/bin`,
legt Startmenü-Eintrag und Autostart an und startet es. Jeder weitere Doppelklick holt die neueste Version.
Ohne Internet startet einfach die installierte Version.

**Oder aus dem Quellcode** (braucht `cargo`, Arch/CachyOS: `sudo pacman -S rust`):

```
git clone https://github.com/ProfessorTee/HID-Fanatecff-GUI.git
cd HID-Fanatecff-GUI
cargo build --release
./target/release/fanatec-tuning --install
```

**Schreibrechte (einmalig):** Damit normale Benutzer die Werte ändern dürfen, eine udev-Regel anlegen und
danach die Base aus- und wieder einschalten:

```
sudo ~/.local/bin/fanatec-tuning --udev
```

Entfernen: `fanatec-tuning --uninstall` (Profile in `~/.config/fanatec-tuning` bleiben erhalten).

## Befehle

| Befehl | Wirkung |
|---|---|
| `fanatec-tuning` | Desktop-App öffnen |
| `fanatec-tuning --daemon` | Spielerkennung + Tray-Icon im Hintergrund (Autostart) |
| `fanatec-tuning --next` | nächstes Profil des laufenden Spiels |
| `fanatec-tuning --apply NAME` | Profil NAME setzen |
| `fanatec-tuning --install` / `--uninstall` | installieren / entfernen |
| `sudo fanatec-tuning --udev` | Schreibrechte einrichten |

## Voraussetzungen

- Linux mit dem Treiber [hid-fanatecff](https://github.com/gotzl/hid-fanatecff) (stellt
  `/sys/class/ftec_tuning/` bereit)
- Entwickelt und getestet mit einer Fanatec CSL DD (Boost Kit) unter KDE Plasma 6 (CachyOS)

## Entwicklung

Rust-Programm in `src/main.rs` (egui-Fenster mit CPU-Rendering, ksni fürs Tray). Ein Tag `v*` baut per
GitHub Actions auf Ubuntu 22.04 (läuft damit auf glibc ≥ 2.35) und hängt das Programm ans Release, das
der Ein-Klick-Starter lädt.

## Lizenz

MIT. hid-fanatecff ist ein eigenes Projekt (gotzl) und nicht Teil dieses Repos.
