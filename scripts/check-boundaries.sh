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

if ! grep -q '^remoport-rdp v' <<< "$TREE_OUTPUT"; then
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

# Phase 2 session separation: signin and controller never mention egui; display only uses geometry types
for file in remoport-core/src/session/signin.rs remoport-core/src/session/controller.rs; do
    if BAD=$(grep -nE 'egui::|\buse egui\b' "$file"); then
        echo "ERROR: $file must not reference egui:" >&2
        echo "$BAD" >&2
        exit 1
    fi
done

BAD_DISPLAY_EGUI=$(grep -nE 'egui::|\buse egui\b' remoport-core/src/session/display.rs \
    | sed -E 's/^[0-9]+://; s/egui::|\b(use|pos2|vec2|Pos2|Rect|Vec2)\b//g; s/[{},;[:space:]]//g' \
    | grep -v '^$' || true)
if [ -n "$BAD_DISPLAY_EGUI" ]; then
    echo "ERROR: remoport-core/src/session/display.rs uses non-geometry egui items: $BAD_DISPLAY_EGUI" >&2
    exit 1
fi

echo "Boundary checks passed (R1: remoport-rdp independent; session submodules clean)."
