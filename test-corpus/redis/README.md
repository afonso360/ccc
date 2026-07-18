# Redis corpus contract

The manifest pins the official Redis 8.8.0 core-source archive. The adapter
verifies its byte length, the published SHA-256 digest, and a separately
recorded SHA3-256 digest before extraction. It then verifies the version macro
and all three licensing options in `LICENSE.txt`. Downloaded source is kept in
disposable storage and is not committed to this repository.

The core archive does not contain Redis's separately distributed module
bundle. This profile builds only `redis-server` and `redis-cli`; it does not
silently fetch modules or optional dependencies.

## Build interface

The adapter uses Redis's upstream component Makefiles on x86-64 Linux. It
builds the exact static dependencies needed by the two selected programs, then
invokes the core `redis-server` and `redis-cli` targets. CCC translates an
audited set of 178 C inputs:

- 120 Redis core and command-line inputs;
- 7 hiredis inputs;
- 35 bundled Lua and Redis Lua-extension inputs;
- 12 TRE inputs;
- one input each from linenoise, HDR Histogram, fpconv, and xxHash.

Both the expected list and the observed absolute source paths are sorted and
compared byte-for-byte. A missing, added, duplicated, substituted, or
native-compiled C input fails the run. Lua executables, dependency tests,
shared libraries, developer utilities, vector-set sources, and module-bundle
sources are outside the selected set.

The compiler wrapper sends every C input to CCC. Native GCC receives only
objects and static archives for exactly two final links; a failed CCC
translation is never retried with another compiler. The adapter records the
resolved GCC path, version, target, complete identity output, and predefined
macros. CCC currently emits static-model objects, so both final links use
`-no-pie`; the resulting programs must be ELF `EXEC` files without dynamic
text relocations.

The profile selects the system allocator and disables TLS, systemd, link-time
optimization, vector sets, and the external module bundle. Redis's C11 atomic
probe is pinned unavailable, consistently with CCC's shipped
`__STDC_NO_ATOMICS__` definition. Under the advertised GNU 4.2.1 identity,
Redis therefore selects its legacy `__sync_*` implementation rather than
pulling C11 or `__atomic_*` support into the build.

Ambient Make variables and package-manager flags are cleared before any
upstream target runs. The locale, timezone, and source-date epoch are fixed,
and all required dependency targets receive the same language, assertion, and
optimization profile.

## Hosted assertions

Assertions remain enabled through the target's unmodified hosted header.
CCC implements glibc's selected GNU statement-expression macro and its
`__PRETTY_FUNCTION__` diagnostic identifier directly, so Redis needs no
compatibility include directory or forced assertion header. The retained
capability probe verifies that normal GNU header mode and the system assertion
path remain selected.

## Selected compiler surface

The selected source path uses the following optional integer and atomic
builtins:

- one-argument `__builtin_bswap64`, `__builtin_clz`, `__builtin_clzl`,
  `__builtin_clzll`, `__builtin_ctzll`, `__builtin_popcount`, and
  `__builtin_popcountll`;
- three-argument `__builtin_prefetch(pointer, 0, 3)`, always the read/high
  locality form;
- `__sync_add_and_fetch`, `__sync_fetch_and_add`, `__sync_sub_and_fetch`,
  `__sync_bool_compare_and_swap`, `__sync_val_compare_and_swap`,
  `__sync_lock_test_and_set`, and `__sync_synchronize`.

Redis uses both the ordinary and optional protected-list forms of the legacy
sync builtins: sub-and-fetch appears with two or three arguments, and boolean
compare-and-swap with three or four. The optional sentinel is
`__sync_synchronize`. The effective profile does not select `__atomic_*`,
`__builtin_bswap32`, the other count-trailing-zero widths, `__builtin_popcountl`,
or CPU-dispatch builtins.

The compiler wrapper mirrors every real translation with a preprocessing-only
pass under the same language, macro, include, warning, and optimization
arguments. Dependency-generation options are omitted from the mirror so it
cannot alter the upstream build's dependency files. The runner requires one
nonempty capture for every pinned source, compares their relative paths to the
178-entry source inventory, and records counts for every expanded
`__builtin_*` and `__sync_*` token. It requires all selected builtins to occur,
checks pinned exact counts for the integer and prefetch group, and rejects the
unselected atomic, byte-swap, bit-count, CPU-dispatch, overflow, alignment,
return-address, and unreachable forms explicitly. The bulky preprocessed
copies are then removed; their exact input list and compact count artifact are
retained. The same mirror rejects any selected `asm`, `__asm`, or `__asm__`
form and retains a zero-form inventory.

The build also exercises packed hiredis layout, flexible array members,
compound literals, GNU attributes, and thread-local storage. TRE's configured
sources do not define `TRE_USE_ALLOCA`, so this profile contains no
variable-length array objects or dynamic stack allocation. The optional x86
assembly selected by upstream HDR Histogram is handled by the bounded source
adjustment described below; no assembly input or inline-assembly form reaches
CCC.

## Bounded source adjustment

One checked patch adjusts two files in the extracted disposable tree. The
patch and its preimage/postimage hash list are themselves pinned by SHA-256.
Application requires GNU patch, zero fuzz, and no offset; every resulting file
must match its recorded postimage, and a second application must fail.

The two adjustments are deliberately narrow:

- HDR Histogram's six x86 atomic assembly statements become the selected
  legacy sync builtins. CCC's contract gives these operations sequentially
  consistent ordering, which is at least as strong as the required load,
  store, exchange, add, and compare/exchange behavior.
- xxHash's compiler guard normally selects an empty GNU inline-assembly
  statement from the advertised compatibility tuple. CCC does not implement
  that source form or perform the vectorization the guard inhibits, so the
  patch selects a behavior-compatible standard-C no-op under `__CCC__`. Its
  compile-time marker lets the mirrored source prove the exact selected
  expansion count without emitting data or code. All hashing code and target
  selection remain unchanged.

The runner records exact semantic occurrence counts after patching. In
particular, it requires the upstream statement-expression CAS call to remain,
all six HDR assembly statements to be absent, the xxHash header to retain its nine
ordinary and three Clang-NEON guard call sites, exactly one ordinary guard to
expand to the CCC no-op, no guard identifier to remain unexpanded, and exactly
seven upstream math-classification calls to remain: one in hiredis, five in
cjson, and one in cmsgpack. CCC's hosted `math.h` wrapper supplies
single-evaluation binary64-compatible definitions without changing these
sources or adding a corpus-specific include path. The Solaris-only fallback
macro definition left in cjson is neither selected nor counted as a call.

## Execution validation

This is a focused compiler execution profile, not Redis's full upstream Tcl
test suite. After the build and audit complete, the adapter:

1. checks both program version reports;
2. starts the CCC-built server on a private Unix-domain socket with persistence
   disabled;
3. exercises `PING`, string set/get, integer increments, list ordering, hash
   set/get, embedded-Lua `EVAL`, and database cardinality through the
   CCC-built CLI;
4. requests `SHUTDOWN NOSAVE` and requires a clean server exit.

The private socket avoids opening a TCP port, and the work directory retains
the server log, smoke transcript, compile/link commands, source audit,
capability inventory, source-adjustment proof, native-driver identity, and ELF
metadata.

Run [`run.sh`](run.sh) as a non-root user on x86-64 Linux with `CCC` set to the
compiler executable and `CCC_RESOURCE_DIR` set to the shipped headers. Pass an
already downloaded archive with `--archive`, or allow the adapter to populate
its disposable cache. An explicitly supplied work directory must be empty and
is retained for inspection.

The shell-only adapter regressions are available through
[`test-run.sh`](test-run.sh).

Official references:

- [Redis 8.8.0 core-source archive](https://download.redis.io/releases/redis-8.8.0.tar.gz)
- [Redis release hash index](https://github.com/redis/redis-hashes/blob/master/README)
- [Redis source repository](https://github.com/redis/redis/tree/8.8.0)
