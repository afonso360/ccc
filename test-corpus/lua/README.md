# Lua corpus contract

The manifest pins the official Lua 5.5.0 release and its matching test archive.
Both archives are fetched into disposable storage and
verified by byte length, the SHA-256 values published by Lua.org, and recorded
SHA3-256 values before extraction. The adapter also verifies the source version,
the test-suite version, and the upstream MIT license text. The test files refer
to the copyright notice in the matching source archive's `lua.h`; both archives
are therefore verified as one versioned provenance set. No downloaded source is
committed to the repository.

## Build interface

The adapter uses Lua's upstream `make linux` interface on x86-64 Linux with
`CC=ccc`. Replacing Lua's bundled `gcc -std=gnu99` command with CCC also
replaces that command's language option, so the build uses CCC's documented
GNU C default without an adapter-supplied `-std` flag. All 34 C translation
units that produce `liblua.a`, `lua`, and `luac` are compiled by CCC, and CCC
drives both final links through its resolved target toolchain. The adapter never
retries a failed translation or link with another compiler. A source-input log is
checked against every `.c` file in the pinned `src/` directory, so a duplicate,
omitted, substituted, or native-compiled translation unit fails the run rather
than being hidden by a command count. The corresponding 34 object outputs and
the exact 32-member `liblua.a` inventory are checked independently. The two
normalized upstream link commands must consume only `lua.o` or `luac.o`,
`liblua.a`, and the upstream Linux libraries in their original order.

CCC emits position-independent objects, and Lua's link steps use CCC's normal
PIE executable default without an adapter-supplied relocation flag.
After linking, the adapter requires both programs to be ELF `DYN` files with
the `DF_1_PIE` dynamic flag and rejects `TEXTREL` dynamic tags. The retained
ELF headers, dynamic tags, and exact link commands make this boundary
auditable.
The adapter rejects a native driver that does not identify as GCC, targets a
non-GNU x86-64 Linux ABI, or exposes Clang identity macros. Its resolved path,
target, version, complete `--version` output, and predefined macros are retained
with the run.

Lua describes its implementation as ISO C, but the Linux build selects a
meaningful GNU surface through CCC's predefined compiler identity. The checked
inventory requires:

- `__builtin_expect` branch hints;
- the `__builtin_huge_val` expression selected by the hosted `math.h` contract;
- binary64 `DBL_MANT_DIG` and `DBL_MAX_10_EXP` facts supplied through the
  target predefined-macro profile;
- `noreturn` and internal-visibility attributes;
- the `__extension__` operator used for data-to-function-pointer conversion;
- computed-goto dispatch in `luaV_execute`.

The run fails before the build if CCC's advertised GNU 4.2.1 identity would
select a different surface. The Linux profile also selects `_setjmp` and
`_longjmp` for protected calls. Ambient GNU Make injection through `MAKEFILES`
or `GNUMAKEFLAGS` is cleared with the other build flags. Exact compiler and
linker commands are extracted from the retained upstream build log. The run
rejects any link command containing a C source input, requires exactly the two
upstream program links, and verifies that every C translation retains the
upstream optimization and `LUA_USE_LINUX` selections without adding a language
override or disabling compiler-selected builtins or jump tables.

## Official test profiles

Lua.org publishes three ways to use the test archive:

1. The basic profile runs `lua -e'_U=true' all.lua`. Upstream describes this as
   portable and suitable for users; it skips the resource-heavy,
   platform-specific, and internal-instrumentation cases.
2. The complete profile first builds the C libraries under `libs/`, then runs
   `all.lua` without `_U`. Upstream warns that this mode is intentionally
   nonportable, resource-intensive, and may require local adjustments.
3. The internal profile copies `ltests.c` and `ltests.h` into the source tree,
   rebuilds Lua with `LUA_USER_H` selecting the test instrumentation, and runs
   the suite with internal consistency checks enabled.

The reproducible gate uses the official basic command unchanged and requires
both a successful process status and its terminal `final OK !!!` marker. Before
that invocation, the adapter removes ambient `LUA_INIT`, module-path, and
version-specific variants so host configuration cannot inject code or replace
the pinned suite. The complete and internal profiles are not silently
approximated: the former requires position-independent shared test modules, and
the latter adds the instrumented runtime, GNU assertion statement expressions,
and further nonlocal-control coverage. Their sources remain covered by the
separately pinned official test archive.

Run [`run.sh`](run.sh) on x86-64 Linux with `CCC` set to the compiler binary.
Set `CCC_CC` when its resolved target GCC driver is not available as `gcc`.
Pass already-downloaded archives with `--source-archive` and `--test-archive`,
or let the adapter populate its disposable cache. The work directory must be
empty and is retained with the logs named in `manifest.toml`.

Official references:

- [Lua download and published source hash](https://www.lua.org/download.html)
- [Lua 5.5 build instructions and license](https://www.lua.org/manual/5.5/readme.html)
- [Lua test-suite downloads and profiles](https://www.lua.org/tests/)
