# CCC real-program timing and validation

This directory separates native timing from correctness evidence. run.py never
validates a program, reads its stdout, round-trips data, computes output
checksums, creates validation artifacts, or invokes upstream tests. Its results
therefore do **not** establish correctness.

Use validate.py explicitly when correctness evidence is required. It is never
imported or called by run.py.

## Timing

run.py measures already-built executables using deterministic generated inputs
for compression and a deterministic Lua workload for interpretation. It sends
every program's stdout to /dev/null, retains stderr and timing JSON as
operational-failure evidence, and fails only for a nonzero exit status.

Operations are explicit:

- compression generates the declared deterministic input and times compression.
- decompression requires --decompression-input CASE=PATH for every selected
  compression case. Each fixture must already exist; it is recorded only as
  size/SHA-256 provenance and is never validated or discovered implicitly.
- interpreter runs the timing Lua script, which retains the computation but
  contains no checksum or error oracle.

Decompression throughput and work_count always use the declared generated
input byte count (--input-mebibytes), not the fixture size. Supplying a fixture
whose contents represent that contract is the operator's responsibility.

~~~sh
benchmarks/real-world/run.py \
  --output /tmp/ccc-real-world \
  --program bzip2=/path/to/bzip2 \
  --program zlib=/path/to/minigzip \
  --program zstd=/path/to/zstd \
  --program lua=/path/to/lua \
  --decompression-input bzip2=/fixtures/bzip2.input \
  --decompression-input zlib=/fixtures/zlib.input \
  --decompression-input zstd=/fixtures/zstd.input
~~~

Use --operations compression,decompression,interpreter to choose a subset; the
default includes every applicable operation.

## Explicit validation

validate.py generates the checked Lua workload. For compression cases it writes
a compressed artifact, decompresses it, and compares both byte count and
SHA-256 to the generated input. It writes validation-only commands, artifacts,
and evidence; it does not produce summary.tsv.

~~~sh
benchmarks/real-world/validate.py \
  --output /tmp/ccc-real-world-validation \
  --program bzip2=/path/to/bzip2 \
  --program zlib=/path/to/minigzip \
  --program zstd=/path/to/zstd \
  --program lua=/path/to/lua
~~~

## Timing results

Timing output uses format version 2.

| Path | Contents |
| --- | --- |
| summary.tsv | Timed-operation runtime, CPU/RSS, declared work contract, and throughput. |
| run-times.tsv | Warmup and sample timings only. |
| artifacts.tsv | Generated timing inputs, program identities, and fixture provenance. |
| environment.json | Host, selected operations, declared input bytes, and fixture size/SHA-256 provenance. |
| commands.jsonl | Timed command vectors only. |
| workloads/ | Timing stderr and timing JSON evidence. |

The fake-tool regression exercises timing and validation separately without
building or running a real program:

~~~sh
benchmarks/real-world/test-run.sh
~~~
