#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-sqlite-runner-test.XXXXXX")
cleanup() {
  if [[ -n "$temporary_directory" ]] && [[ -d "$temporary_directory" ]]; then
    rm -rf -- "$temporary_directory"
  fi
}
trap cleanup EXIT

# The runner and this test share the checked suite-to-entrypoint mapping.
source "$script_directory/suite-plan.sh"

assert_suite() {
  local name=$1
  local expected_driver=$2
  local expected_target=$3
  local expected_entrypoint=$4
  local expected_components=$5
  local expected_command=$6

  select_sqlite_suite "$name"
  [[ "$suite_driver" == "$expected_driver" ]]
  [[ "$suite_make_target" == "$expected_target" ]]
  [[ "$suite_tcl_entrypoint" == "$expected_entrypoint" ]]
  [[ "$suite_components" == "$expected_components" ]]
  [[ "$suite_command" == "$expected_command" ]]
}

assert_suite \
  veryquick make-target tcltest test/veryquick.test tcltest \
  'make -j1 tcltest'
assert_suite \
  quick test-script testfixture test/quick.test quick \
  'make -j1 testfixture && ./testfixture "$TOP/test/quick.test" --verbose=file --output=test-out.txt'
assert_suite \
  all make-target alltest test/all.test alltest \
  'make -j1 alltest'
assert_suite \
  full make-target fulltest test/all.test alltest,fuzztest \
  'make -j1 fulltest'

execution_directory="$temporary_directory/execution"
fake_bin="$temporary_directory/bin"
source_directory="$temporary_directory/source"
trace="$temporary_directory/execution-trace"
mkdir -p "$execution_directory" "$fake_bin" "$source_directory/test"

cat >"$fake_bin/make" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'make' >>"$TRACE"
printf ' <%s>' "$@" >>"$TRACE"
printf '\n' >>"$TRACE"
last=
for argument in "$@"; do
  last=$argument
done
case "$last" in
  tcltest | alltest | fulltest)
    : >test-out.txt
    ;;
esac
EOF
cat >"$execution_directory/testfixture" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'testfixture' >>"$TRACE"
printf ' <%s>' "$@" >>"$TRACE"
printf '\n' >>"$TRACE"
for argument in "$@"; do
  case "$argument" in
    --output=*)
      : >"${argument#--output=}"
      ;;
  esac
done
EOF
chmod +x "$fake_bin/make" "$execution_directory/testfixture"

assert_execution() {
  local name=$1
  local expected_make_target=$2
  local expected_fixture_count=$3
  local expected_entrypoint=${4:-}

  : >"$trace"
  rm -f "$execution_directory/test-out.txt"
  select_sqlite_suite "$name"
  (
    cd "$execution_directory"
    export PATH="$fake_bin:$PATH"
    export TRACE="$trace"
    run_sqlite_suite "$source_directory" /usr/bin/gcc /adapter/ccc-cc
  )

  [[ "$(grep -c '^make ' "$trace")" == 1 ]]
  grep -Fq " <$expected_make_target>" "$trace"
  grep -Fq ' <BCC=/usr/bin/gcc -g>' "$trace"
  grep -Fq ' <CC=/adapter/ccc-cc>' "$trace"
  [[ "$(grep -c '^testfixture' "$trace" || true)" == "$expected_fixture_count" ]]
  [[ -f "$execution_directory/test-out.txt" ]]
  if [[ -n "$expected_entrypoint" ]]; then
    grep -Fq "testfixture <$source_directory/$expected_entrypoint>" "$trace"
    grep -Fq ' <--verbose=file> <--output=test-out.txt>' "$trace"
  fi
}

assert_execution veryquick tcltest 0
assert_execution quick testfixture 1 test/quick.test
assert_execution all alltest 0
assert_execution full fulltest 0

set +e
invalid_output=$("$script_directory/run.sh" \
  --suite unsupported \
  --work-dir "$temporary_directory/unused" 2>&1)
invalid_status=$?
missing_output=$("$script_directory/run.sh" --suite 2>&1)
missing_status=$?
set -e

if [[ "$invalid_status" != 2 ]]; then
  echo "invalid suite exited with $invalid_status instead of 2:" >&2
  echo "$invalid_output" >&2
  exit 1
fi
if [[ "$invalid_output" == *"can only return"* ]]; then
  echo "invalid suite attempted a top-level return:" >&2
  echo "$invalid_output" >&2
  exit 1
fi
[[ "$invalid_output" == *"unsupported SQLite test suite: unsupported"* ]]
[[ "$invalid_output" == *"--suite veryquick|quick|all|full"* ]]
[[ ! -e "$temporary_directory/unused" ]]
[[ "$missing_status" == 2 ]]
[[ "$missing_output" == *"--suite veryquick|quick|all|full"* ]]

help_output=$("$script_directory/run.sh" --help 2>&1)
[[ "$help_output" == *"--suite veryquick|quick|all|full"* ]]

"$script_directory/test-source-adjustment.sh"

echo "SQLite runner tests passed"
