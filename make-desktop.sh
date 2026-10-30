#!/usr/bin/env bash
# Rebuilds Fanatec-Tuning.desktop: the one-click launcher with the whole program embedded
# (compressed) in its X-Fanatec-Code line. Run after every change to fanatec-tuning.
set -euo pipefail
cd "$(dirname "$0")"
code=$(python3 -I -c 'import sys,zlib,base64; print(base64.b64encode(zlib.compress(open("fanatec-tuning","rb").read(),9)).decode())')
head -n -1 Fanatec-Tuning.desktop > Fanatec-Tuning.desktop.new
echo "X-Fanatec-Code=$code" >> Fanatec-Tuning.desktop.new
mv Fanatec-Tuning.desktop.new Fanatec-Tuning.desktop
echo "Fanatec-Tuning.desktop neu gebaut."
