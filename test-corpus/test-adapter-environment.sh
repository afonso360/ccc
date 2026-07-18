#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-adapter-environment-test.XXXXXX")
cleanup() {
  if [[ -n "$temporary_directory" ]] && [[ -d "$temporary_directory" ]]; then
    rm -rf -- "$temporary_directory"
  fi
}
trap cleanup EXIT

source "$script_directory/adapter-environment.sh"

cat >"$temporary_directory/fake-gcc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  -dumpmachine)
    echo x86_64-linux-gnu
    ;;
  '-dumpfullversion -dumpversion')
    echo 12.2.0
    ;;
  --version)
    echo 'gcc (Fake GCC) 12.2.0'
    ;;
  '-dM -E -x c /dev/null')
    echo '#define __GNUC__ 12'
    echo '#define __GNUC_MINOR__ 2'
    ;;
  *)
    exit 2
    ;;
esac
EOF

cat >"$temporary_directory/fake-clang" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  -dumpmachine)
    echo x86_64-linux-gnu
    ;;
  '-dumpfullversion -dumpversion')
    echo 18.1.0
    ;;
  --version)
    echo 'clang version 18.1.0'
    ;;
  '-dM -E -x c /dev/null')
    echo '#define __GNUC__ 4'
    echo '#define __clang__ 1'
    ;;
  *)
    exit 2
    ;;
esac
EOF
chmod +x "$temporary_directory/fake-gcc" "$temporary_directory/fake-clang"

identity_artifact="$temporary_directory/gcc-identity.txt"
macro_artifact="$temporary_directory/gcc-macros.txt"
record_native_gcc_driver \
  Test "$temporary_directory/fake-gcc" "$identity_artifact" "$macro_artifact"
grep -Fxq "executable=$temporary_directory/fake-gcc" "$identity_artifact"
grep -Fxq 'target=x86_64-linux-gnu' "$identity_artifact"
grep -Fxq 'version=12.2.0' "$identity_artifact"
grep -Fxq '#define __GNUC__ 12' "$macro_artifact"

for label in Lua SQLite; do
  set +e
  clang_output=$(record_native_gcc_driver \
    "$label" "$temporary_directory/fake-clang" \
    "$temporary_directory/$label-identity.txt" \
    "$temporary_directory/$label-macros.txt" 2>&1)
  clang_status=$?
  set -e
  [[ "$clang_status" == 1 ]]
  [[ "$clang_output" == "$label native driver must be GCC rather than Clang" ]]
  [[ ! -e "$temporary_directory/$label-identity.txt" ]]
done

export GNUMAKEFLAGS=ambient
export MAKEFILES=ambient
export MAKEFLAGS=ambient
export MAKEOVERRIDES=ambient
export MFLAGS=ambient
clear_ambient_make_injection
for variable in GNUMAKEFLAGS MAKEFILES MAKEFLAGS MAKEOVERRIDES MFLAGS; do
  if declare -p "$variable" >/dev/null 2>&1; then
    echo "ambient Make variable remains defined: $variable" >&2
    exit 1
  fi
done

echo "corpus adapter environment tests passed"
