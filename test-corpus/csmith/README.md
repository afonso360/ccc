# Csmith differential tests

This opt-in suite generates deterministic C programs, establishes an output
consensus with GCC and Clang at `-O0` and `-O2`, and compares CCC with that
consensus. It is intentionally separate from the ordinary Cargo test run.

The default profile uses an exact Csmith 2.4.0 source commit with consecutive
seeds and a bounded program shape. Floating-point generation, compiler
builtins, packed structures, unions, and bit-fields are disabled; integer
types, pointers, arrays, structures, qualifiers, calls, and control flow remain
enabled. The checked-in [profile](profile.sh) is the authoritative argument
list and generates the expected manifest; the runner refuses to start if the
two files drift.

Csmith can still emit a C constraint violation, especially around pointer
types. Each candidate must therefore pass both GCC and Clang with
`-std=c11 -pedantic-errors -fsyntax-only` before it is eligible for output
comparison. Jointly rejected seeds are recorded as `inadmissible` and replaced
until the requested number of eligible differential cases completes or the
attempt limit is reached. A one-sided rejection is an oracle failure.

## Run locally

The execution oracle supports either an x86-64 Linux GNU host or a native
Apple-silicon macOS host. Both profiles require distinct native GCC and Clang
installations so the reference consensus does not collapse to two names for
the same compiler.

On x86-64 Linux:

```sh
cargo build --locked -p ccc-driver
test-corpus/csmith/run.sh --cases 100 --start-seed 1
```

On Apple-silicon macOS, install GNU GCC, GNU `timeout`, CMake, and OpenSSL 3.
The runner finds Homebrew's versioned GCC executable automatically, selects
the active macOS SDK with `xcrun`, uses Apple Clang for CCC's native driver,
and uses `nmedit` for Mach-O symbol localization:

```sh
brew install cmake coreutils gcc m4 openssl@3
export PATH="$(brew --prefix coreutils)/libexec/gnubin:$(brew --prefix openssl@3)/bin:$PATH"
cargo build --locked -p ccc-driver
test-corpus/csmith/run.sh --cases 100 --start-seed 1
```

The Darwin profile applies the selected SDK and minimum deployment version to
GNU GCC, Apple Clang, CCC, and the final native link. Override its defaults
with `--gcc`, `--clang`, `--nmedit`, `--sdk-root`, or
`--deployment-target` when reproducing a run with recorded tools.

The runner downloads the source archive named in
[the manifest](manifest.toml), verifies its byte count, SHA-256, SHA3-256,
version, and license, and builds it in the retained work directory. The archive
is cached under `${XDG_CACHE_HOME:-$HOME/.cache}/ccc/corpus/csmith`; source,
build, generated programs, and compiler outputs never enter the repository.
Use `--archive PATH` to run without network access.

For harness development, an existing installation can be selected explicitly.
Both its executable and runtime headers are required, the executable must
report version 2.4.0, and the unverified override must be acknowledged because
the version string alone does not identify the pinned commit:

```sh
test-corpus/csmith/run.sh \
  --csmith /opt/csmith/bin/csmith \
  --csmith-runtime /opt/csmith/include \
  --allow-unverified-csmith
```

Run `test-corpus/csmith/run.sh --help` for the complete command-line option
list. Defaults can also be set with `CSMITH_CASES`, `CSMITH_START_SEED`,
`CSMITH_MAX_ATTEMPTS`, `CSMITH_BUILD_JOBS`, `CSMITH_GENERATOR_TIMEOUT`,
`CSMITH_COMPILE_TIMEOUT`, `CSMITH_RUN_TIMEOUT`, `CSMITH_WORK_DIR`,
`CSMITH_ARCHIVE`, `CSMITH`, `CSMITH_RUNTIME`, `CSMITH_GCC`, `CSMITH_CLANG`,
`CSMITH_OBJCOPY`, `CSMITH_NMEDIT`, `CSMITH_SDKROOT`,
`CSMITH_DEPLOYMENT_TARGET`, `CSMITH_CXX`, `CCC`, and `CCC_RESOURCE_DIR`;
command-line values take precedence. The **Csmith differential tests**
workflow exposes the eligible case count and first attempted seed through a
manual Linux dispatch; it does not run for pushes or pull requests.

## Results and reproduction

The runner prints its artifact directory before provisioning or compiling. A
default invocation creates a retained directory under `${TMPDIR:-/tmp}`; an
explicit `--work-dir` must name an empty directory. `summary.tsv` contains one
row for every attempted seed, and `run-summary.txt` distinguishes attempted,
eligible, completed, inadmissible, and inconclusive counts. Outcomes include:

- `pass`: all reference outputs agree and CCC matches them;
- `inadmissible`: both strict C11 validators reject the source;
- `inconclusive-timeout`: all four reference executables time out;
- `generator-failure` or `generator-provenance-failure`;
- `reference-syntax-failure`, `reference-syntax-disagreement`,
  `reference-compile-failure`,
  `reference-execution-failure`, `reference-invalid-output`, or
  `reference-disagreement`;
- `ccc-compile-failure`, `ccc-link-failure`, `ccc-execution-failure`, or
  `output-mismatch`;
- `harness-failure` when a case worker terminates without recording a result.

Every case retains `program.c`, shell-escaped commands, compile and execution
statuses, stdout, stderr, and a concise result. `run-config.txt` records the
seed range, generator arguments, timeouts, target assumptions, and reference
matrix. `tool-identities/` records generator/runtime hashes, the exact CCC
binary and resource tree, a dirty-source patch, compiler/object-copier
identities, targets, hashes, and predefined macros. The runner binds
CCC's native header discovery to the same validated GCC used by the reference
matrix and rejects non-LP64 or mismatched target configurations before case
generation.

To reproduce one finding with the same checked-in profile, pass its seed as a
one-case range and use the same tools recorded in the original artifact:

```sh
test-corpus/csmith/run.sh --cases 1 --start-seed SEED --work-dir EMPTY_PATH
```

The suite treats reference disagreement as a failed oracle, not as evidence
against CCC. A timeout shared by all references is inconclusive and causes the
suite to try the next seed; partial timeouts are failed oracles. CCC emits one
object, which the recorded GCC driver links with `-lm` under its default
relocation policy. Neither the reference executables nor CCC's linked
executable receive a non-PIE override, so CCC compile and link failures remain
distinct and the suite exercises the platform's default PIE configuration.
Sanitizers may be useful while investigating a retained source, but their
silence is not treated as proof that a program is defined.

## Harness test

The runner control flow has a host-independent test that supplies fake Csmith,
compiler, target, and timeout commands:

```sh
test-corpus/csmith/test-run.sh
```

This test covers deterministic seed ranges, replacement of inadmissible and
timed-out seeds, partial and abnormal reference failures, a CCC output
mismatch, a generator failure followed by later cases, runtime/ABI guards,
environment cleanup, path handling, and invalid arguments.
