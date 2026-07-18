#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-redis-assert-test.XXXXXX")
cleanup() {
  if [[ -n "$temporary_directory" ]] && [[ -d "$temporary_directory" ]]; then
    rm -rf -- "$temporary_directory"
  fi
}
trap cleanup EXIT

host_cc=${HOST_CC:-cc}
command -v "$host_cc" >/dev/null 2>&1 || {
  echo "Redis hosted-assert test requires a host C preprocessor: $host_cc" >&2
  exit 2
}

system_include="$temporary_directory/system-include"
mkdir -p "$system_include"

cat >"$system_include/features.h" <<'EOF'
#ifndef CCC_TEST_FEATURES_H
#define CCC_TEST_FEATURES_H

#ifdef __STRICT_ANSI__
#define CCC_TEST_FEATURES_SELECTED_STRICT 1
#else
#define CCC_TEST_FEATURES_SELECTED_GNU_MODE 1
#endif

#endif
EOF

cat >"$system_include/assert.h" <<'EOF'
#ifdef CCC_TEST_SYSTEM_ASSERT_INCLUDED
#error the compatibility wrapper did not retain its assert selection
#endif
#define CCC_TEST_SYSTEM_ASSERT_INCLUDED 1

#ifndef __STRICT_ANSI__
#error the system assert header was not selected in temporary strict mode
#endif
#ifndef CCC_TEST_FEATURES_SELECTED_GNU_MODE
#error hosted features were not primed before temporary strict mode
#endif
#ifdef CCC_TEST_FEATURES_SELECTED_STRICT
#error strict feature selection leaked into the hosted feature profile
#endif

#define assert(expression) ((expression) ? (void)0 : (void)0)
#define __ASSERT_FUNCTION incompatible_host_extension
EOF

cat >"$temporary_directory/probe.c" <<'EOF'
#include <assert.h>
#include <assert.h>
EOF

"$host_cc" -std=gnu11 \
  -I"$script_directory/compat/include" -I"$system_include" \
  -dM -E "$temporary_directory/probe.c" \
  >"$temporary_directory/macros.txt"

for macro in \
  '#define CCC_TEST_FEATURES_SELECTED_GNU_MODE 1' \
  '#define CCC_TEST_SYSTEM_ASSERT_INCLUDED 1' \
  '#define CCC_REDIS_HOSTED_FEATURES_PRIMED 1' \
  '#define CCC_REDIS_PORTABLE_ASSERT 1' \
  '#define __ASSERT_FUNCTION __func__'; do
  grep -Fxq "$macro" "$temporary_directory/macros.txt"
done

if grep -Eq '^#define (__STRICT_ANSI__|CCC_TEST_FEATURES_SELECTED_STRICT)' \
  "$temporary_directory/macros.txt"; then
  echo "Redis hosted-assert test: temporary strict mode leaked" >&2
  exit 1
fi

echo "Redis hosted-assert compatibility tests passed"
