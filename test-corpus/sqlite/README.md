# SQLite corpus contract

The manifest pins the canonical SQLite 3.47.2 source archive used for the
supported Tcl and fuzz-test suites. The smaller amalgamation distribution is
not interchangeable: it does not contain the complete test suite. The default
is `veryquick`, matching the regular corpus gate.

The adapter fetches the archive into a disposable cache and verifies, before
configuration:

- archive byte length, SHA-256, and SHA3-256;
- the extracted `VERSION` file;
- the complete `manifest.uuid`; and
- the release `SQLITE_SOURCE_ID` recorded in `manifest.toml`.

After SQLite generates `sqlite3.c`, the adapter verifies its published
SHA3-256 as a second provenance boundary. No downloaded source is committed to
the repository.

## Build interface

Configuration is out of tree and uses the classic Autoconf-generated
`configure` plus Makefile interface. Host build tools are compiled with GCC
through SQLite's `BCC` Make variable; target objects and `testfixture` are
compiled with CCC through `CC`, then linked by the configured native driver.
Optional readline, shared-library, and load-extension paths are disabled
explicitly so installed host packages cannot silently alter the selected C
surface. Tcl and zlib discovery must nevertheless succeed and is recorded in
`config.log`.

The configured native driver must identify as GCC, target a GNU x86-64 Linux
ABI, and omit Clang identity macros. Its resolved path, target, version,
complete `--version` output, and predefined macros are retained with the run.
Ambient GNU Make injection through `MAKEFILES` or `GNUMAKEFLAGS` is cleared
with the other build and configure overrides before configuration.

CCC emits static-model objects, while current Linux GCC drivers commonly link
position-independent executables by default. Configuration therefore pins
`LDFLAGS=-no-pie`. The adapter requires the resulting `testfixture` to be an
ELF `EXEC` file and rejects dynamic text relocations, preventing an apparently
successful host link from weakening the executable contract.

Every CCC translation receives an explicit leading `-std=gnu11`, matching the
manifest even if CCC's driver default changes. The wrapper evaluates all
`-std=` arguments in command order and records the last, effective choice plus
the scoped hardware-timing predicate state for each translation in
`language-modes.txt`.

Configuration detects `isnan` without a cache override. The compiler's hosted
`math.h` wrapper delegates declarations and constants to libc, then supplies a
single-evaluation binary64 classification macro that does not expose an
unselected `long double` branch. The adapter requires `HAVE_ISNAN` in the
generated configuration, proving that the normal configure probe selected the
usable source interface.

## Verified test-source adjustment

The pinned release has an order-dependent test-harness defect. When
`ext/recover/recovercorrupt4.test` runs immediately before
`ext/recover/recoverfault.test`, it leaves a 3072-byte truncated database at
`testdir/test.db2`. The later test deliberately reuses that path for recovery
output, so its final no-fault iteration reports four failures at the end of the
persistent and transient OOM loops. The exact two-file sequence reproduces
under both CCC and GCC; either file by itself passes. The failure also survives
separate `testfixture` processes sharing the same test directory, proving that
the state is filesystem-local rather than compiler- or process-global.

The adapter applies
[`adjustments/recoverfault-clean-output.patch`](adjustments/recoverfault-clean-output.patch)
only after verifying the archive hashes, `VERSION`, `manifest.uuid`, and the
canonical generated `sqlite3.c` hash and source ID. Applying it earlier would
make SQLite's own source-ID generator label the tree as an alternate source.
The patch adds one `forcedelete test.db2` before `recoverfault.test` begins,
restoring the initial state that the test expects. Deleting `test.db2` alone
fixes the sequence; deleting only `test.db` or `test.db3` does not. Every
upstream test still runs, including all fault-injection iterations.

The manifest pins the patch hash plus the target's exact preimage and postimage
hashes. Application uses a zero-fuzz dry run, rejects offset hunks, and fails if
any hash or context drifts. `source-adjustment.patch`,
`source-adjustment-apply.log`, and `source-adjustment.txt` retain the applied
patch, command result, hashes, target, and rationale with the run artifacts.
The cleanup matters to `quick`, `all`, and `full`, which include both files;
`veryquick` excludes fault-simulation files.

Select a checked upstream suite with `--suite`:

| Mode        | Pinned upstream entrypoint                      | Coverage                                                                                                                                     |
| ----------- | ----------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `veryquick` | `make -j1 tcltest` → `test/veryquick.test`      | The regular per-change subset, excluding malloc and I/O fault simulation.                                                                    |
| `quick`     | `testfixture test/quick.test`                   | The quick Tcl suite, including the fault-simulation tests omitted by `veryquick`.                                                            |
| `all`       | `make -j1 alltest` → `test/all.test`            | The full Tcl permutation matrix selected by the upstream Makefile.                                                                           |
| `full`      | `make -j1 fulltest` → `alltest` plus `fuzztest` | The `all` matrix followed by SQLite's Makefile-owned fuzz targets; its amalgamation selects the no-assembly timing fallback described below. |

The `all` and `full` Makefile targets also build SQLite's command-line shell.
With glibc and CCC's pinned GNU compatibility identity, the shell's `NAN` and
`INFINITY` constants select `__builtin_nanf("")` and `__builtin_inff()`.
The capability probe records both hosted builtins before the suite starts;
the focused compiler regressions verify their exact binary32 constants. The
NaN contract accepts only an empty ordinary or `u8` string literal. Runtime,
wide, and nonempty payloads are rejected because CCC does not claim GNU payload
encoding.

SQLite 3.47.2's target named `quicktest` actually invokes
`test/extraquick.test`, a smaller subset than `veryquick`. The adapter therefore
builds `testfixture` and invokes the pinned `test/quick.test` entrypoint directly
for `--suite quick`, passing the Makefile's pinned output options so
`test-out.txt` is retained consistently. The other modes use the upstream
Makefile targets shown above. Suite execution remains serial; `--jobs` controls
only generation of the amalgamation and its host build tools.

For example, the larger Tcl matrix is selected with:

```text
CCC=/path/to/ccc ./run.sh \
  --archive /path/to/sqlite-src-3470200.zip \
  --work-dir /path/to/empty-linux-work-directory \
  --jobs 2 \
  --suite all
```

Run [`run.sh`](run.sh) on x86-64 Linux with `CCC` set to the compiler binary.
`ccc-cc` is a transparent compiler-driver adapter: every C input is compiled by
CCC, while the configured native toolchain performs only the final link. It
never retries a failed translation with another C compiler. Pass a previously
downloaded archive with `--archive`, or let the adapter populate its disposable
cache after verifying the pinned hashes.

Before executing the selected suite, the adapter builds `testfixture` once and
compares the recorded CCC inputs with the configured Makefile's
`TESTFIXTURE_SRC` expansion. The pinned profile contains 91 distinct C
translation units: archive sources plus the verified generated `sqlite3.c`.
The wrapper rejects sources outside those provenance roots, and the audit
requires one CCC command per source followed by exactly one source-free native
link. `-no-pie`, `-Wl,*`, and library arguments are retained only for that
native link. Command and source logging each append a complete record in one
operation, so concurrent Make jobs cannot splice partial records together.
The sorted expected and observed source sets are retained with the build
artifacts. Suite-specific targets may subsequently compile additional archive
sources or generated C files such as `sqlite3_analyzer.c`; those inputs remain
restricted to the verified archive and disposable build roots and continue to
the complete command and source logs. They do not change the frozen
`testfixture` source-set snapshot.

After the selected suite returns, the adapter rechecks the canonical
`sqlite3.c` hash and rejects any native link record that contains C or
preprocessed-C input. For `full`, it additionally requires both `fuzzcheck` and
`sessionfuzz` and retains separate ELF header and dynamic-tag evidence proving
that each is a non-PIE `EXEC` file without dynamic text relocations.

Run the adapter as a non-root user on a native Linux filesystem. SQLite's Tcl
suite deliberately checks that a mode-`0000` file is unreadable; it rejects UID
0, and Docker Desktop bind mounts backed by macOS do not provide the required
permission semantics. For containerized validation, use a Linux Docker volume
owned by the non-root container user for the complete work directory.

The checked-in expected inventory is evaluated under CCC's effective compiler
identity, never under the host GCC or Clang identity. It selects the full
barrier builtin `__sync_synchronize`, exposes the hosted binary32 constants
required by the command-line shell, and selects no inline assembly.
`testfixture` and the Tcl suites keep `VDBE_PROFILE`,
`SQLITE_PERFORMANCE_TRACE`, and `SQLITE_ENABLE_STMT_SCANSTATUS` absent.

The upstream fuzzcheck profile deliberately enables
`SQLITE_ENABLE_STMT_SCANSTATUS`. In GNU mode on x86-64, that feature includes
`src/hwtime.h` from the generated `sqlite3.c` amalgamation and selects its
`rdtsc` inline assembly. When the wrapper sees that exact generated input in a
`SQLITE_OSS_FUZZ` command, it appends `-D__STRICT_ANSI__=1` to that one CCC
translation while retaining GNU C11 mode. That exact upstream predicate selects
the existing no-assembly timing implementation, which returns zero; SQLite
documents the implementation as disabling only obscure profiling and analysis
timing. In SQLite 3.47.2 the predicate's only other source-level effect is to
suppress the `SQLITE_INLINE` optimization hint. The wrapper does not alter
source, remove a fuzz input, or change `FUZZCHECK_OPT`: statement scan status
and the rest of SQLite's fuzz feature profile remain enabled. The eight
fuzzcheck support translation units, `alltest`, and `sessionfuzz` receive no
predicate override, and every translation remains in GNU C11 mode.

The compiler wrapper audits the final effective `-std` option and
`__STRICT_ANSI__` command-line state on every command. It requires the predicate
override exactly for the generated fuzzcheck amalgamation and rejects it
elsewhere. Thus an ambient or reordered flag cannot silently re-enable the
inline-assembly branch or widen the override surface.

`effective-macros.txt` is CCC's complete predefined-macro dump for the run.
`predicate-probe.txt` records the version and feature predicates that select
SQLite's compiler-specific paths. The adapter rejects a run when those
decisions differ from the pinned inventory. `suite-plan.txt` records the checked
mode, upstream target or script, component targets, and command for the run.

## Updating the pin

Changing the version requires a fresh archive hash, source ID, generated-source
hash, license review, macro/predicate snapshot, compile-command capture, and
capability inventory. Versions beginning with 3.48.0 use Autosetup for the
canonical-source configuration interface, so an update beyond this pin is also
a build-adapter migration rather than an ordinary source refresh. The
precompiled amalgamation followed that migration in 3.49.0.

Failure artifacts retain separate configuration and target command logs, the
expected and observed target source sets, effective identity, inventory, and
complete test output named in the manifest. The per-translation language-mode
log makes the full-suite capability adjustment independently auditable.

Release references:

- [SQLite 3.47.2](https://www.sqlite.org/releaselog/3_47_2.html)
- [SQLite 3.48.0](https://www.sqlite.org/releaselog/3_48_0.html)
- [SQLite 3.49.0](https://www.sqlite.org/releaselog/3_49_0.html)
