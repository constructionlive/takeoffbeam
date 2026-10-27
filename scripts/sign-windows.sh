#!/usr/bin/env bash
# Authenticode-signs one file in place with the command in WIN_SIGN. makensis calls this for
# the uninstaller and the installer (see scripts/package-windows.sh).
set -euo pipefail
eval "$WIN_SIGN \"\$1\""
