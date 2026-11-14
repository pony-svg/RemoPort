#!/usr/bin/env bash
set -euo pipefail

# scripts/check-boundaries.sh
# Checks architectural boundary rules (R1-R6) across workspace crates and layers.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

check_forbidden_deps() {
    local crate="$1"
    local rule="$2"
    local forbidden="$3"
    local tree_output
    if ! tree_output=$(cargo tree -p "$crate" -e normal --prefix none --format '{p}' 2>&1); then
        echo "ERROR: Failed to inspect dependencies for $crate:" >&2
        echo "$tree_output" >&2
        exit 1
    fi

    if ! grep -q "^${crate} v" <<< "$tree_output"; then
        echo "ERROR: cargo tree output does not contain ${crate} root crate." >&2
        exit 1
    fi

    local violations
    violations=$(grep -E "^(${forbidden}) v" <<< "$tree_output" || true)
    if [ -n "$violations" ]; then
        echo "ERROR: Boundary rule $rule broken: $crate depends on forbidden crates:" >&2
        echo "$violations" >&2
        exit 1
    fi
}

FORBIDDEN_UI_PLATFORM="egui|eframe|cpal|wayland-client|keyring|winit"

# R1 & R2: engine and core must not depend on UI or platform crates
check_forbidden_deps "remoport-rdp" "R1" "$FORBIDDEN_UI_PLATFORM"
check_forbidden_deps "remoport-core" "R2" "$FORBIDDEN_UI_PLATFORM"

# R3: ui/ must not reference platform libraries or crate::platform; platform/ must not mention egui/eframe
if BAD=$(grep -rnE '\b(cpal|wayland|nix)\b|keyring::|crate::platform' src/ui/ 2>/dev/null); then
    echo "ERROR: Boundary rule R3 broken: src/ui/ references platform crates or adapters:" >&2
    echo "$BAD" >&2
    exit 1
fi
if BAD=$(grep -rnE '\b(egui|eframe)\b' src/platform/ 2>/dev/null); then
    echo "ERROR: Boundary rule R3 broken: src/platform/ references egui/eframe:" >&2
    echo "$BAD" >&2
    exit 1
fi

# R4: Only remoport-core/src/session/controller.rs starts remoport_rdp::Session
SESSION_STARTS=$(grep -rn 'Session::start' src/ remoport-core/src/ 2>/dev/null || true)
BAD_STARTS=$(grep -v '^remoport-core/src/session/controller\.rs:' <<< "$SESSION_STARTS" | grep -v '^$' || true)
if [ -n "$BAD_STARTS" ]; then
    echo "ERROR: Boundary rule R4 broken: Session::start called outside controller.rs:" >&2
    echo "$BAD_STARTS" >&2
    exit 1
fi

# R5: platform/ never constructs or sends engine Commands
if BAD=$(grep -rn '\bCommand\b' src/platform/ 2>/dev/null); then
    echo "ERROR: Boundary rule R5 broken: src/platform/ references Command:" >&2
    echo "$BAD" >&2
    exit 1
fi

# R6: remoport-core never mentions i18n
if BAD=$(grep -rn 'i18n' remoport-core/ 2>/dev/null); then
    echo "ERROR: Boundary rule R6 broken: remoport-core references i18n:" >&2
    echo "$BAD" >&2
    exit 1
fi

# UI pages isolation: no file in src/ui/pages/ imports or references another page module
if BAD=$(grep -rnE 'pages::(favorites|devices|apps|settings)\b|super::(favorites|devices|apps|settings)\b|crate::ui::pages::' src/ui/pages/ 2>/dev/null); then
    echo "ERROR: Page isolation broken: src/ui/pages/ references another page:" >&2
    echo "$BAD" >&2
    exit 1
fi

echo "Boundary checks passed (R1-R6 and UI page isolation verified across remoport-rdp, remoport-core, src/ui, src/platform)."
