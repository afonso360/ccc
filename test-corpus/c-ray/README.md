# C-Ray generated-code benchmark

This adapter fetches the official C-Ray 1.1 release, verifies the archive,
unmodified `c-ray-mt.c`, and selected scene by size and cryptographic hash, and
builds the one C translation unit with CCC at `-O0`, `-O2`, and `-Oz`. The
source remains in the disposable work directory; it is not vendored into CCC.
The retained `upstream-license-notice.txt` comes directly from the pinned
GPL-2.0-or-later source header.

Every run also builds a strict-floating-point native reference at `-O2`.
Upstream's `-ffast-math` Makefile default is deliberately not used. The runner
first verifies that CCC and the reference compiler both describe a
little-endian LP64 target, then supplies C-Ray's required `LITTLE_ENDIAN`
definition. It requires an exact binary `P6` image of the requested dimensions,
byte-identical CCC output across all three optimization profiles, and
byte-identical agreement with the same-host reference before retaining any
timing.

Two profiles are available:

| Profile | Scene | Invocation | Default samples | Purpose |
| --- | --- | --- | --- | --- |
| `correctness` | `scene` | `-t 1 -r 1 -s 320x240` | no warmup, 1 sample | Fast local and adapter validation |
| `performance` | `sphfract` | `-t 1 -r 1 -s 800x600` | 1 warmup, 5 samples | Stable scalar generated-code measurement |

Both use one worker and one ray per pixel. This removes scheduler scaling and
unused jitter samples from the comparison. Override `--warmups` and `--samples`
to increase repetition; measured samples must remain positive.

## Running

Build CCC and run from the repository root:

```sh
cargo build --locked -p ccc-driver

test-corpus/c-ray/run.sh --profile correctness \
  --work-dir "$CCC_TEST_ROOT/c-ray-correctness"

test-corpus/c-ray/run.sh --profile performance \
  --work-dir "$CCC_TEST_ROOT/c-ray-performance"
```

Without `--source-archive`, the runner downloads the pinned
`c-ray-1.1.tar.gz` into `${XDG_CACHE_HOME:-$HOME/.cache}/ccc/corpus/c-ray`.
For an offline run:

```sh
test-corpus/c-ray/run.sh --profile performance \
  --source-archive /path/to/c-ray-1.1.tar.gz \
  --work-dir "$CCC_TEST_ROOT/c-ray-performance"
```

The supported native profiles are x86-64 GNU/Linux and Apple-silicon macOS.
Linux defaults to GCC as the reference and link driver. macOS defaults to Apple
Clang, the active macOS SDK, and deployment target 11.0. Reproduce an
environment with `--target`, `--reference-cc`, `--sdk-root`, and
`--deployment-target`; cross-target or emulated timings are intentionally
rejected.

Required common tools are Bash, Python 3, Curl when the archive is not supplied,
OpenSSL with SHA3-256, Tar, and the usual POSIX file/text utilities. Linux uses
GNU-compatible `size`; macOS uses the active developer toolchain's `llvm-size`.
The compiler and reference/link driver must have libc, libm, and pthreads for
the selected native target.

## Results

`summary.tsv` is the comparison entry point. For each compiler/profile it
records:

- compile and link wall time;
- sample count and median/minimum/maximum render wall time;
- compile peak RSS and median render peak RSS;
- post-inlining CLIF function, block, instruction, call, stack-slot, signature,
  external-reference, and global-value counts for every CCC profile;
- object and executable byte size;
- portable object-section totals for text, read-only data, writable data, BSS,
  unwind metadata, debug metadata, and uncategorized sections;
- the validated image SHA-256.

`timings.tsv` and the per-command JSON files under `timings/` retain every raw
wall, CPU, peak-RSS, exit-status, and command measurement. `commands.txt`,
compiler identity and macro files, per-run C-Ray stderr, and every validated
output hash make a result auditable. `object-sections.tsv` retains every exact
section name and its normalized category;
`object-section-totals.tsv` is the machine-readable input to `summary.tsv`;
`object-sections.txt`, `object-size.txt`, and `executable-size.txt` preserve the
raw size-tool output. `codegen-stats.tsv` retains CCC's complete versioned
compiler-side metric set, while each profile's original key/value dump remains
under `tool-output/`. Section totals include virtual sections such as BSS, so
they are evidence about the generated program layout rather than a replacement
for the on-disk `object_bytes` value. The strict-FP reference has no CLIF
columns. `reference.ppm` is the correctness oracle retained for the run.

Compare measurements only between runs on the same controlled native host.
The adapter records evidence; it does not hide noise, normalize results from
different machines, or accept a faster image that fails exact comparison.
