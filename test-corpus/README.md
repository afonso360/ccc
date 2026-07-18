# Test corpus metadata

Third-party corpus pins, hashes, provenance, and fetch metadata live here.
Corpus source is fetched into disposable storage and is never silently replaced
by a host-installed copy.

| Corpus                                            | Contract                                                                                                                 |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| [Hosted GNU-like headers](libc-headers/README.md) | Original project fixture for deterministic preprocessing and parsing                                                     |
| [SQLite 3.47.2](sqlite/README.md)                 | Fetched canonical source, audited configure/Makefile adapter, and explicit `veryquick`, `quick`, `all`, and `full` modes |
| [Lua 5.5.0](lua/README.md)                        | Fetched official source and tests, audited upstream Linux build, and official basic profile                              |

`test-adapter-environment.sh` checks the shared native-GCC identity boundary
and removal of ambient GNU Make injection used by the executable corpus
adapters.
