#!/usr/bin/env bash

tiffany_resolve_cargo() {
  local configured="${CARGO:-}"
  if [[ -n "$configured" ]]; then
    if command -v "$configured" >/dev/null 2>&1; then
      printf '%s\n' "$configured"
      return 0
    fi
    if [[ -x "$configured" ]]; then
      printf '%s\n' "$configured"
      return 0
    fi
    printf 'CARGO is set but not executable: %s\n' "$configured" >&2
    return 1
  fi

  if command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "cargo"
    return 0
  fi

  if [[ -n "${CARGO_HOME:-}" && -x "$CARGO_HOME/bin/cargo" ]]; then
    printf '%s\n' "$CARGO_HOME/bin/cargo"
    return 0
  fi

  if [[ -n "${HOME:-}" && -x "$HOME/.cargo/bin/cargo" ]]; then
    printf '%s\n' "$HOME/.cargo/bin/cargo"
    return 0
  fi

  cat >&2 <<'EOF'
Could not find cargo.
Install Rust with rustup, set CARGO=/path/to/cargo, or add Cargo to PATH:
  export PATH="$HOME/.cargo/bin:$PATH"
EOF
  return 1
}
