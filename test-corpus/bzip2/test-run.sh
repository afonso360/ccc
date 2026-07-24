#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

help_output=$("$script_directory/run.sh" --help 2>&1)
[[ "$help_output" == *"--source-archive PATH"* ]]
[[ "$help_output" == *"--test-repository PATH"* ]]
[[ "$help_output" == *"--work-dir PATH"* ]]
[[ "$help_output" == *"--jobs COUNT"* ]]
[[ "$help_output" == *"--target TRIPLE"* ]]

set +e
invalid_jobs_output=$("$script_directory/run.sh" --jobs 0 2>&1)
invalid_jobs_status=$?
missing_value_output=$("$script_directory/run.sh" --source-archive 2>&1)
missing_value_status=$?
unknown_option_output=$("$script_directory/run.sh" --unknown 2>&1)
unknown_option_status=$?
missing_target_output=$("$script_directory/run.sh" --source-archive /does/not/exist 2>&1)
missing_target_status=$?
unsupported_target_output=$("$script_directory/run.sh" --target unsupported 2>&1)
unsupported_target_status=$?
set -e

[[ "$invalid_jobs_status" == 2 ]]
[[ "$invalid_jobs_output" == *"must be a positive integer"* ]]
[[ "$missing_value_status" == 2 ]]
[[ "$missing_value_output" == *"usage:"* ]]
[[ "$unknown_option_status" == 2 ]]
[[ "$unknown_option_output" == *"usage:"* ]]
[[ "$missing_target_status" == 2 ]]
[[ "$missing_target_output" == *"usage:"* ]]
[[ "$unsupported_target_status" == 1 ]]
[[ "$unsupported_target_output" == *"unsupported bzip2 target"* ]]

temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-bzip2-runner-test.XXXXXX")
cleanup() {
  rm -rf -- "$temporary_directory"
}
trap cleanup EXIT
mkdir "$temporary_directory/resource-dir" "$temporary_directory/qemu-root"

cat >"$temporary_directory/fake-qemu" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" >"$QEMU_TRACE"
EOF
chmod +x "$temporary_directory/fake-qemu"
mkdir "$temporary_directory/programs"
cp "$script_directory/qemu-launcher" "$temporary_directory/programs/bzip2"
chmod +x "$temporary_directory/programs/bzip2"
: >"$temporary_directory/programs/bzip2.target"
chmod +x "$temporary_directory/programs/bzip2.target"
cp "$script_directory/qemu-launcher" "$temporary_directory/programs/bzip2recover"
chmod +x "$temporary_directory/programs/bzip2recover"
: >"$temporary_directory/programs/bzip2recover.target"
chmod +x "$temporary_directory/programs/bzip2recover.target"
{
  printf 'CCC_BZIP2_QEMU=%q\n' "$temporary_directory/fake-qemu"
  printf 'CCC_BZIP2_QEMU_ROOT=%q\n' "$temporary_directory/qemu-root"
} >"$temporary_directory/programs/.ccc-qemu-config"
export QEMU_TRACE="$temporary_directory/qemu-trace"
"$temporary_directory/programs/bzip2" -9 input
[[ "$(sed -n '1p' "$QEMU_TRACE")" == -L ]]
[[ "$(sed -n '2p' "$QEMU_TRACE")" == "$temporary_directory/qemu-root" ]]
[[ "$(sed -n '3p' "$QEMU_TRACE")" == -0 ]]
[[ "$(sed -n '4p' "$QEMU_TRACE")" == "$temporary_directory/programs/bzip2" ]]
[[ "$(sed -n '5p' "$QEMU_TRACE")" == "$temporary_directory/programs/bzip2.target" ]]
[[ "$(sed -n '6p' "$QEMU_TRACE")" == -9 ]]
[[ "$(sed -n '7p' "$QEMU_TRACE")" == input ]]

"$temporary_directory/programs/bzip2recover" compressed-file
[[ "$(sed -n '4p' "$QEMU_TRACE")" == "$temporary_directory/programs/bzip2recover" ]]
[[ "$(sed -n '5p' "$QEMU_TRACE")" == "$temporary_directory/programs/bzip2recover.target" ]]
[[ "$(sed -n '6p' "$QEMU_TRACE")" == compressed-file ]]

host_os=$(uname -s)
host_arch=$(uname -m)
if [[ "$host_os" == Linux && "$host_arch" == x86_64 ]]; then
  missing_driver_work="$temporary_directory/missing-driver-work"
  set +e
  missing_driver_output=$(CCC=/bin/true \
    CCC_RESOURCE_DIR="$temporary_directory/resource-dir" \
    CCC_LINK_CC="$temporary_directory/absent-driver" \
    "$script_directory/run.sh" --target x86_64-unknown-linux-gnu \
      --work-dir "$missing_driver_work" 2>&1)
  missing_driver_status=$?
  missing_root_output=$(BZIP2_QEMU_ROOT='' \
    "$script_directory/run.sh" --target aarch64-unknown-linux-gnu \
      --work-dir "$temporary_directory/missing-root-work" 2>&1)
  missing_root_status=$?
  set -e
  [[ "$missing_driver_status" == 1 ]]
  [[ "$missing_driver_output" == *"executable is not available"* ]]
  [[ ! -e "$missing_driver_work" ]]
  [[ "$missing_root_status" != 0 ]]
  [[ "$missing_root_output" == *"BZIP2_QEMU_ROOT"* ]]
  [[ ! -e "$temporary_directory/missing-root-work" ]]
elif [[ "$host_os" == Darwin && ("$host_arch" == arm64 || "$host_arch" == aarch64) ]]; then
  missing_sdk_work="$temporary_directory/missing-sdk-work"
  set +e
  missing_sdk_output=$(CCC=/usr/bin/true \
    CCC_RESOURCE_DIR="$temporary_directory/resource-dir" \
    BZIP2_SDKROOT="$temporary_directory/absent-sdk" \
    "$script_directory/run.sh" --target aarch64-apple-darwin \
      --work-dir "$missing_sdk_work" 2>&1)
  missing_sdk_status=$?
  set -e
  [[ "$missing_sdk_status" == 1 ]]
  [[ "$missing_sdk_output" == *"directory does not exist"* ]]
  [[ ! -e "$missing_sdk_work" ]]
fi

openssl_tool=${BZIP2_OPENSSL:-openssl}
if command -v "$openssl_tool" >/dev/null 2>&1; then
  checksum_input="$temporary_directory/checksum-input"
  checksum_file="$temporary_directory/checksums.md5"
  checksum_stdin_file="$temporary_directory/checksums-stdin.md5"
  printf 'ccc checksum adapter test\n' >"$checksum_input"

  BZIP2_OPENSSL="$openssl_tool" \
    "$script_directory/md5sum-darwin" "$checksum_input" >"$checksum_file"
  BZIP2_OPENSSL="$openssl_tool" \
    "$script_directory/md5sum-darwin" --check "$checksum_file" >/dev/null

  checksum=$(
    BZIP2_OPENSSL="$openssl_tool" \
      "$script_directory/md5sum-darwin" "$checksum_input" | awk '{ print $1 }'
  )
  printf '%s  -\n' "$checksum" >"$checksum_stdin_file"
  BZIP2_OPENSSL="$openssl_tool" \
    "$script_directory/md5sum-darwin" -c "$checksum_stdin_file" \
    <"$checksum_input" >/dev/null

  printf 'corrupt\n' >>"$checksum_input"
  if BZIP2_OPENSSL="$openssl_tool" \
    "$script_directory/md5sum-darwin" -c "$checksum_file" >/dev/null 2>&1; then
    echo "md5sum-darwin accepted a corrupted input" >&2
    exit 1
  fi
fi

"$script_directory/test-ccc-cc.sh"

echo "bzip2 runner tests passed"
