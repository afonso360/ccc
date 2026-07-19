#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

bash -n "$script_directory/run.sh"

grep -Fq 'CC="$CCC"' "$script_directory/run.sh"
! grep -Eq 'source-adjustment|patch[[:space:]]' "$script_directory/run.sh"
grep -Fq 'CCC links; expected $expected_link_commands' "$script_directory/run.sh"
grep -Fq 'link_commands = 2' "$script_directory/manifest.toml"
grep -Fq 'exact pinned set of C source files' "$script_directory/run.sh"
grep -Fq 'exact pinned set of object files' "$script_directory/run.sh"
grep -Fq 'exact pinned library object set' "$script_directory/run.sh"
grep -Fq 'exact two pinned upstream CCC link commands' "$script_directory/run.sh"
grep -Fq "'-o lua lua.o liblua.a -lm -Wl,-E -ldl'" "$script_directory/run.sh"
grep -Fq "'-o luac luac.o liblua.a -lm -Wl,-E -ldl'" "$script_directory/run.sh"
grep -Fq "'-std=|-DLUA_NOBUILTIN|-DLUA_USE_JUMPTABLE=0'" "$script_directory/run.sh"
! grep -Eq 'ccc-cc|test-ccc-cc' "$script_directory/run.sh" "$script_directory/manifest.toml"
[[ ! -e "$script_directory/ccc-cc" ]]
grep -Fq 'source_adjustments = "none"' "$script_directory/manifest.toml"
grep -Fq 'compiler_wrapper = "none"' "$script_directory/manifest.toml"
grep -Fq 'lua -e'"'"'_U=true'"'"' all.lua' "$script_directory/manifest.toml"

echo "Lua direct-build adapter regression passed"
