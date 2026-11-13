#!/usr/bin/env bash
set -euo pipefail

# scripts/check-boundaries.sh
# Checks architectural boundary rules across workspace crates.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# R1: remoport-rdp must not depend on UI or platform crates
FORBIDDEN_R1="egui|eframe|cpal|wayland-client|keyring|winit"
VIOLATIONS_R1=$(cargo tree -p remoport-rdp -e normal | grep -E "\b($FORBIDDEN_R1)\b" || true)

if [ -n "$VIOLATIONS_R1" ]; then
    echo "ERROR: Boundary rule R1 broken: remoport-rdp depends on UI or platform crates ($FORBIDDEN_R1):" >&2
    echo "$VIOLATIONS_R1" >&2
    exit 1
fi

echo "Boundary checks passed (R1: remoport-rdp is independent of UI and platform)."
