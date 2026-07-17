# SQLite corpus contract

The manifest pins the canonical SQLite 3.47.2 source archive used for the Tcl
`veryquick` suite. The smaller amalgamation distribution is not interchangeable:
it does not contain the complete test suite.

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

The adapter pins `ac_cv_func_isnan=no`. A link-only Autoconf probe finds the
host `isnan` symbol, but glibc exposes the source interface as a type-generic
macro whose expansion references `__isnanl` even when its operand is `double`.
That source interface therefore requires an ABI CCC does not advertise. The
cache decision selects SQLite's own binary64 bit test, which is the intended
fallback when the host interface is unusable; it does not patch SQLite source.

The adapter builds `testfixture` and runs the Makefile-owned serial target that
invokes `test/veryquick.test`:

```text
make -j1 tcltest
```

Run [`run.sh`](run.sh) on x86-64 Linux with `CCC` set to the compiler binary.
`ccc-cc` is a transparent compiler-driver adapter: every C input is compiled by
CCC, while the configured native toolchain performs only the final link. It
never retries a failed translation with another C compiler. Pass a previously
downloaded archive with `--archive`, or let the adapter populate its disposable
cache after verifying the pinned hashes.

Run the adapter as a non-root user on a native Linux filesystem. SQLite's Tcl
suite deliberately checks that a mode-`0000` file is unreadable; it rejects UID
0, and Docker Desktop bind mounts backed by macOS do not provide the required
permission semantics. For containerized validation, use a Linux Docker volume
owned by the non-root container user for the complete work directory.

The checked-in expected inventory is evaluated under CCC's effective compiler
identity, never under the host GCC or Clang identity. It selects the full
barrier builtin `__sync_synchronize` and no inline assembly. The latter remains
true only while `VDBE_PROFILE`, `SQLITE_PERFORMANCE_TRACE`, and
`SQLITE_ENABLE_STMT_SCANSTATUS` are absent.

`effective-macros.txt` is CCC's complete predefined-macro dump for the run.
`predicate-probe.txt` records the version and feature predicates that select
SQLite's compiler-specific paths. The adapter rejects a run when those
decisions differ from the pinned inventory.

## Updating the pin

Changing the version requires a fresh archive hash, source ID, generated-source
hash, license review, macro/predicate snapshot, compile-command capture, and
capability inventory. Versions beginning with 3.48.0 use Autosetup for the
canonical-source configuration interface, so an update beyond this pin is also
a build-adapter migration rather than an ordinary source refresh. The
precompiled amalgamation followed that migration in 3.49.0.

Failure artifacts retain the configuration log, effective identity, exact
commands, inventory, and complete test output named in the manifest.

Release references:

- [SQLite 3.47.2](https://www.sqlite.org/releaselog/3_47_2.html)
- [SQLite 3.48.0](https://www.sqlite.org/releaselog/3_48_0.html)
- [SQLite 3.49.0](https://www.sqlite.org/releaselog/3_49_0.html)
