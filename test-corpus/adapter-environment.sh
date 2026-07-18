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

compiler_option_overrides_default_pie() {
  case "$1" in
    -fPIE | -fpie | -fno-PIE | -fno-pie | -fno-PIC | -fno-pic | \
      -pie | --pie | -no-pie | --no-pie | -nopie)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

linker_token_overrides_default_pie() {
  case "$1" in
    -pie | pie | --pie | -no-pie | no-pie | --no-pie | -nopie | nopie)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

# Print one driver argument with any explicit PIE-control token removed.  A
# status of one means the whole argument was a PIE control and should be
# omitted.  For a mixed -Wl bundle, retain every unrelated linker token.
filter_default_pie_driver_argument() {
  local argument=$1
  local payload token joined=
  local tokens=()
  local retained=()

  if compiler_option_overrides_default_pie "$argument"; then
    return 1
  fi

  case "$argument" in
    -Wl,* | -Wl=*)
      payload=${argument#-Wl,}
      payload=${payload#-Wl=}
      IFS=',' read -r -a tokens <<<"$payload"
      for token in "${tokens[@]}"; do
        if ! linker_token_overrides_default_pie "$token"; then
          retained+=("$token")
        fi
      done
      ((${#retained[@]})) || return 1
      for token in "${retained[@]}"; do
        if [[ -n "$joined" ]]; then
          joined+=,
        fi
        joined+=$token
      done
      printf '%s\n' "-Wl,$joined"
      ;;
    -Xlinker=*)
      payload=${argument#-Xlinker=}
      linker_token_overrides_default_pie "$payload" && return 1
      printf '%s\n' "$argument"
      ;;
    *)
      printf '%s\n' "$argument"
      ;;
  esac
}
