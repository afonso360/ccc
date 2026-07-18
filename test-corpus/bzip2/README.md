# bzip2 corpus contract

The manifest pins the official bzip2 1.0.8 release from Sourceware. The
downloaded archive is verified before extraction by byte length, the SHA-512
value published in Sourceware's release checksum file, and recorded SHA-256
and SHA3-256 values. The adapter then verifies the version and release date in
both `README` and `LICENSE`. No downloaded source or test data is committed to
this repository.

The separate official bzip2 test collection is pinned by its full Git commit
and tree identifiers. The adapter fetches that exact commit rather than a
movable branch, exports it into disposable storage, and rejects either an
unexpected commit or tree. The pinned tree contains 38 valid compressed
streams and 8 deliberately malformed streams. Its runner and each imported
subdirectory retain their upstream license information.

## Build interface

The target must be selected explicitly with `--target` or `BZIP2_TARGET`.
The adapter supports native x86-64 GNU/Linux, AArch64 GNU/Linux through
QEMU user-mode execution, RISC-V RV64GC LP64D GNU/Linux through QEMU
user-mode execution, and native Darwin arm64. It builds `libbz2.a`, `bzip2`,
and `bzip2recover` with the following owned inputs:

- `CC=ccc-cc`, relying on CCC's documented GNU C11 driver default;
- `CFLAGS="-Wall -Winline -O2 -g -D_FILE_OFFSET_BITS=64"`;
- the `ar` and `ranlib` programs belonging to the selected target toolchain.

CCC compiles all nine C translation units selected by those targets: the seven
library inputs plus `bzip2.c` and `bzip2recover.c`. The four C files shipped as
developer utilities (`dlltest.c`, `mk251.c`, `spewG.c`, and `unzcrash.c`) are
not silently counted as part of the default product build. Both the complete
13-file archive inventory and the exact nine-file selected source set are
checked, so an added, omitted, duplicated, substituted, or native-compiled
input fails the run.

The selected target driver receives only objects and archives for the two
final program links. A failed CCC translation is never retried with another
compiler, and a link command containing C or preprocessed-C input is rejected.
GNU/Linux profiles require a matching GCC driver and record its target,
version, compiler sysroot, complete identity output, and predefined macros.
The Darwin profile requires native Apple Clang, a macOS SDK, and a deployment
target; those inputs are recorded and applied consistently to translation and
linking.

The driver links with the platform default relocation policy. No profile adds
a language-standard, PIE, architecture, or ABI selection flag, and the source
archive is not patched. GNU/Linux outputs must be target-machine ELF `DYN`
executables with the PIE dynamic flag, the expected ABI flags, an absolute
interpreter, and no dynamic text relocations. Darwin outputs must be Mach-O
arm64 executables with the PIE flag, `LC_BUILD_VERSION`, the selected minimum
OS version, the expected public entry symbol, no writable-and-executable load
segment, and no retained relocation entries. Exact commands, source inputs,
headers, dynamic metadata, load commands, relocations, and symbols remain in
the retained work directory.

The AArch64 and RISC-V profiles require an explicit, non-host QEMU runtime
root. The adapter reads each executable's interpreter from its ELF program
headers and refuses to run unless that interpreter exists beneath the selected
root. After auditing the real target executables it installs the tracked
`qemu-launcher` at the upstream executable names. This preserves bzip2's
unmodified `make check` command and the official test runner's convention of
deriving `bzip2recover` from the `bzip2` pathname.

Ambient Make injection and build flags are cleared before invoking the
upstream Makefile. `BZIP` and `BZIP2` are also removed because the built
program consumes those variables as implicit command-line options. The
adapter pins the C locale and UTC timezone for stable test output.

## Selected compiler surface

bzip2 describes the product sources as standard ANSI C, but its GNU compiler
predicate is consequential. Under CCC's shipped GNU 4.2.1 compatibility
identity, the selected source surface uses:

- the `noreturn` GNU attribute in the command-line program;
- the `__inline__` keyword in the library internals;
- `unsigned long long` counters in `bzip2recover`.

The checked predicate probe records these selections before any product
translation. The selected build contains no compiler builtins, computed
gotos, inline assembly, `__int128`, variable-length array objects, or statement
expressions.

## Execution validation

Three complementary checks run against the CCC-built programs:

1. Upstream `make check` performs the six release-vector checks described by
   the source README: three known-stream decompressions and three deterministic
   recompressions compared byte-for-byte with the shipped references.
2. A deterministic smoke fixture concatenates pinned release files, compresses
   it at level 9, performs an integrity check, decompresses it, and compares the
   result byte-for-byte with the input.
3. The official bzip2-tests runner exercises all 46 pinned streams in normal
   and small-memory modes, verifies the valid outputs with their MD5 reference
   values, requires corruption detection for the malformed inputs, and runs
   `bzip2recover` across both sets.

The extended runner is explicitly passed `--without-valgrind`; its optional
host-dependent Valgrind discovery is outside this compiler execution check.
The pinned collection is about five megabytes when exported, and its bounded
runner materially extends coverage of decoder edge cases and recovery without
turning the adapter into an open-ended stress test. A zero exit status, exactly
440 `PASS` records, and the terminal `All tests passed` marker are required.

Set `CCC` to the compiler binary and invoke [`run.sh`](run.sh) with one of the
four exact triples recorded in `manifest.toml`. `CCC_LINK_CC`, `BZIP2_AR`,
`BZIP2_RANLIB`, and `BZIP2_READELF` can override the target-profile defaults.
Cross-Linux execution additionally requires `BZIP2_QEMU_ROOT` and may override
`BZIP2_QEMU`; `BZIP2_SYSROOT` controls the compiler sysroot independently from
the runtime root. Darwin may override `BZIP2_SDKROOT` and
`BZIP2_DEPLOYMENT_TARGET`.

`BZIP2_OPENSSL` must name an OpenSSL implementation with SHA3-256. Darwin's
system LibreSSL is not sufficient. `BZIP2_MD5SUM` must name a GNU-compatible
MD5 utility. Darwin CI uses the tracked OpenSSL-backed `md5sum-darwin` adapter,
including the checked-stdin mode required by the official suite. Both hash
tools are probed before any download or build. Pass an already-downloaded
archive with `--source-archive` and a Git repository containing the pinned test commit with
`--test-repository`, or let the adapter populate its disposable cache. The
work directory must be empty and is retained with the artifacts named in
`manifest.toml`.

The shell-only adapter regressions are available through
[`test-run.sh`](test-run.sh).

Official references:

- [Sourceware bzip2 downloads and signing information](https://sourceware.org/bzip2/downloads.html)
- [Sourceware release archive and checksum index](https://sourceware.org/pub/bzip2/)
- [bzip2 1.0.8 manual and release date](https://sourceware.org/bzip2/manual/manual.html)
- [Sourceware bzip2 test-suite description](https://sourceware.org/bzip2/downloads.html)
- [official bzip2-tests Git repository](https://sourceware.org/git/bzip2-tests.git)
