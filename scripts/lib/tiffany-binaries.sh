#!/usr/bin/env bash

tiffany_exe_suffix() {
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) printf '.exe\n' ;;
    *) printf '\n' ;;
  esac
}

tiffany_runtime_alias_from_bin_dir() {
  local bin_dir="$1"
  local alias_dir="$2"
  local exe_suffix
  exe_suffix="$(tiffany_exe_suffix)"

  local orchestrator_bin="$bin_dir/orchestrator$exe_suffix"
  if [[ -x "$orchestrator_bin" ]]; then
    printf '%s\n' "$orchestrator_bin"
    return 0
  fi

  local tiffany_bin="$bin_dir/tiffany-loop$exe_suffix"
  if [[ -x "$tiffany_bin" ]]; then
    mkdir -p "$alias_dir"
    local alias="$alias_dir/orchestrator$exe_suffix"
    case "$exe_suffix" in
      .exe) cp "$tiffany_bin" "$alias" ;;
      *) ln -sf "$tiffany_bin" "$alias" ;;
    esac
    printf '%s\n' "$alias"
    return 0
  fi

  echo "missing executable: $orchestrator_bin" >&2
  echo "also checked: $tiffany_bin" >&2
  echo "build first with ./scripts/tiffany-build --small" >&2
  return 1
}
