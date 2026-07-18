#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-applicability-test.XXXXXX")
cleanup() {
  rm -rf -- "$temporary_directory"
}
trap cleanup EXIT

cat >"$temporary_directory/target-applicability.toml" <<'EOF'
format_version = 1
enabled_targets = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "riscv64-unknown-linux-gnu", "aarch64-apple-darwin"]
corpora = ["fixture"]
EOF
mkdir "$temporary_directory/fixture"
printf '#!/usr/bin/env bash\nexit 0\n' >"$temporary_directory/fixture/run.sh"
chmod +x "$temporary_directory/fixture/run.sh"
: >"$temporary_directory/fixture/probe.c"

cat >"$temporary_directory/fixture/manifest.toml" <<'EOF'
format_version = 1
execution_status = "blocked-test-capability"

[target_applicability."x86_64-unknown-linux-gnu"]
status = "applicable"
evidence_kind = "execution"
runner = "run.sh"
reason = "fixture"

[target_applicability."aarch64-unknown-linux-gnu"]
status = "applicable"
evidence_kind = "execution"
runner = "run.sh"
reason = "fixture"

[target_applicability."riscv64-unknown-linux-gnu"]
status = "applicable"
evidence_kind = "execution"
runner = "run.sh"
reason = "fixture"

[target_applicability."aarch64-apple-darwin"]
status = "applicable"
evidence_kind = "execution"
runner = "run.sh"
reason = "fixture"
EOF

set +e
blocked_output=$("$script_directory/report-target-applicability.py" \
  --root "$temporary_directory" 2>&1)
blocked_status=$?
set -e
[[ "$blocked_status" == 1 ]]
[[ "$blocked_output" == *"execution_status 'blocked-test-capability' cannot claim execution evidence"* ]]

cat >"$temporary_directory/fixture/manifest.toml" <<'EOF'
format_version = 1
execution_status = "ready"

[target_applicability."x86_64-unknown-linux-gnu"]
status = "applicable"
evidence_kind = "execution"
runner = "run.sh"
reason = "fixture"

[target_applicability."aarch64-unknown-linux-gnu"]
status = "applicable"
evidence_kind = "parse-only"
entrypoint = "probe.c"
reason = "fixture"

[target_applicability."riscv64-unknown-linux-gnu"]
status = "applicable"
evidence_kind = "parse-only"
entrypoint = "probe.c"
reason = "fixture"

[target_applicability."aarch64-apple-darwin"]
status = "applicable"
evidence_kind = "parse-only"
entrypoint = "probe.c"
reason = "fixture"
EOF

set +e
coverage_output=$("$script_directory/report-target-applicability.py" \
  --root "$temporary_directory" 2>&1)
coverage_status=$?
set -e
[[ "$coverage_status" == 1 ]]
[[ "$coverage_output" == *"enabled targets must each have execution evidence"* ]]

echo "target applicability regression tests passed"
