# zstd corpus contract

The manifest pins the official zstd 1.5.7 release archive. The archive is
fetched into disposable storage and verified by byte length, the upstream
SHA-256 sidecar, and a recorded SHA3-256 before extraction. The adapter also
checks the version macros and both upstream license alternatives. Downloaded
source is not committed to this repository.

## Build boundary

The adapter uses the upstream GNU Make interface on x86-64 Linux. Every C
translation that produces the `zstd` command-line program or its `datagen` test
helper goes through CCC using its GNU C11 driver default. The wrapper filters
upstream C99, GNU99, and GNU C11 overrides instead of injecting a replacement
standard flag. The native GCC driver receives only
objects for final links; it never receives C, preprocessed C, assembly,
or response-file inputs and is never used as a retry path. The adapter derives
the selected upstream source multiset from the release Makefile, appends the
four inputs used by `datagen`, and compares it with every recorded CCC input.
Duplicate source use is significant and remains in the audit.

The programs Makefile also generates and translates the same bounded pthread
capability probe during each of its two recursive evaluations. GNU Make expands
the immediate probe assignment even when `HAVE_PTHREAD=1` is pinned on the
command line. Both exact source bytes are SHA-256 checked by the compiler
adapter before CCC sees them, and both source occurrences and source-free probe
links are part of the expected command multiset. The temporary probe programs
are not part of the retained product binaries.

The selected configuration enables pthread support and legacy decoding while
disabling optional zlib, liblzma, and liblz4 format wrappers. It leaves zstd's
`MEM_FORCE_MEMORY_ACCESS` and embedded xxHash's `XXH_FORCE_MEMORY_ACCESS`
unset. Their exact upstream headers default both controls to `1` under CCC's
advertised GNU identity. Zstd consequently uses its file-scope
`aligned(1)` scalar typedefs for 16-, 32-, 64-bit, and word-sized unaligned
loads and stores; xxHash uses its block-scope `aligned(1)` 32- and 64-bit
scalar typedefs. The capability probe preprocesses the pinned headers and
requires those defaults, while build-command auditing rejects any forced
definition or undefinition of either control. This exercises the upstream
alignment-bearing types without changing those headers or injecting a
compatibility flag.

The configuration makes the tested feature set independent of development
libraries installed on the host. Ambient GNU Make injection, build flags,
platform overrides, test binary overrides, and helper command overrides are
cleared. The debug level, C locale, UTC timezone, and creation mask are pinned.
The resolved native link driver must identify as GCC for an x86-64 Linux GNU
target; its identity and predefined macros are retained.

CCC emits position-independent objects, and the native GCC links use their
platform defaults without an adapter-supplied relocation option. The adapter
requires the resulting `zstd` and `datagen` files to be PIE executables with
ELF type `DYN` and the PIE dynamic flag, and rejects dynamic text relocations.

## Portable no-assembly configuration

Upstream's `ZSTD_NO_ASM=1` setting removes the stand-alone amd64 assembly
translation unit and defines `ZSTD_DISABLE_ASM`, but zstd 1.5.7 still selects a
small set of GNU inline-assembly performance paths from the compiler identity.
CCC intentionally advertises GNU 4.2.1 compatibility, which also selects those
paths. The complete selected count-bit group is implemented directly through
`__builtin_clz`, `__builtin_clzll`, `__builtin_ctz`, and `__builtin_ctzll`, so
the upstream builtin path remains unchanged.

The checked source adjustment extends `ZSTD_DISABLE_ASM` guards only to those
performance-only inline-assembly paths. The existing CPU-feature fallback,
conditional expression, and unaligned loops remain the implementation. The
upstream prefetch path uses CCC's validated nonfaulting builtin. The adjustment
does not undefine `__GNUC__`, remove a source file, alter the public API, or
change the compressed format. Its patch, complete
target preimage and postimage hashes, exact application log, and rationale are
retained with each run. Any release drift or non-exact hunk match fails before
compilation.

The capability probe requires CCC's own GNU 4.2.1 and LP64 x86-64 identity,
the exact relevant builtin-registry profile, the native count-bit and upstream
`aligned(1)` scalar unaligned-access paths, and disabled assembly. CCC advertises
`__builtin_bswap64` and a behavior-compatible no-op `__builtin_prefetch`;
zstd's version predicates exclude the former and use the latter. Build-command
auditing requires the no-assembly decision on every C translation.

Zstd's dependency header selects `__builtin_memcpy`, `__builtin_memmove`, and
`__builtin_memset` from the legacy GNU version tuple. CCC implements all three
with their libc-compatible signatures and target libcalls, so the unmodified
header is used without a forced dependency override.

Debian glibc selects a GNU statement-expression implementation of the standard
`assert` macro from the same identity. CCC implements that expression form and
the selected `__PRETTY_FUNCTION__` identifier directly. Assertions therefore
remain enabled through the unmodified system header without a compatibility
include directory or forced header.

## Bounded upstream tests

The reproducible profile runs upstream `make check`. Zstd's root Makefile calls
this the basic test for the command-line program, and the project README calls
it the quick smoke test for a local build. The target builds `zstd` and
`datagen`, then runs `tests/playTests.sh` with its long-data cases disabled. It
covers command-line compression and decompression, streaming, dictionaries,
file handling, corruption rejection, permissions, sparse files, and the
selected threaded implementation without invoking the time-bounded fuzzers or
the broader long-duration target.

After the upstream target succeeds, the adapter performs two deterministic
round trips over a generated text payload: one through named files and one
through standard streams. It also runs the CLI integrity check on the compressed
file and compares both decoded outputs byte for byte. Inputs, compressed data,
outputs, commands, suite output, exact source and link audits, and ELF metadata
remain in the work directory.

Run [`run.sh`](run.sh) as a non-root user on x86-64 Linux with `CCC` set to the
compiler binary. Pass an already-downloaded archive with `--source-archive`, or
let the adapter populate its disposable cache. The selected work directory must
be empty and is retained after the run.

Official references:

- [Zstandard v1.5.7 release](https://github.com/facebook/zstd/releases/tag/v1.5.7)
- [Official zstd 1.5.7 SHA-256 sidecar](https://github.com/facebook/zstd/releases/download/v1.5.7/zstd-1.5.7.tar.gz.sha256)
- [Upstream build and quick-test instructions](https://github.com/facebook/zstd/blob/v1.5.7/README.md)
- [Upstream test categories](https://github.com/facebook/zstd/blob/v1.5.7/TESTING.md)
