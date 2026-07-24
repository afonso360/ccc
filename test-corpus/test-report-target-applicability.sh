#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-applicability-test.XXXXXX")
cleanup() {
  rm -rf -- "$temporary_directory"
}
trap cleanup EXIT

enabled_targets=()
while IFS= read -r target; do
  [[ -n "$target" ]] && enabled_targets+=("$target")
done < <("$script_directory/report-target-applicability.py" --list-enabled-targets)
((${#enabled_targets[@]} > 1))

{
  printf 'format_version = 1\n'
  printf 'enabled_targets = ['
  separator=
  for target in "${enabled_targets[@]}"; do
    printf '%s"%s"' "$separator" "$target"
    separator=', '
  done
  printf ']\ncorpora = ["fixture"]\n'
} >"$temporary_directory/target-applicability.toml"
mkdir "$temporary_directory/fixture"
printf '#!/usr/bin/env bash\nexit 0\n' >"$temporary_directory/fixture/run.sh"
chmod +x "$temporary_directory/fixture/run.sh"
: >"$temporary_directory/fixture/probe.c"

write_manifest() {
  local execution_status=$1
  local evidence_mode=$2
  local index target
  {
    printf 'format_version = 1\n'
    printf 'execution_status = "%s"\n' "$execution_status"
    for ((index = 0; index < ${#enabled_targets[@]}; index++)); do
      target=${enabled_targets[index]}
      printf '\n[target_applicability."%s"]\n' "$target"
      printf 'status = "applicable"\n'
      if [[ "$evidence_mode" == all-execution || "$index" == 0 ]]; then
        printf 'evidence_kind = "execution"\n'
        printf 'runner = "run.sh"\n'
      else
        printf 'evidence_kind = "parse-only"\n'
        printf 'entrypoint = "probe.c"\n'
      fi
      printf 'reason = "fixture"\n'
    done
  } >"$temporary_directory/fixture/manifest.toml"
}

write_manifest blocked-test-capability all-execution

set +e
blocked_output=$("$script_directory/report-target-applicability.py" \
  --root "$temporary_directory" 2>&1)
blocked_status=$?
set -e
[[ "$blocked_status" == 1 ]]
[[ "$blocked_output" == *"execution_status 'blocked-test-capability' cannot claim execution evidence"* ]]

write_manifest ready first-execution

set +e
coverage_output=$("$script_directory/report-target-applicability.py" \
  --root "$temporary_directory" 2>&1)
coverage_status=$?
set -e
[[ "$coverage_status" == 1 ]]
[[ "$coverage_output" == *"enabled targets must each have execution evidence"* ]]

echo "target applicability regression tests passed"
