# Test corpus metadata

Third-party corpus pins, hashes, provenance, and fetch metadata live here.
Corpus source is fetched into disposable storage and is never silently replaced
by a host-installed copy.

| Corpus                                            | Contract                                                                                                                 |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| [Hosted GNU-like headers](libc-headers/README.md) | Original project fixture for deterministic preprocessing and parsing                                                     |
| [SQLite 3.47.2](sqlite/README.md)                 | Fetched canonical source, audited configure/Makefile adapter, and explicit `veryquick`, `quick`, `all`, and `full` modes |
| [Lua 5.5.0](lua/README.md)                        | Fetched official source and tests, unmodified `CC=ccc` Linux build, and official basic profile                           |
| [Redis 8.8.0](redis/README.md)                    | Fetched official core source, audited x87/inline-assembly build, and focused server/CLI smoke profile                    |
| [bzip2 1.0.8](bzip2/README.md)                    | Fetched official source and test repository, upstream checks, deterministic round trip, and 440-check extended profile  |
| [zstd 1.5.7](zstd/README.md)                      | Fetched official source, audited portable build, bounded upstream checks, and deterministic file/stream round trips      |
| [zlib 1.3.2](zlib/README.md)                      | Fetched official source, unmodified configure/Make build, static/shared tests, and deterministic minigzip round trip     |
| [Csmith 2.4.0](csmith/README.md)                  | Pinned generated programs, strict C11 admission, GCC/Clang consensus, reproducible seeds, and retained failure artifacts  |
| [C-Ray 1.1](c-ray/README.md)                      | Pinned unmodified ray tracer, strict-FP native reference, exact PPM oracle, and raw compile/link/render measurements       |

[`test-adapters.sh`](test-adapters.sh) runs every shell-only adapter regression,
including the shared native-GCC identity boundary and removal of ambient GNU
Make injection. The fetched builds and execution profiles remain explicit
Linux jobs invoked through each corpus's `run.sh`.

## Target applicability

[`target-applicability.toml`](target-applicability.toml) is the fail-closed
catalog of enabled compiler targets and corpus manifests. Every listed manifest
has one `target_applicability` table for every enabled target. Each table uses
exactly one of these contracts:

- `applicable` entries have a nonempty reason and identify either an executable
  corpus `run.sh` or a target-independent parse-only entry point;
- `inapplicable` entries have a nonempty reason that names the missing adapter,
  platform, or execution contract. They are never inferred from the host and
  are not reported as successful evidence.

[`report-target-applicability.py`](report-target-applicability.py) validates the
catalog and every table before printing the complete corpus-by-target matrix.
It rejects missing or additional manifests and targets, unknown statuses or
fields, empty reasons, absent or non-executable execution runners, absent parse
entry points, and an enabled target with no applicable evidence. The adapter
regression entry point runs this report on every invocation.

The hosted-header fixture is parse-only evidence for all enabled targets: it
does not claim ABI, link, or execution coverage. Executable applicability is
corpus-specific. The bzip2 runner covers every enabled target through native or
checked emulated execution. Csmith covers native
`x86_64-unknown-linux-gnu` and `aarch64-apple-darwin`; the remaining fetched
executable profiles currently have audited runners only for
`x86_64-unknown-linux-gnu`. The manifest catalog, not the build host, is
authoritative for each row.

C-Ray is a native generated-code benchmark rather than a cross-target
conformance oracle. Its runner supports x86-64 GNU/Linux and Apple-silicon
macOS, verifies every image before timing it, and rejects emulated or
cross-host performance comparisons.
