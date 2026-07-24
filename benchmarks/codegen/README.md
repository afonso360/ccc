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
- equivalent variadic `printf` programs using either a minimal declaration or
  the target's complete hosted `<stdio.h>`;
- generated translation units with 0, 32, 256, and 1024 unused function
  declarations;
- generated translation units with the same independent scale of unused data
  declarations;
- four fixed live functions containing 0, 32, 256, or 1024 unused block-scope
  function declarations apiece;
- generated live call chains with 8, 32, and 128 functions;
- independent generated axes for live conditional blocks, dependent SSA
  values, referenced global definitions, and distinct referenced string
  literals.

The declaration series are also regression checks. Increasing either
translation-unit-level unused function prototypes or unused external data
declarations must not change any `post_inline_ir.*` metric or the selected
primary-object byte, symbol, undefined-symbol, relocation, and text-size
metrics. The independent `declarations-per-function` family keeps a four-call,
five-emitted-function backend workload fixed while adding exactly the selected
number of unused block-scope prototypes to each of the four callees. It
requires every `post_inline_ir.*` and `primary_object.*` metric to remain
identical across scales. Its archived source and environment record both the
per-function scale and fixed function count, so increasing frontend input
cannot masquerade as increasing CLIF or object work. These checks catch the old
behavior where trivial programs accumulated unused CLIF references or object
declarations.

The live-function series provides a growing backend workload for codegen
performance work. The four structural axes fail closed per optimization
profile: every adjacent scale must grow its defining metric within a checked
linear bound. The block series tracks live CLIF blocks; the SSA series tracks
live block parameters and results of live instructions; the global series
tracks CLIF global values, defined symbols, and initialized data; and the
string series tracks CLIF global values, relocations, and read-only data. These
checks reject accidentally dead fixtures and structural quadratic growth
before their timings are considered.

The `hosted-header` pair references exactly one function and one data object;
the `hosted-printf` pair references exactly one variadic function. For each
optimization profile, each `<stdio.h>` case must match its minimal-declaration
baseline in every `post_inline_ir.*` and `primary_object.*` metric.
Preprocessing and semantic-analysis time may grow; CLIF and object structure
may not. The paired objects are not required to have the same hash because
libc headers may give referenced declarations target-specific physical symbol
spellings. CI runs both structural checks on every enabled target; only
controlled native runs should use their timings as performance evidence.

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
  --declarations-per-function-scales 0,100,1000,10000 \
  --function-scales 1,16,64,256 \
  --block-scales 0,16,128,512 \
  --value-scales 0,100,1000,10000 \
  --global-scales 0,100,1000,10000 \
  --string-scales 0,100,1000,10000
```

Pass `--target=<triple>` to select an enabled target when its compiler driver
and sysroot are configured. The `hosted-header` and `hosted-printf` families
additionally require that target's `<stdio.h>`. Use `--cases` with a
comma-separated subset to isolate one family; declaration-only scaling uses
`declaration-heavy`, `data-declaration-heavy`, and
`declarations-per-function`, while structural scaling uses `block-count`,
`ssa-values`, `live-globals`, and `string-literals`.
`declarations-per-function` and every structural family require at least two
scales so their adjacent-scale invariants cannot be bypassed. The output
directory must be new or empty so evidence from separate runs cannot be mixed
accidentally.

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
`post_inline_ir.*` counters describe input to Cranelift's own passes. In
particular, `post_inline_ir.values` counts block parameters plus instruction
results reachable through the final CLIF layout, not detached data-flow-graph
entities.
`primary_object.*` describes CCC's primary object and excludes generated bridge
assembly. Compile timings cover only ordinary `-c` invocations. The structural
stats query runs after the timed samples and is never included in timing
summaries.

Use a release-built CCC for compiler-performance comparisons. A debug build is
useful for exercising the harness and invariants, but its timings are not a
performance baseline.

Every timed invocation has a unique explicit `.o` path. The objects are retained
and hashed, and repeated samples must produce identical object bytes.
`primary_object.file_bytes` describes Cranelift's primary relocatable object;
the timed `.o` byte count can be larger when the driver packages generated ABI
support alongside it. The runner requires the two sizes to match for cases
without generated support and retains both sizes for variadic cases where they
are distinct compiler boundaries. The compiler executable SHA-256, successful
`-dumpmachine` query, raw query output, effective target, and
`--print-effective-config` output for every profile are archived with each
result set.

The runner uses `wait4` for per-process resource measurements and therefore
targets the same Unix hosts as CCC (Linux and Darwin). Peak RSS follows the
platform convention internally and is normalized to bytes.

## Regression test

The self-test uses a fake compiler, performs positive and negative
translation-unit and per-function declaration-liveness,
hosted-header-equivalence, dead-axis, and superlinear structural-growth checks,
and does not require a CCC build:

```sh
benchmarks/codegen/test-run.sh
```
