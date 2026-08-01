# CCC real-program benchmark suite

This is the native generated-code layer of CCC's performance evidence. It
measures already-built real executables on deterministic workloads; it does not
build a corpus or invoke its upstream test suite. Build and conformance
validation are deliberately separate concerns.

The suite complements:

- benchmarks/kernels, which isolates instruction-level and compact algorithmic
  regressions;
- this directory, which exercises complete bzip2, zlib, zstd, and Lua
  executables; and
- test-corpus/c-ray, the existing reference-checked native renderer benchmark.

## Workloads

| Case | Real program | Timed operations | Correctness gate |
| --- | --- | --- | --- |
| bzip2 | bzip2 1.0.8 | -9 compression and decompression | A 32 MiB generated input round-trips byte-for-byte. |
| zlib | zlib 1.3.2 minigzip | -9 compression and decompression | The same input round-trips byte-for-byte. |
| zstd | zstd 1.5.7 | Single-threaded compression and decompression | The same input round-trips byte-for-byte. |
| lua | Lua 5.5.0 | Deterministic table/number/bit-operation interpreter loop | The generated script checks its fixed 0x9bbfd5c9 result. |

The compression input consists of 64 KiB blocks: one pseudo-random block for
every three patterned record blocks. That mixture avoids a purely
incompressible or trivially repetitive input while keeping the source,
byte-count, and SHA-256 evidence reproducible. The default is 32 MiB and
--input-mebibytes changes it explicitly.

A one-time validation writes and checks full compressed and decompressed
artifacts. Warmups and timed samples send program output to /dev/null; this
keeps filesystem throughput out of the generated-code measurement. Timing is
native-host evidence only.

## Running

First produce the executables by your normal CCC build workflow. The benchmark
does not run that build or its tests. Then supply the exact executable paths:

~~~sh
benchmarks/real-world/run.py \
  --output /tmp/ccc-real-world \
  --program bzip2=/path/to/bzip2 \
  --program zlib=/path/to/minigzip \
  --program zstd=/path/to/zstd \
  --program lua=/path/to/lua
~~~

Use a controlled native host and a release build of each program. The default
is one warmup and five samples. To focus on one workload, select it and provide
only its executable:

~~~sh
benchmarks/real-world/run.py \
  --output /tmp/ccc-zstd \
  --cases zstd \
  --program zstd=/path/to/zstd \
  --input-mebibytes 64
~~~

test-corpus/c-ray/run.sh --profile performance remains the real renderer entry
point. It owns C-Ray's separate build/reference-image contract and is not
nested in this runner.

## Results

| Path | Contents |
| --- | --- |
| summary.tsv | Median/minimum/maximum runtime, CPU/RSS, work contract, throughput, and validation digest for each operation. |
| run-times.tsv | Every warmup and measured invocation. |
| artifacts.tsv | The generated input/script, supplied executable identities, and validation artifacts. |
| environment.json | Host, input/script identities, sample configuration, and supplied executable hashes. |
| commands.jsonl | Exact validation and timed command vectors. |
| workloads/ | Validation output, stderr, and raw timing JSON. |

validation_sha256 is the compressed artifact hash for compression, the original
input hash for decompression, and the workload-script hash for Lua. Do not
compare timing results across machines. Treat the recorded executable SHA-256
values as part of the result identity.

The fast runner regression uses a fake executable only; it neither builds nor
tests any real corpus:

~~~sh
benchmarks/real-world/test-run.sh
~~~
