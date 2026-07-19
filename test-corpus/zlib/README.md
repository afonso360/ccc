# zlib corpus contract

The manifest pins the official zlib 1.3.2 release archive from the permanent
zlib fossil directory. The adapter verifies its byte length, the SHA-256 value
published by zlib.net, a recorded SHA3-256 value, the embedded version, and the
upstream zlib license before extraction. Downloaded source remains in
disposable storage and is never committed or substituted with a host copy.

## Unmodified build interface

The execution gate invokes the upstream interface directly:

```sh
CC=ccc ./configure
make test test64
```

`CCC_RESOURCE_DIR` selects the shipped compiler headers and `CCC_CC` selects
the audited native GCC toolchain used internally for system-header discovery,
assembly, and final platform linking. Neither variable changes the commands
issued by zlib's configure script or Makefile. There is no compiler wrapper,
source patch, generated-Makefile rewrite, retry with another compiler, or
native compilation fallback.

The configure probes must recognize CCC's documented GCC-compatible identity,
retain CCC as `CC`, enable `-fPIC`, accept hidden visibility, and keep shared
library support enabled. The upstream build compiles all 15 core sources twice,
once for `libz.a` and once for the shared object. Its ordinary and large-file
test programs account for four additional C translations. The adapter compares
the resulting 34-entry source multiset against an exact expected multiset.

The link surface deliberately exercises ordered object and archive inputs,
`-L`, shared output, a version script, SONAME selection, and the compiler's
default PIE policy. The gate inspects the shared object and six executables,
requires PIE dynamic flags on every executable, checks the shared SONAME, and
rejects dynamic text relocations.

## Execution profile

Upstream `make test` runs the static and shared example profiles, and the
separate `test64` target runs the large-file profile. The adapter invokes both
targets and requires all three upstream success markers.
It then performs an independent deterministic `minigzip` round trip and
compares the decompressed payload byte for byte.

The default release uses checked-in CRC and inflate tables on x86-64, so the
build does not select its optional runtime table initialization path. Atomic
semantics remain covered by focused compiler tests rather than being inferred
from this corpus pass. The x86-64 source selects no inline assembly.

Run [`run.sh`](run.sh) on x86-64 Linux with `CCC` set to the compiler binary.
Pass an already-downloaded archive with `--source-archive`, or allow the adapter
to populate its disposable cache. The work directory is retained with the
configure log, complete compiler commands, source inventory, ELF inspection,
and test artifacts named in `manifest.toml`.

Official references:

- [zlib home page and published release hash](https://zlib.net/)
- [permanent release archive directory](https://zlib.net/fossils/)
- [upstream source repository](https://github.com/madler/zlib)
