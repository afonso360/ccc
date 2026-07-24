# CCC defined-behavior kernel benchmarks

This suite measures small executable workloads with fixed work counts and
built-in correctness checks. The first vertical slice contains
`direct-call`, a four-million-call unsigned-integer kernel whose leaf is
eligible for CCC's current `-O2` inlining policy. It validates the exact
`0x99b0c920` result and returns zero only on success.

The suite is intentionally separate from the compiler-only runner in
`benchmarks/codegen`. Kernel results distinguish CCC's compiler-side
`primary_object.*` statistics from the final published `-c` object. Generated
TLS or ABI bridge units can be packaged into that final object after the
primary object is measured, so the two sizes must not be treated as equal.

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

The result directory uses format version 1:

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
