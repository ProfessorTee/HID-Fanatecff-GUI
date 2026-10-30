# HID-Fanatecff-GUI (Fanatec Tuning)

**English** · [Deutsch](README.de.md)

**Graphical interface for the tuning menu of Fanatec wheel bases on Linux**, built on the kernel driver
[hid-fanatecff](https://github.com/gotzl/hid-fanatecff). Steering angle, force feedback, damping, smoothing
etc. can be set with sliders, plus profiles that are loaded automatically when a game starts.

The interface follows the system language: German on German systems, English otherwise.

## Features

- **All tuning values** of the base (SEN, FF, FFS, FUL, FEI, FOR, SPR, DPR, SHO, NDP, NFR, NIN, INT, slot, …)
  with readable names and recommendations. Changes go straight into the base's current slot.
- **Profiles** to save and load. Included: *Empfohlen* (recommended), *GT*, *Rally*, *Drift*.
- **Automatic game detection:** a background service detects running games (also under Proton) by their
  `.exe` and applies the assigned profile. Optionally a different profile after the game exits.
- **Switch profiles in game** with a keyboard shortcut bound to `fanatec-tuning --next`.
- **Desktop app** with PySide6; without PySide6 it falls back to a **browser interface** (only on `127.0.0.1`).

## Installation

**One click:** download `Fanatec-Tuning.desktop` and double-click it (on KDE you may have to "allow
executing" first). The file contains the whole program, installs it to `~/.local/bin`, adds a start menu
entry and autostart, and launches it. A newer launcher updates the installation on the next double-click.

**Or via terminal:**

```
git clone https://github.com/ProfessorTee/HID-Fanatecff-GUI.git
cd HID-Fanatecff-GUI
./fanatec-tuning --install
```

**Write permissions (once):** so normal users may change the values, add a udev rule and then switch the
base off and on again:

```
sudo ~/.local/bin/fanatec-tuning --udev
```

Remove: `fanatec-tuning --uninstall` (profiles in `~/.config/fanatec-tuning` are kept).

## Commands

| Command | Effect |
|---|---|
| `fanatec-tuning` | open the desktop app |
| `fanatec-tuning --web` | open the interface in the browser |
| `fanatec-tuning --daemon` | only game detection in the background (autostart) |
| `fanatec-tuning --next` | next profile of the running game |
| `fanatec-tuning --apply NAME` | apply profile NAME |
| `fanatec-tuning --install` / `--uninstall` | install / remove |
| `sudo fanatec-tuning --udev` | set up write permissions |

## Requirements

- Linux with the driver [hid-fanatecff](https://github.com/gotzl/hid-fanatecff) (provides
  `/sys/class/ftec_tuning/`)
- Python 3, plus PySide6 for the desktop app (Arch/CachyOS: `sudo pacman -S pyside6`)
- Developed and tested with a Fanatec CSL DD (Boost Kit) on KDE Plasma 6 (CachyOS)

## Development

The program is the single file `fanatec-tuning`. After changes, run `./make-desktop.sh` so the one-click
launcher contains the new program.

## License

MIT. hid-fanatecff is a separate project (gotzl) and not part of this repo.
