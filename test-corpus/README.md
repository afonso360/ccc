# Test corpus metadata

Third-party corpus pins, hashes, provenance, and fetch metadata live here.
Corpus source is fetched into disposable storage and is never silently replaced
by a host-installed copy.

| Corpus                                            | Contract                                                                                                                 |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| [Hosted GNU-like headers](libc-headers/README.md) | Original project fixture for deterministic preprocessing and parsing                                                     |
| [SQLite 3.47.2](sqlite/README.md)                 | Fetched canonical source, audited configure/Makefile adapter, and explicit `veryquick`, `quick`, `all`, and `full` modes |
| [Lua 5.5.0](lua/README.md)                        | Fetched official source and tests, audited upstream Linux build, and official basic profile                              |
| [Redis 8.8.0](redis/README.md)                    | Fetched official core source and audited adapter; fail-loud native-f80 boundary before the server/CLI smoke profile      |
| [bzip2 1.0.8](bzip2/README.md)                    | Fetched official source and test repository, upstream checks, deterministic round trip, and 440-check extended profile  |
| [zstd 1.5.7](zstd/README.md)                      | Fetched official source, audited portable build, bounded upstream checks, and deterministic file/stream round trips      |
| [Csmith 2.4.0](csmith/README.md)                  | Pinned generated programs, strict C11 admission, GCC/Clang consensus, reproducible seeds, and retained failure artifacts  |

[`test-adapters.sh`](test-adapters.sh) runs every shell-only adapter regression,
including the shared native-GCC identity boundary and removal of ambient GNU
Make injection. The fetched builds and execution profiles remain explicit
Linux jobs invoked through each corpus's `run.sh`.
