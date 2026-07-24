# CCC defined-behavior kernel benchmarks

This suite measures small executable workloads with fixed work counts and
built-in correctness checks. The current scalar, control-flow, memory, and ABI
slice covers direct calls and inlining, an unsigned-integer loop, exact
binary32/binary64 loops, data-dependent branches plus a dense switch, indexed
loads/stores, aggregate copies, thread-local access, C11 atomics, and a
variadic definition plus caller.

The suite is intentionally separate from the compiler-only runner in
`benchmarks/codegen`. Kernel results distinguish CCC's compiler-side
`primary_object.*` statistics from the final published `-c` object. Generated
TLS or ABI bridge units can be packaged into that final object after the
primary object is measured, so the two sizes must not be treated as equal.

## Cases

| Case | Fixed work | Self-validation |
| --- | ---: | --- |
| `direct-call` | 4,000,000 leaf calls | Unsigned result `0x99b0c920`; the call remains at `-O0`/`-Oz` and is inlined at `-O2`. |
| `integer-loop` | 4,000,000 integer iterations | Unsigned result `0x2b37aed1`. |
| `floating-loop` | 4,000,000 additions in 2,000,000 paired iterations | Both the binary32 and binary64 results equal exactly `1250001`. |
| `branch-switch` | 4,000,000 switch/branch iterations | Unsigned result `0x2f58cc08`. |
| `memory-traffic` | 4,000,000 indexed updates of a 256-word working set | Unsigned result `0xf8599ec7`; each update consumes two dynamic loads and publishes one store. |
| `aggregate-copy` | 1,000,000 assignments of a 32-byte structure | Unsigned result `0x294ffa8f`; copied destination fields feed the checksum. |
| `tls-access` | 1,000,000 thread-local updates | Unsigned result `0x19138677`; volatile accesses force CCC to package and call the target TLS address accessor. |
| `atomic-rmw` | 4,000,000 atomic operations in 1,000,000 iterations | Unsigned result `0xf133366f`; each iteration performs lock-free fetch-add, fetch-xor, compare-exchange, and load operations. |
| `variadic-call` | 1,000,000 calls | Unsigned result `0xb4ceb671`; the callee consumes an unsigned integer, signed integer, default-promoted float, and object pointer through `va_arg`. |

All wrapping calculations use unsigned arithmetic, every shift count is in
range, and all inputs are initialized. The floating-point steps and every
intermediate result are exactly representable in the enabled targets'
binary32/binary64 formats. Volatile seeds prevent a compiler from replacing a
kernel with its recorded answer. The memory kernels use separate statically
allocated working sets with in-range unsigned indices; the aggregate-copy
source and destination never overlap.

The TLS object is initialized per-thread and volatile so the loop retains its
source-level reads and writes; the benchmark does not measure thread creation.
The atomic case uses an aligned `atomic_uint`, valid memory-order pairs, and
only operations advertised as always lock-free by CCC's `<stdatomic.h>`.
The variadic case passes and consumes matching types. In particular, its
`float` actual argument is read as the required default-promoted `double`, and
the pointed-to object remains live for the duration of every call.

## Modes

- `object` compiles each kernel and records structural statistics for any
  enabled target. It does not require a linker or runnable target.
- `correctness` additionally links and executes each profile once. A
  non-native target requires an explicit `--runner`.
- `performance` requires the compiler's selected target to match the native
  host, rejects emulated runners, performs a validation execution first, and
  then records warmups and repeated samples.

Every execution must exit zero without writing stdout or stderr. The runner
retains failed command output and timing evidence instead of accepting a fast
incorrect result.

## Running

Build a release compiler and choose a new result directory:

```sh
cargo build --locked --release -p ccc-driver
benchmarks/kernels/run.py \
  --ccc target/release/ccc \
  --output /tmp/ccc-kernels \
  --mode performance
```

A quick native correctness run is:

```sh
benchmarks/kernels/run.py \
  --ccc target/debug/ccc \
  --output /tmp/ccc-kernels-correctness \
  --mode correctness \
  --compile-warmups 0 \
  --compile-samples 1
```

For cross-target correctness, place the emulator and its arguments before the
generated executable:

```sh
CCC_CC=riscv64-linux-gnu-gcc \
benchmarks/kernels/run.py \
  --ccc target/debug/ccc \
  --output /tmp/ccc-kernels-riscv64 \
  --mode correctness \
  --target riscv64-unknown-linux-gnu \
  --runner qemu-riscv64 \
  --runner-arg=-L \
  --runner-arg=/usr/riscv64-linux-gnu
```

QEMU execution is correctness and rough-trend evidence only. The runner marks
only native `performance` results as comparison-ready.

## Results

The result directory uses format version 2:

| Path | Contents |
| --- | --- |
| `summary.tsv` | Per-kernel/profile compile, link, runtime, structural, primary-object, final-object, and executable summary. |
| `build-times.tsv` | Raw compile and link resource measurements. |
| `run-times.tsv` | Validation, warmup, and runtime-sample measurements. |
| `codegen-stats.tsv` | Complete normalized `--emit=codegen-stats` stream. |
| `artifacts.tsv` | Final-object and executable byte sizes and hashes. |
| `manifest.tsv` | Fixed work/result contract and copied-source identities. |
| `environment.json` | Compiler hash, target/configuration, host, mode, and runner identity. |
| `commands.jsonl` | Exact command argument vectors in execution order. |
| `raw/` | Objects, executables, command output, timing JSON, and raw stats. |

Compile samples must produce identical final-object hashes. Runtime summaries
exclude validation and warmup executions. `runtime_ns_per_work_unit` is useful
only for comparing the same kernel and fixed work contract.

The fake-tool regression needs no CCC build:

```sh
benchmarks/kernels/test-run.sh
```
