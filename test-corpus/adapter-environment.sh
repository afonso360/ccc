#!/usr/bin/env bash

record_native_gcc_driver() {
  local label=$1
  local driver=$2
  local identity_artifact=$3
  local macro_artifact=$4
  local target version version_output version_lower

  target=$(LC_ALL=C "$driver" -dumpmachine) || {
    echo "$label native driver did not report its target" >&2
    return 1
  }
  version=$(LC_ALL=C "$driver" -dumpfullversion -dumpversion) || {
    echo "$label native driver did not report its version" >&2
    return 1
  }
  version_output=$(LC_ALL=C "$driver" --version) || {
    echo "$label native driver did not report its identity" >&2
    return 1
  }
  LC_ALL=C "$driver" -dM -E -x c /dev/null >"$macro_artifact" || {
    echo "$label native driver did not report its predefined macros" >&2
    return 1
  }
  version_lower=$(printf '%s\n' "$version_output" |
    tr '[:upper:]' '[:lower:]')

  if [[ "$version_lower" == *clang* ]] ||
    grep -Eq '^#define __clang__' "$macro_artifact"; then
    echo "$label native driver must be GCC rather than Clang" >&2
    return 1
  fi
  if [[ "$version_lower" != *gcc* &&
    "$version_lower" != *"gnu compiler collection"* ]]; then
    echo "$label native driver is not GCC" >&2
    return 1
  fi
  if [[ "$target" != x86_64*-linux-gnu* ]]; then
    echo "$label native GCC target is $target rather than x86-64 Linux GNU" >&2
    return 1
  fi
  if ! grep -Eq '^#define __GNUC__[[:space:]]+[0-9]+$' "$macro_artifact"; then
    echo "$label native driver does not expose GCC identity macros" >&2
    return 1
  fi

  {
    printf 'executable=%s\n' "$driver"
    printf 'target=%s\n' "$target"
    printf 'version=%s\n' "$version"
    printf '%s\n' '--version:' "$version_output"
  } >"$identity_artifact"
}

clear_ambient_make_injection() {
  unset GNUMAKEFLAGS MAKEFILES MAKEFLAGS MAKEOVERRIDES MFLAGS
}
