# CCC code-generation microbenchmarks

This directory contains small compiler-only benchmarks for tracking frontend,
CCC-IR, inlining, Cranelift, and object-emission costs without link or runtime
noise. Every timed command is an ordinary object-only `ccc -c ... -o ...`
compilation. One separate, untimed `ccc --emit=codegen-stats` invocation per
case and profile records the structure of the generated IR and primary object.

The default set covers:

- a minimal `main` returning zero;
- one direct `puts` call;
- one variadic `printf` call;
- equivalent `fputs`/`stdout` programs using either minimal declarations or
  the target's complete hosted `<stdio.h>`;
- generated translation units with 0, 32, 256, and 1024 unused function
  declarations;
- generated translation units with the same independent scale of unused data
  declarations;
- generated live call chains with 8, 32, and 128 functions.

The two declaration series are also regression checks. Increasing either
unused function prototypes or unused external data declarations must not
change any `post_inline_ir.*` metric or the selected primary-object byte,
symbol, undefined-symbol, relocation, and text-size metrics. This catches the
old behavior where trivial programs accumulated unused CLIF references or
object declarations. The live-function series provides a growing backend
workload for codegen performance work.

The hosted-header pair references exactly one function and one data object.
For each optimization profile, the `<stdio.h>` case must match the
minimal-declaration baseline in every `post_inline_ir.*` and
`primary_object.*` metric. Preprocessing and semantic-analysis time may grow;
CLIF and object structure may not. The two objects are not required to have the
same hash because libc headers may give `fputs` or `stdout` target-specific
physical symbol spellings. CI runs this structural check on every enabled
target; only controlled native runs should use its timings as performance
evidence.

## Running

Build CCC, choose a new result directory, and run:

```sh
cargo build --locked --release -p ccc-driver
benchmarks/codegen/run.py \
  --ccc target/release/ccc \
  --output /tmp/ccc-codegen-benchmark
```

Defaults are one warmup and five measured samples for `-O0`, `-O2`, and `-Oz`.
Use smaller or larger generated dimensions without editing fixtures:

```sh
benchmarks/codegen/run.py \
  --ccc target/release/ccc \
  --output /tmp/ccc-codegen-scaling \
  --profiles O2 \
  --warmups 2 \
  --samples 10 \
  --declaration-scales 0,100,1000,10000 \
  --data-declaration-scales 0,100,1000,10000 \
  --function-scales 1,16,64,256
```

Pass `--target=<triple>` to select an enabled target when its compiler driver
and sysroot are configured. The `hosted-header` family additionally requires
that target's `<stdio.h>`. Use `--cases` with a comma-separated subset to
isolate one family; declaration-only scaling uses `declaration-heavy` and
`data-declaration-heavy`. The output directory must be new or empty so
evidence from separate runs cannot be mixed accidentally.

## Results

The result directory is self-contained:

| Path | Contents |
| --- | --- |
| `summary.tsv` | Timing, RSS, and selected codegen metrics per case/profile. |
| `compile-times.tsv` | Per-run time, RSS, faults, context switches, and status. |
| `codegen-stats.tsv` | One normalized structural-stats record per case/profile. |
| `raw/` | Objects, compiler output, timing JSON, and untimed raw stats output. |
| `sources/` | Exact static and generated C translation units that were measured. |
| `manifest.tsv` | Source identities and any exact-equivalence baseline. |
| `commands.jsonl` | Exact compiler argument vectors in execution order. |
| `environment.json` | Host, compiler hash, effective target, and query evidence. |
| `effective-config/` | Resource, sysroot, and external tools per profile. |

Compare the same profile, target, compiler build mode, and host. The raw
`post_inline_ir.*` counters describe input to Cranelift's own passes;
`primary_object.*` describes CCC's primary object and excludes generated bridge
assembly. Compile timings cover only ordinary `-c` invocations. The structural
stats query runs after the timed samples and is never included in timing
summaries.

Use a release-built CCC for compiler-performance comparisons. A debug build is
useful for exercising the harness and invariants, but its timings are not a
performance baseline.

Every timed invocation has a unique explicit `.o` path. The objects are retained
and hashed, and repeated samples must produce identical object bytes. The
reported primary-object byte count must also match every timed object. The
compiler executable SHA-256, successful `-dumpmachine` query, raw query output,
effective target, and `--print-effective-config` output for every profile are
archived with each result set.

The runner uses `wait4` for per-process resource measurements and therefore
targets the same Unix hosts as CCC (Linux and Darwin). Peak RSS follows the
platform convention internally and is normalized to bytes.

## Regression test

The self-test uses a fake compiler, performs positive and negative
declaration-liveness and hosted-header-equivalence checks, and does not require
a CCC build:

```sh
benchmarks/codegen/test-run.sh
```
