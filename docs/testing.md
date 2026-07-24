# Test commands and prerequisites

Run every command in this document from the repository root. The standalone
oracles and corpus runners intentionally fail when a required compiler, target
runtime, debugger, archive, or opt-in environment variable is absent. A skipped
check is not successful evidence.

## Common setup

CCC requires Rust 1.96 and the toolchain pinned by `rust-toolchain.toml`. Build
the compiler used by shell harnesses before running them:

```sh
rustup toolchain install 1.96.0
cargo build --locked -p ccc-driver
export CCC="$PWD/target/debug/ccc"
export CCC_RESOURCE_DIR="$PWD/resource-dir"
```

Keep artifact directories outside the source tree. An explicitly supplied
corpus work directory must be empty:

```sh
CCC_TEST_ROOT="$(mktemp -d)"
```

The fetched-source suites need network access unless their pinned archives are
provided with the documented archive options. Their runners verify byte counts,
SHA-256, and SHA3-256 before extraction.

## Rust tests

The complete Rust unit and integration suite is:

```sh
cargo test --locked --workspace --all-targets
```

That command covers every workspace crate, the `ccc-types` layout integration
test, and these `ccc-driver` integration binaries: `abi_oracle`, `diagnostics`,
`execution`, `header_parsing`, `link_inputs`, `object_emission`,
`preprocessing`, `sysv_amd64_environment`, `sysv_amd64_interop`, and
`visibility`. To run those binaries separately:

```sh
for suite in \
  abi_oracle diagnostics execution header_parsing link_inputs object_emission \
  preprocessing sysv_amd64_environment sysv_amd64_interop visibility
do
  cargo test --locked -p ccc-driver --test "$suite"
done
cargo test --locked -p ccc-types --test layout
```

The x86-64 ABI oracle additionally requires native x86-64 GNU/Linux, GCC, and
Clang. `CCC_ABI_GCC` and `CCC_ABI_CLANG` may name target-qualified driver
commands. Native execution tests require the enabled host's assembler, linker,
runtime libraries, and object tools.

Set `CCC_CC` to the compiler driver for the Rust test profile. CI sets it
job-wide to native GCC on x86-64 and AArch64 Linux, the RISC-V cross GCC under
QEMU, and Apple Clang on Darwin. The installed-header preprocessing, parsing,
linking, and glibc assembly-label tests reject a driver whose `-dumpmachine`
architecture does not match the Rust test target. The RISC-V profile relies on
the configured binfmt/QEMU runner when those tests execute child binaries;
those executions are correctness evidence, not native timing evidence.

Run the workspace command on each supported host profile. The RISC-V CI profile
uses these exact settings:

```sh
rustup target add riscv64gc-unknown-linux-gnu
export CARGO_BUILD_TARGET=riscv64gc-unknown-linux-gnu
export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc
export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_RUNNER='qemu-riscv64 -L /usr/riscv64-linux-gnu'
export CARGO_PROFILE_DEV_OPT_LEVEL=2
export CCC_CC=riscv64-linux-gnu-gcc
export QEMU_LD_PREFIX=/usr/riscv64-linux-gnu
cargo test --locked --workspace --all-targets
```

This needs the Rust RISC-V target, `riscv64-linux-gnu-gcc`, matching binutils
and glibc under `/usr/riscv64-linux-gnu`, and QEMU user-mode execution. Native
AArch64 Linux needs an AArch64 GCC-compatible driver and binutils. Darwin arm64
needs Xcode Command Line Tools, an installed macOS SDK, and the platform
`nmedit` and `dsymutil` tools.

The object-only Mach-O debug-map oracle is intentionally runnable on any Rust
test host and needs none of those Apple tools:

```sh
cargo test --locked -p ccc-link \
  macho_tls_bridge_debug_map_retains_oso_inputs_without_native_linking
```

It packages an AArch64 Mach-O primary carrying `__debug_info` together with a
generated Darwin TLS accessor, inspects the published object's raw `N_OSO`
entry and localized helper, and checks that the referenced primary plus the
generated assembly/object share the packaging guard's lifetime. Its in-process
tool model does not validate Apple assembly or relocation encodings, native
linker/`nmedit` behavior, `.dSYM` UUIDs, `dsymutil`, or LLDB; the `darwin-arm64`
target oracle supplies that evidence.

Formatting and lint validation are separate from the test suite:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

### Inspecting code-generation statistics

Use the versioned TSV dump when comparing lowering or inlining changes:

```sh
target/debug/ccc --emit=codegen-stats -O2 path/to/input.c
```

`post_inline_ir.*` rows describe the CLIF handed to Cranelift's optimization
and machine-code pipeline. In schema version 3, `post_inline_ir.values` counts
block parameters plus instruction results reachable through the final layout;
detached data-flow-graph values are not reported. Signature, external-function,
and global-value totals count their allocated Cranelift tables. Their
`unused_*` companions count entries unreachable from live layout instructions
and Cranelift's function-level semantic roots, including transitive
global-value bases. These counters observe inlining residue; CCC does not run a
duplicate cleanup pass. The `primary_object.*` rows count disjoint logical
section sizes, symbols, and relocations in CCC's primary relocatable object.
Generated assembly bridge units are deliberately excluded from that
primary-object view; benchmark
runners which care about packaged output must inspect the final `-c` artifact
as C-Ray does. The schema starts with `schema_version` and retains a stable row
order so results can be archived and diffed without parsing human-readable
CLIF.

## Target oracles

Build CCC first, then run each target explicitly:

```sh
CCC_REQUIRE_TARGET_ORACLE=1 tests/target-oracle/run.sh x86_64-linux
CCC_REQUIRE_TARGET_ORACLE=1 tests/target-oracle/run.sh aarch64-linux
CCC_REQUIRE_TARGET_ORACLE=1 tests/target-oracle/run.sh riscv64-linux
CCC_REQUIRE_TARGET_ORACLE=1 tests/target-oracle/run.sh darwin-arm64
```

The common requirements are Bash, `tee`, `cmp`, the built CCC executable, and
the selected target's compiler driver, assembler, relocatable linker, symbol
localizer, object inspector, and runtime. `CCC_BIN` overrides the compiler and
`CCC_TARGET_ORACLE_ARTIFACTS` selects the retained artifact directory.

Target-specific prerequisites are:

| Oracle | Required environment |
| --- | --- |
| `x86_64-linux` | Native x86-64 GNU/Linux; GCC, GNU `objcopy`, `readelf`, `nm`, `objdump`, `timeout`, and GDB. Override them with `CCC_X86_64_CC`, `CCC_X86_64_OBJCOPY`, `CCC_X86_64_READELF`, `CCC_X86_64_NM`, and `CCC_X86_64_OBJDUMP`. |
| `aarch64-linux` | Linux; `aarch64-linux-gnu-gcc` and matching binutils. A native AArch64 host uses GDB. Other hosts use `qemu-aarch64`, `gdb-multiarch`, and an AArch64 runtime root containing the executable interpreter; set `CCC_QEMU_ROOT` when it is not `/usr/aarch64-linux-gnu`. |
| `riscv64-linux` | Linux; `riscv64-linux-gnu-gcc` and matching binutils. A native RISC-V64 host uses GDB. Other hosts use `qemu-riscv64`, `gdb-multiarch`, and an LP64D runtime root; set `CCC_QEMU_ROOT` when it is not `/usr/riscv64-linux-gnu`. |
| `darwin-arm64` | Native arm64 macOS; `xcrun`, Apple Clang, `otool`, `nm`, `dwarfdump`, LLDB, `file`, `shasum`, a macOS SDK, `nmedit`, and `dsymutil`. `CCC_DARWIN_SDK_ROOT`, `CCC_DARWIN_CC`, and `CCC_NMEDIT` override discovery. |

The Linux cross runners execute through QEMU rather than treating a successful
cross-link as execution evidence. They inspect the target ELF interpreter
before launch. Fixed and variadic ABI boundaries, TLS in both link directions,
unwind behavior, scalar atomics, and runtime semantics execute at both `-O0`
and `-O2`. The same loop checks VLA bound evaluation and runtime `sizeof`,
requires hosted `realloc`/`free` imports only for allocating functions, and
proves that allocation failure reaches the target trap path.
Predefined target identity, object relocations, and debugger backtraces are
checked once using the retained `-O0` artifacts.

### Extended-precision differential oracle

Berkeley SoftFloat 3e and TestFloat 3e are a separate native x86-64 Linux
oracle. Supply the exact archives recorded in
`tests/target-oracle/berkeley-testfloat.toml`:

```sh
CCC_REQUIRE_TESTFLOAT_ORACLE=1 \
CCC_SOFTFLOAT_ARCHIVE=/path/to/SoftFloat-3e.zip \
CCC_TESTFLOAT_ARCHIVE=/path/to/TestFloat-3e.zip \
tests/target-oracle/run-testfloat.sh
```

This requires GCC, Make, GNU `readelf`, `nm`, `sha256sum`, `timeout`, `unzip`,
and an OpenSSL build with SHA3-256. `CCC_TESTFLOAT_ARTIFACTS` changes the
retained output directory. The script rejects missing or hash-mismatched
archives.

## Debugger suites

Debugger validation is the final part of each target-oracle command; there is
no success mode that omits it. On native Linux it uses batch GDB. Cross-Linux
starts QEMU's gdbstub and connects with `gdb-multiarch`; `CCC_GDB_PORT_BASE`
can move the two reserved ports. Darwin uses batch LLDB and verifies the linked
`.dSYM` with `dwarfdump` and `dsymutil`. Its source probe stops after the
assignment in `debug_local.c` and requires both the retained stack local and
the SSA-promoted `observed` local to evaluate to `42`; a missing or stale
promoted location fails the oracle.

The host must allow the debugger to launch or attach to test processes. On
macOS, install Xcode Command Line Tools and ensure LLDB/debugserver authorization
is enabled for the user running the suite. A debugger timeout, an unavailable
gdbstub port, an unresolved generated helper, or a missing caller frame fails
the complete target oracle.

## Adapter regressions and applicability

The shell-only adapter tests use fake tools and local fixtures; they do not
download or build upstream projects:

```sh
./test-corpus/test-adapters.sh
```

The live Csmith runner establishes GCC/Clang consensus at `-O0` and `-O2`,
then compiles and executes every accepted case with CCC at `-O0`, `-O2`, and
`-Oz`. A case passes only when all three CCC profiles match the same reference
output.

`test-adapters.sh` runs the shared environment test, applicability regression,
the live applicability report, and the C-Ray, SQLite, Lua, bzip2, Redis, zstd,
zlib, and Csmith adapter regressions. The SQLite runner regression also invokes
its source-patch test.

To print and validate the target/corpus coverage matrix directly:

```sh
./test-corpus/report-target-applicability.py
```

The hosted GNU-like header fixture is parse-only on all enabled targets. Its
direct entry point is:

```sh
for target in \
  x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu \
  riscv64-unknown-linux-gnu aarch64-apple-darwin
do
  "$CCC" --target="$target" --dump-ast -nostdinc \
    -isystem test-corpus/libc-headers/glibc-like \
    test-corpus/libc-headers/glibc-like/probe.c >/dev/null
done
```

The Rust `header_parsing` and `preprocessing` integration binaries perform the
full deterministic assertions for this fixture.

## Fetched corpus suites

The standard x86-64 Linux corpus run uses a non-root user, native GCC and GNU
binutils, Bash, Python 3, Make, Clang, Git, Curl, OpenSSL with SHA3-256, and the
usual POSIX text/file utilities. SQLite additionally needs Tcl development
files, `patch`, and `unzip`; zlib development headers are needed by the selected
zstd build. Set `CCC` and `CCC_RESOURCE_DIR` as shown in the common setup.

The bounded commands used by CI are:

```sh
test-corpus/lua/run.sh \
  --work-dir "$CCC_TEST_ROOT/lua"

test-corpus/sqlite/run.sh --suite veryquick \
  --work-dir "$CCC_TEST_ROOT/sqlite"

test-corpus/bzip2/run.sh --target x86_64-unknown-linux-gnu \
  --work-dir "$CCC_TEST_ROOT/bzip2-x86_64"

test-corpus/zlib/run.sh \
  --work-dir "$CCC_TEST_ROOT/zlib"

test-corpus/redis/run.sh \
  --work-dir "$CCC_TEST_ROOT/redis"

test-corpus/zstd/run.sh \
  --work-dir "$CCC_TEST_ROOT/zstd"

test-corpus/c-ray/run.sh --profile correctness \
  --work-dir "$CCC_TEST_ROOT/c-ray-correctness"
```

SQLite exposes four exact upstream profiles. Use a fresh empty directory for
each:

```sh
for suite in veryquick quick all full
do
  test-corpus/sqlite/run.sh --suite "$suite" \
    --work-dir "$CCC_TEST_ROOT/sqlite-$suite"
done
```

The `all` and `full` profiles take substantially longer. SQLite, Redis, and
zstd reject UID 0 because their upstream permission checks would be
meaningless. Use a native Linux filesystem; a macOS-backed container bind mount
does not provide SQLite's required mode-`0000` behavior.

Lua accepts `--source-archive` and `--test-archive`; SQLite and Redis accept
`--archive`; zlib and zstd accept `--source-archive`. Each also accepts
`--jobs`. Supplying those archives disables the corresponding download.

### bzip2 target matrix

bzip2 is the corpus with execution adapters for all enabled targets:

```sh
test-corpus/bzip2/run.sh --target x86_64-unknown-linux-gnu \
  --work-dir "$CCC_TEST_ROOT/bzip2-x86_64"

BZIP2_QEMU_ROOT=/usr/aarch64-linux-gnu \
test-corpus/bzip2/run.sh --target aarch64-unknown-linux-gnu \
  --work-dir "$CCC_TEST_ROOT/bzip2-aarch64"

BZIP2_QEMU_ROOT=/usr/riscv64-linux-gnu \
test-corpus/bzip2/run.sh --target riscv64-unknown-linux-gnu \
  --work-dir "$CCC_TEST_ROOT/bzip2-riscv64"

BZIP2_OPENSSL=/path/to/openssl-with-sha3 \
BZIP2_MD5SUM=/path/to/gnu-compatible-md5sum \
test-corpus/bzip2/run.sh --target aarch64-apple-darwin \
  --work-dir "$CCC_TEST_ROOT/bzip2-darwin-arm64"
```

The cross-Linux commands require the matching GCC, `ar`, `ranlib`, `readelf`,
QEMU executable, and target runtime root. Darwin requires native arm64 macOS,
Xcode Command Line Tools, an SDK, and GNU-compatible checksum tools. Use
`--source-archive` and `--test-repository` for offline inputs. The complete
override names are documented in `test-corpus/bzip2/README.md`.

Hosted CI runs this complete bzip2 contract on all four enabled targets. The
x86-64 profile shares the full corpus job; the other three profiles have
dedicated target-matrix jobs and retain their object, toolchain, and execution
evidence as artifacts.

## Code-generation microbenchmarks

The local compiler-only suite measures minimal return, direct `puts`, variadic
`printf`, equivalent minimal/hosted `fputs` plus `stdout` programs, equivalent
minimal/hosted variadic `printf` programs, independent unused function/data
declaration scaling, unused block-scope declarations in a fixed live function
graph, live-function scaling, and independent live block, SSA value, global,
and string-literal scaling without link or runtime noise:

```sh
cargo build --locked --release -p ccc-driver
benchmarks/codegen/run.py \
  --ccc target/release/ccc \
  --output "$CCC_TEST_ROOT/codegen-benchmarks"
```

The default matrix runs `-O0`, `-O2`, and `-Oz`, with one warmup and five
samples. It retains every versioned codegen-stat dump, per-process wall/CPU/RSS
measurement, generated source and hash, exact command, and a comparison-ready
`summary.tsv`. Timed samples are ordinary `-c` object builds; one separate,
untimed structural-stat query runs for each case and optimization profile. The
evidence also records the compiler executable hash, effective target, resource
directory, sysroot, and selected external-tool configuration. The
unused function-, data-, and per-function declaration families require every
post-inlining CLIF metric plus primary-object byte, symbol, undefined-symbol,
relocation, and text metrics to remain identical from zero through 1,024
declarations. The per-function family adds that many unused block-scope
prototypes to each of four fixed live callees, proving frontend input growth
does not masquerade as increasing backend work. The two hosted-stdio pairs
require every post-inlining CLIF and primary-object
metric to match the equivalent minimal declarations at each profile while
retaining the extra frontend cost in their timing samples. The live-function
family supplies increasing backend work. Use a release compiler for timing
comparisons; debug builds are only suitable for exercising the harness. Use
`--cases hosted-header` or `--cases hosted-printf` to isolate one pair, and
`--declaration-scales`, `--data-declaration-scales`,
`--declarations-per-function-scales`, `--function-scales`, `--block-scales`,
`--value-scales`, `--global-scales`, `--string-scales`, `--profiles`,
`--warmups`, and `--samples` for focused investigations. Each
structural axis must grow its defining metric within checked adjacent-scale
bounds, rejecting dead fixtures and accidental quadratic IR growth before
timings are considered. See `benchmarks/codegen/README.md` for the complete
result contract.

Every enabled-target job runs both hosted-header pairs at `-O0`, `-O2`, and
`-Oz`, requires exact structural equivalence, and uploads the evidence. The
Linux x86-64 corpus job also runs the complete one-sample matrix, proving the
scaling and no-unused-function/data-declaration invariants. Timing comparisons
must use repeated runs on a controlled native host. The runner's independent
fake-tool regression is:

```sh
benchmarks/codegen/test-run.sh
```

## Defined-behavior kernel benchmarks

The executable kernel runner is separate from the compiler-only suite. Its
nine fixed-work cases cover direct calls, unsigned integers,
binary32/binary64, branch/switch control flow, indexed load/store traffic,
32-byte aggregate copies, TLS access, C11 atomic read-modify-write operations,
and a variadic definition plus caller. Every case validates its exact result.
The direct-call case records whether its leaf call remains in the
post-inlining CLIF at `-O0`, `-O2`, and `-Oz`; TLS records the expected
target-accessor calls, while the variadic case retains two functions and one
call at every profile. Run a quick native correctness check with:

```sh
benchmarks/kernels/run.py \
  --ccc target/debug/ccc \
  --output "$CCC_TEST_ROOT/kernels-correctness" \
  --mode correctness \
  --compile-warmups 0 \
  --compile-samples 1
```

Use `--mode object` for compile and structural evidence when the selected
target cannot run on the host. Cross-target correctness requires an explicit
runner; for example:

```sh
CCC_CC=riscv64-linux-gnu-gcc \
benchmarks/kernels/run.py \
  --ccc target/debug/ccc \
  --output "$CCC_TEST_ROOT/kernels-riscv64" \
  --mode correctness \
  --target riscv64-unknown-linux-gnu \
  --runner qemu-riscv64 \
  --runner-arg=-L \
  --runner-arg=/usr/riscv64-linux-gnu
```

For native timing, build a release compiler and select performance mode:

```sh
cargo build --locked --release -p ccc-driver
benchmarks/kernels/run.py \
  --ccc target/release/ccc \
  --output "$CCC_TEST_ROOT/kernels-performance" \
  --mode performance
```

Performance mode rejects emulated runners and a compiler target that does not
match the native host. Every retained executable is validated before warmups
or samples, and every execution must exit zero without output. Results keep
the compiler-side `primary_object.*` metrics distinct from the final published
object, because generated ABI or TLS bridge units may be packaged later.
`benchmarks/kernels/README.md` documents the versioned TSV/JSON schema and all
runner options.

The fast harness regression uses a fake compiler, linker, and cross-target
runner. It exercises all three modes and proves that a nonzero validation
result or nondeterministic final object fails the run:

```sh
benchmarks/kernels/test-run.sh
```

The fast CI job runs the fake-tool regression. The all-target matrix also runs
all nine kernels in object mode at `-O0`, `-O2`, and `-Oz`, retains their
structural evidence, and checks the declaration-per-function invariant plus
the four generated structural codegen-scaling axes at two bounded scales. The
scheduled Cranelift-`main` candidate runs the same gate. All-target correctness
execution and controlled native runtime baselines remain follow-up work. QEMU
results are correctness and rough-trend evidence, never native performance
evidence.

## C-Ray generated-code benchmark

C-Ray 1.1 is a native-only benchmark with correctness checks enabled for every
measurement. It supports x86-64 GNU/Linux and Apple-silicon macOS. The fast
profile renders `scene` at 320x240:

```sh
test-corpus/c-ray/run.sh --profile correctness \
  --work-dir "$CCC_TEST_ROOT/c-ray-correctness"
```

The performance profile renders `sphfract` at 800x600, with one warmup and five
measured samples per CCC optimization profile and native reference:

```sh
test-corpus/c-ray/run.sh --profile performance \
  --work-dir "$CCC_TEST_ROOT/c-ray-performance"
```

Both profiles build the unmodified source at CCC `-O0`, `-O2`, and `-Oz`,
compare exact `P6` output with a strict-FP native GCC or Apple Clang build, and
retain raw JSON/TSV timing, CPU, peak-memory, size, tool-identity, and image-hash
evidence. The summary includes post-inlining CLIF structure for CCC and parsed
section totals from every final CCC/reference object. They require Python 3,
OpenSSL with SHA3-256, Tar, GNU-compatible `size` on Linux or Xcode
`llvm-size` on macOS, libc, libm, pthreads, and Curl unless `--source-archive`
supplies the exact pinned release. Use only same-host native runs for
performance comparisons; the adapter rejects cross-target and emulated timing.
The full result schema and override options are documented in
`test-corpus/c-ray/README.md`.

The Linux x86-64 corpus job runs the correctness profile on every pull request
and push to `master`, then uploads the complete retained work directory. Those
single-sample results prove the benchmark and schema remain executable; they
are not an authoritative performance baseline.

## Csmith differential suite

Csmith differential execution is supported on native x86-64 GNU/Linux and
native Apple-silicon macOS. Both profiles require distinct GCC and Clang
installations, a C++ compiler, CMake, M4, GNU `timeout`, OpenSSL with SHA3-256,
and the common file/text utilities. Linux additionally uses GNU `objcopy`.
Darwin uses Homebrew GCC for one reference compiler, Apple Clang as the other
reference compiler and CCC's native driver, the active Xcode SDK, and
`nmedit` for Mach-O symbol localization.

On x86-64 GNU/Linux, run a bounded seed range with:

```sh
cargo build --locked -p ccc-driver
test-corpus/csmith/run.sh --cases 100 --start-seed 1 \
  --work-dir "$CCC_TEST_ROOT/csmith"
```

On Apple-silicon macOS, install the non-system dependencies first:

```sh
brew install cmake coreutils gcc m4 openssl@3
export PATH="$(brew --prefix coreutils)/libexec/gnubin:$(brew --prefix openssl@3)/bin:$PATH"
cargo build --locked -p ccc-driver
test-corpus/csmith/run.sh --cases 100 --start-seed 1 \
  --work-dir "$CCC_TEST_ROOT/csmith"
```

The Darwin runner discovers the versioned Homebrew GCC, SDK, and deployment
target automatically. Use `--gcc`, `--clang`, `--nmedit`, `--sdk-root`, or
`--deployment-target` to reproduce a run with explicitly recorded tools.

Without `--archive`, the runner downloads and builds the pinned Csmith 2.4.0
revision. For an existing developer installation, both the generator and
runtime headers are mandatory and the unverified override must be explicit:

```sh
test-corpus/csmith/run.sh --cases 100 --start-seed 1 \
  --csmith /opt/csmith/bin/csmith \
  --csmith-runtime /opt/csmith/include \
  --allow-unverified-csmith \
  --work-dir "$CCC_TEST_ROOT/csmith-installed"
```

Reproduce one retained seed with a new empty directory:

```sh
test-corpus/csmith/run.sh --cases 1 --start-seed SEED \
  --work-dir "$CCC_TEST_ROOT/csmith-SEED"
```

`test-corpus/csmith/run.sh --help` lists timeout, attempt-limit, compiler, and
tool overrides. Reference disagreement, a one-sided compiler rejection,
timeout, insufficient admissible cases, or a CCC/reference output mismatch is
an oracle failure; the runner retains the per-seed commands and artifacts.
