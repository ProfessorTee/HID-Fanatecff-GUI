# HID-Fanatecff-GUI (Fanatec Tuning)

**Grafische Oberfläche für das Tuning-Menü von Fanatec-Wheelbases unter Linux**, aufbauend auf dem
Kerneltreiber [hid-fanatecff](https://github.com/gotzl/hid-fanatecff). Lenkwinkel, Force Feedback,
Dämpfung, Glättung usw. lassen sich per Schieberegler einstellen. Dazu kommen Profile, die beim Start
eines Spiels automatisch geladen werden.

## Funktionen

- **Alle Tuning-Werte** der Base (SEN, FF, FFS, FUL, FEI, FOR, SPR, DPR, SHO, NDP, NFR, NIN, INT, Slot, …)
  mit verständlichen Namen und Empfehlungen. Änderungen landen sofort im aktuellen Slot der Base.
- **Profile** speichern und laden. Mitgeliefert: *Empfohlen*, *GT*, *Rally*, *Drift*.
- **Automatische Spielerkennung:** Ein Hintergrunddienst erkennt laufende Spiele (auch unter Proton) an
  ihrer `.exe` und setzt das zugeordnete Profil. Nach Spielende optional ein anderes Profil.
- **Profilwechsel im Spiel** per Tastenkürzel auf `fanatec-tuning --next`.
- **Desktop-App** mit PySide6, ohne PySide6 automatisch als **Browser-Oberfläche** (nur auf `127.0.0.1`).

## Installation

**Ein Klick:** `Fanatec-Tuning.desktop` herunterladen und doppelklicken (bei KDE ggf. erst
„Ausführen erlauben“). Die Datei enthält das komplette Programm, installiert es nach `~/.local/bin`,
legt Startmenü-Eintrag und Autostart an und startet es. Ein neuerer Starter aktualisiert die Installation
beim nächsten Doppelklick.

**Oder per Terminal:**

```
git clone https://github.com/ProfessorTee/HID-Fanatecff-GUI.git
cd HID-Fanatecff-GUI
./fanatec-tuning --install
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
| `fanatec-tuning --web` | Oberfläche im Browser öffnen |
| `fanatec-tuning --daemon` | nur Spielerkennung im Hintergrund (Autostart) |
| `fanatec-tuning --next` | nächstes Profil des laufenden Spiels |
| `fanatec-tuning --apply NAME` | Profil NAME setzen |
| `fanatec-tuning --install` / `--uninstall` | installieren / entfernen |
| `sudo fanatec-tuning --udev` | Schreibrechte einrichten |

## Voraussetzungen

- Linux mit dem Treiber [hid-fanatecff](https://github.com/gotzl/hid-fanatecff) (stellt
  `/sys/class/ftec_tuning/` bereit)
- Python 3, für die Desktop-App zusätzlich PySide6 (Arch/CachyOS: `sudo pacman -S pyside6`)
- Entwickelt und getestet mit einer Fanatec CSL DD (Boost Kit) unter KDE Plasma 6 (CachyOS)

## Entwicklung

Das Programm ist die eine Datei `fanatec-tuning`. Nach Änderungen `./make-desktop.sh` ausführen, damit
der Ein-Klick-Starter das neue Programm enthält.

## Lizenz

MIT. hid-fanatecff ist ein eigenes Projekt (gotzl) und nicht Teil dieses Repos.
