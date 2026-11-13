#!/usr/bin/env bash
set -euo pipefail

# scripts/check-boundaries.sh
# Checks architectural boundary rules across workspace crates.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# R1: remoport-rdp must not depend on UI or platform crates
if ! TREE_OUTPUT=$(cargo tree -p remoport-rdp -e normal --prefix none --format '{p}' 2>&1); then
    echo "ERROR: Failed to inspect dependencies for remoport-rdp:" >&2
    echo "$TREE_OUTPUT" >&2
    exit 1
fi

if ! echo "$TREE_OUTPUT" | grep -q '^remoport-rdp v'; then
    echo "ERROR: cargo tree output does not contain remoport-rdp root crate." >&2
    exit 1
fi

FORBIDDEN_R1="egui|eframe|cpal|wayland-client|keyring|winit"
VIOLATIONS_R1=$(echo "$TREE_OUTPUT" | grep -E "^($FORBIDDEN_R1) v" || true)

if [ -n "$VIOLATIONS_R1" ]; then
    echo "ERROR: Boundary rule R1 broken: remoport-rdp depends on UI or platform crates:" >&2
    echo "$VIOLATIONS_R1" >&2
    exit 1
fi

echo "Boundary checks passed (R1: remoport-rdp is independent of UI and platform)."
