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

The adapter uses bzip2's upstream Makefile on x86-64 Linux. It builds
`libbz2.a`, `bzip2`, and `bzip2recover` with the following owned inputs:

- `CC="ccc-cc -std=gnu11"`;
- `CFLAGS="-Wall -Winline -O2 -g -D_FILE_OFFSET_BITS=64"`;
- `LDFLAGS=-no-pie`;
- resolved native `ar` and `ranlib` programs.

CCC compiles all nine C translation units selected by those targets: the seven
library inputs plus `bzip2.c` and `bzip2recover.c`. The four C files shipped as
developer utilities (`dlltest.c`, `mk251.c`, `spewG.c`, and `unzcrash.c`) are
not silently counted as part of the default product build. Both the complete
13-file archive inventory and the exact nine-file selected source set are
checked, so an added, omitted, duplicated, substituted, or native-compiled
input fails the run.

The native GCC driver receives only objects and archives for the two final
program links. A failed CCC translation is never retried with another
compiler, and a native link command containing C or preprocessed-C input is
rejected. The adapter verifies the native driver is GCC rather than Clang,
targets the GNU x86-64 Linux ABI, and records its resolved path, version,
target, complete identity output, and predefined macros.

CCC currently emits static-model objects, while contemporary Linux GCC
drivers normally produce position-independent executables. Both links
therefore receive `-no-pie`. The resulting programs must be ELF `EXEC` files
without dynamic text relocations. Exact compilation and link commands, source
inputs, ELF headers, and dynamic tags remain in the retained work directory.

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

Run [`run.sh`](run.sh) on x86-64 Linux with `CCC` set to the compiler binary.
Pass an already-downloaded archive with `--source-archive` and a Git repository
containing the pinned test commit with `--test-repository`, or let the adapter
populate its disposable cache. The work directory must be empty and is
retained with the logs and round-trip artifacts named in `manifest.toml`.

The shell-only adapter regressions are available through
[`test-run.sh`](test-run.sh).

Official references:

- [Sourceware bzip2 downloads and signing information](https://sourceware.org/bzip2/downloads.html)
- [Sourceware release archive and checksum index](https://sourceware.org/pub/bzip2/)
- [bzip2 1.0.8 manual and release date](https://sourceware.org/bzip2/manual/manual.html)
- [Sourceware bzip2 test-suite description](https://sourceware.org/bzip2/downloads.html)
- [official bzip2-tests Git repository](https://sourceware.org/git/bzip2-tests.git)
