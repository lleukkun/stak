# IO Development Journal

## Scope

This journal records the staged IO performance work after the initial benchmark run. The work is intentionally split into independently reviewable changes and must remain uncommitted until reviewed by the repository owner. No pull request is to be created from this work.

## Baseline

The benchmark results in `io-benchmark-results.txt` are fast-profile Criterion medians from a single-process run. Each payload benchmark constructs a fresh Stak VM per iteration, so the timings include VM setup, Scheme execution, data construction, and filesystem work.

The important 100k observations are:

- `make-bytevector`: 258.94 ms preparation.
- In-memory bytevector input preparation: 265.14 ms.
- In-memory bytevector output preparation: 258.75 ms.
- OS bytevector output preparation: 256.14 ms.
- In-memory bytevector read/write: 694.64/390.44 ms.
- OS bytevector read/write: 433.04/503.08 ms.
- In-memory UTF-8 string read/write: 531.44/709.95 ms.

The benchmark therefore exposes two independent costs:

1. `OsFileSystem` performs scalar host reads and writes, causing avoidable syscall overhead.
2. Bytevectors, strings, ports, and their intermediate lists use expensive Scheme-level construction and traversal. Buffering cannot fix this representation cost.

## First change: buffered `OsFileSystem`

The first implementation change keeps the public scalar `FileSystem` trait and Scheme port API unchanged. `OsFileSystem` will store input files in `BufReader<File>` and output files in `BufWriter<File>`.

Required behavior:

- Scalar reads and writes continue to have the same observable results.
- Explicit output `flush` makes buffered bytes visible.
- Successful `close` flushes output before dropping the descriptor.
- A failed close-time flush must not silently discard the descriptor state.
- Invalid descriptors and wrong-direction operations remain errors according to the existing backend contract.
- No changes are made to `LibcFileSystem`, VM representation, bytevectors, or Scheme ports in this change.

This change is deliberately useful but not expected to solve the benchmark by itself.

## Follow-up performance changes

1. Add a native `make-bytevector` builder that constructs the existing tag-8 factor-64 representation without Scheme-level repeated `vector-push!` operations.
2. Change in-memory bytevector ports to retain source-plus-position state instead of eagerly converting the source to a list; accumulate output chunks and combine them only when requested.
3. Apply the analogous source-position/chunk strategy to string ports, with UTF-8 decoding and encoding reviewed independently from the already-open correctness PR #3949.
4. Add native bulk port/filesystem operations only after the representation and port changes have been measured.
5. Defer compact raw bytevector objects, collector changes, and broad VM redesign until allocation and representation telemetry justifies them.

Vector's `../vector` runtime is design evidence for the later Vector-owned fast paths: it uses 64 KiB buffered descriptors, direct construction of Stak's existing bytevector tree, source-position in-memory ports, chunked output, and scalar fallback for generic ports.

## Review boundary

The first reviewable patch is limited to `file/src/file_system/os.rs` and its focused tests. It must not be committed or pushed until reviewed.

## 2026-08-08 - Buffered backend implemented locally

- Replaced `OsFileSystem`'s descriptor map values with 64 KiB `BufReader<File>` and `BufWriter<File>` handles.
- Kept the scalar `FileSystem` trait unchanged.
- Added explicit output flushing on `flush` and successful `close`.
- If close-time flushing fails, the descriptor is restored to the map instead of being silently discarded.
- Added focused tests for multi-buffer reads, close-time write persistence, wrong-direction access, and invalid descriptors.
- The enabled OS test module passes: 10 tests.
- `cargo fmt --all -- --check` passes.

The change remains uncommitted and no PR has been created.

### Validation update

- `cargo check -p stak-file` passes.
- `cargo test -p stak-file --features std` passes: 13 tests.
- `cargo test -p stak --features std --lib` passes as a downstream compilation check; the root library has no unit tests.
- Targeted Clippy and formatting checks remain part of the review gate.
- A workspace-wide test attempt was cancelled because it was too slow; it is not required for this focused patch.

### Final focused validation

- `cargo clippy -p stak-file --features std --all-targets -- -D warnings` passes.
- `cargo test -p stak-file --features std --lib` passes: 13 tests.
- `cargo check -p stak-file` passes.
- `cargo fmt --all -- --check` and `git diff --check` pass.
- The workspace-wide test command was intentionally not repeated after cancellation; the focused package and downstream root-library checks are the validation used for this review boundary.

## 2026-08-08 - Native `make-bytevector` implemented locally

- Added primitive number `600` as `MAKE_BYTEVECTOR` and dispatched it from `SmallPrimitiveSet`.
- Replaced the Scheme-level `make-vector` construction path with a native constructor while preserving the existing tag-8 bytevector representation: the outer rib stores the length and the cdr stores the factor-64 vector tree.
- The builder recursively creates filled leaves and internal nodes with existing `Memory::cons`/`Memory::allocate` APIs and keeps intermediate nodes on the VM stack as allocation roots. No representation or collector changes were made.
- Numeric validation accepts nonnegative integral lengths and fill bytes in `0..=255`; invalid numeric values are reported through the existing VM error wrapper.
- The optional fill remains provided by the Scheme wrapper, defaulting to zero.

### Native bytevector validation

- Corrected boundary probe passed for lengths `0`, `1`, `63`, `64`, `65`, `4096`, and `4097`, including fill values, mutation, length checks, and post-construction reads.
- Added `features/types/bytevector.feature` coverage for an empty bytevector and filled/mutated vectors at the 64- and 4096-element tree boundaries.
- `./tools/integration_test.sh -f std -i stak features/types/bytevector.feature` passed.
- `cargo test -p stak-r7rs --lib` passed; the crate currently has zero unit tests.
- `cargo check -p stak-r7rs --no-default-features` passed, covering the no-std-compatible numeric validation path.
- `cargo clippy -p stak-r7rs --all-targets -- -D warnings` passed.
- `cargo test -p stak --features std --lib` passed as a downstream compilation check.
- A forced-GC interpreter probe exposed an existing unrelated failure: even a scalar-only `(= (+ 1 2) 3)` program panics in `Vm::run_async` under `stak-interpret --features gc_always`, so it cannot currently serve as native-builder-specific evidence.

### IO benchmark comparison

Command:

```sh
cargo bench -p stak-bench --bench io --locked -- \
  --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5
```

Current Criterion medians at 100k, compared with the recorded baseline:

| Case | Baseline | Current | Change |
| --- | ---: | ---: | ---: |
| In-memory `make-bytevector` preparation | 258.94 ms | 0.629 ms | -99.8% |
| In-memory input-bytevector preparation | 265.14 ms | 6.746 ms | -97.5% |
| In-memory output-bytevector preparation | 258.75 ms | 0.538 ms | -99.8% |
| OS output-bytevector preparation | 256.14 ms | 0.476 ms | -99.8% |
| In-memory bytevector read | 694.64 ms | 441.372 ms | -36.5% |
| In-memory bytevector write | 390.44 ms | 130.301 ms | -66.6% |
| OS bytevector read | 433.04 ms | 394.514 ms | -8.9% |
| OS bytevector write | 503.08 ms | 121.112 ms | -75.9% |

The benchmark used the same fast 10-sample profile as the baseline. The native constructor removes most of the former Scheme-level construction cost; remaining payload costs include the unchanged bytevector traversal and port operations.

All changes remain uncommitted and no pull request has been created.

## 2026-08-08 - Codex review fixes for native `make-bytevector`

Codex identified two blocking correctness issues in the initial native constructor:

- The recursive builder pushed a completed list as a GC root but returned the pre-GC local pointer. The caller could therefore use a stale semispace address after `memory.push` triggered copying collection.
- Argument extraction used `memory.pop_numbers()`, whose `assume_number` path can panic for public Scheme values such as `#f`.

Fixes:

- After pushing each completed leaf or internal node, the builder now retrieves the relocated value from `memory.top()` before returning it.
- The primitive now pops ordinary `Value`s and converts them with `Number::try_from`, propagating `NumberExpected` through the existing R7RS error wrapper before range validation.

Regression coverage added:

- A direct primitive-layer test constructs a length-1 bytevector with a 24-value heap, forcing the small-heap copying-GC path and validating the resulting outer bytevector object.
- Direct primitive tests cover nonnumeric length and fill arguments.
- Feature coverage catches both public Scheme-level nonnumeric argument cases with `guard`.

Validation after the fixes:

- `cargo test -p stak-r7rs --lib`: 3 tests passed.
- `cargo clippy -p stak-r7rs --all-targets -- -D warnings`: passed.
- `cargo check -p stak-r7rs --no-default-features`: passed.
- `cargo test -p stak --features std --lib`: passed.
- `./tools/integration_test.sh -f std -i stak features/types/bytevector.feature`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

The benchmark results remain the earlier native-constructor measurements; this correction only changes GC-root recovery and argument failure handling, not the representation or normal allocation path.

## 2026-08-08 - Post-review benchmark rerun

The IO benchmark was rerun after the GC-root and nonnumeric-argument fixes with the same command and fast profile:

```sh
cargo bench -p stak-bench --bench io --locked -- \
  --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5
```

Current Criterion medians:

| Case | 10k | 100k |
| --- | ---: | ---: |
| In-memory `make-bytevector` preparation | 0.486 ms | 0.546 ms |
| In-memory input-bytevector preparation | 1.014 ms | 6.653 ms |
| In-memory output-bytevector preparation | 0.433 ms | 0.591 ms |
| In-memory bytevector read | 31.499 ms | 440.776 ms |
| In-memory bytevector write | 12.110 ms | 130.081 ms |
| OS input-bytevector preparation | 0.514 ms | 0.502 ms |
| OS output-bytevector preparation | 0.315 ms | 0.453 ms |
| OS bytevector read | 29.975 ms | 394.419 ms |
| OS bytevector write | 11.417 ms | 117.660 ms |

The correctness fixes do not introduce a material normal-path regression relative to the prior native-constructor run. The short 10-sample profile remains subject to normal Criterion variance.

## 2026-08-08 - Async test compatibility fix

Codex found that the new direct primitive tests called the `#[maybe_async]`-transformed `operate` method synchronously, so they failed to compile with the supported `async` feature.

- Added `stak-util` as an r7rs dev-dependency.
- Wrapped test primitive calls with the repository-standard `stak_util::block_on!` macro, making the tests work in both synchronous and asynchronous configurations.

Validation:

- `cargo test -p stak-r7rs --lib`: 3 tests passed.
- `cargo test -p stak-r7rs --features async --lib --no-run`: passed.
- `cargo test -p stak-r7rs --features async --lib`: 3 tests passed.
- `cargo clippy -p stak-r7rs --all-targets -- -D warnings`: passed.
- `cargo clippy -p stak-r7rs --features async --all-targets -- -D warnings`: passed.

## 2026-08-08 - In-memory bytevector ports optimized locally

- Changed `open-input-bytevector` to retain the source bytevector and a read index instead of eagerly converting the source with `bytevector->list`.
- Preserved scalar `read-u8`, `peek-u8`, EOF, and generic port buffering behavior.
- Changed `open-output-bytevector` to write into 64-byte bytevector chunks with `bytevector-u8-set!`; a chunk is consed only when full instead of consing once per byte.
- Output retrieval now finalizes a fresh bytevector by copying completed chunks and the partial chunk. Repeated retrievals produce equivalent results, and the port remains writable afterward.
- The port data field stores a finalizer thunk; `get-output-bytevector` invokes that thunk.

### Bytevector-port validation

- Added a port scenario covering output across the 64-byte boundary, repeated retrieval, continued writing, and partial output.
- Added a port scenario covering input indices `0`, `63`, and `64`, followed by repeated EOF reads.
- `./tools/integration_test.sh -f std -i stak features/types/port.feature`: 24 scenarios and 75 steps passed.
- `./tools/integration_test.sh -f std -i stak features/read.feature`: 93 scenarios and 479 steps passed.
- `cargo test -p stak-r7rs --locked`: 3 tests passed.
- `cargo fmt --all -- --check` and `git diff --check` passed.

### Clean IO benchmark rerun

The first post-change benchmark run showed implausible regressions because another project was building concurrently. After that build completed, the benchmark was rerun with the established command:

```sh
cargo bench -p stak-bench --bench io --locked -- \
  --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5
```

The clean rerun showed no material regression for in-memory bytevector ports:

| Case | 10k change | 100k change |
| --- | ---: | ---: |
| In-memory bytevector read | +0.32% | -1.23% |
| In-memory bytevector write | -0.44% | -0.79% |

Preparation measurements were statistically unchanged as well. The changes remain uncommitted and the unrelated untracked files remain preserved.

## 2026-08-09 - Incremental in-memory string-port retrieval

- `open-input-string` retains the source code-point tail and encodes one character directly to a short UTF-8 byte list, eliminating the temporary output-bytevector port previously created for every character.
- `open-output-string` now owns a mutable result string and tail pointer. ASCII bytes are appended immediately; multibyte sequences are retained as pending bytes and decoded when complete using the existing `parse-char-bytes` behavior.
- `get-output-string` invokes the port’s data thunk and returns the cached result directly. Repeated retrieval is therefore O(1) with respect to prior output and does not rescan or re-finalize the accumulated bytes.
- Continued writes after retrieval remain supported. Invalid or incomplete output bytes retain the previous decoding behavior as far as the existing parser permits.
- Codex P2 was deferred until the `origin/main` rebase, which supplied the upstream encoder fix. The post-rebase follow-up consolidates the remaining encoder logic while leaving strict decoder validation for a separate change.

### String-port validation

- The port scenarios cover 65-byte output, repeated retrieval, continued writing, and the final partial byte.
- The long UTF-8 input scenario covers ASCII at index 0, `あ` at index 63, and `😄` at index 64; it verifies all UTF-8 bytes and EOF.
- The interleaved retrieval regression writes one byte and retrieves the result after each write for 1,000 iterations, then verifies the final length and boundary characters.
- `./tools/integration_test.sh -f std -i stak features/types/port.feature`: **27 scenarios and 84 steps passed**.
- `./tools/integration_test.sh -f std -i stak features/types/string.feature`: **157 scenarios and 471 steps passed**.
- `./tools/integration_test.sh -f std -i stak features/read.feature`: **93 scenarios and 479 steps passed**.
- `cargo build --profile release_test --features std`, `cargo fmt --all -- --check`, and `git diff --check` passed.
- Codex’s scaling check verified correct 100,000-character output and reported interleaved write/retrieve CPU times of 0.01 s at 1,000 iterations, 0.02 s at 2,000, 0.04 s at 10,000, and 0.17 s at 100,000.

### String-port benchmark

The relevant fast-profile benchmark was rerun with:

```sh
TMPDIR=target/bench-tmp GOTMPDIR=target/bench-tmp \
cargo bench -p stak-bench --bench io --locked -- \
  --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5
```

Criterion’s current 100k mean point estimates are:

| Case | Current mean point estimate |
| --- | ---: |
| ASCII input-string | 165.59 ms |
| ASCII output-string | 167.53 ms |
| UTF-8 input-string | 285.79 ms |
| UTF-8 output-string | 294.40 ms |

Compared with the previously recorded optimized-source mean point estimates, input remained effectively flat while incremental output retrieval reduced the 100k output cases substantially:

| Case | Previous mean point estimate | Current mean point estimate | Change |
| --- | ---: | ---: | ---: |
| ASCII input-string | 164.29 ms | 165.59 ms | +0.8% |
| ASCII output-string | 605.80 ms | 167.53 ms | -72.3% |
| UTF-8 input-string | 283.65 ms | 285.79 ms | +0.8% |
| UTF-8 output-string | 1.297 s | 294.40 ms | -77.3% |

The run completed successfully with `TMPDIR=target/bench-tmp`. The 27-scenario port validation and the scaling check establish that repeated retrieval no longer causes quadratic rescanning while preserving current encoder behavior. All changes remain uncommitted, and unrelated untracked files remain untouched.

## 2026-08-09 - UTF-8 encoder and decoder helper consolidation

- Audited the UTF-8 paths after the rebase. `write-char` and `char->utf8-bytes` were the two Scheme encoders; `string->utf8` delegates through `open-input-string`, while `write-string` delegates through `write-char`.
- Added shared `for-each-utf8-byte` logic so both encoders select the same UTF-8 width and emit the same byte sequence. Removed the duplicated recursive `write-trailing-bytes` encoder.
- The decoder already shared `parse-char-bytes` across `read-char`, `peek-char`, and incremental `open-output-string` decoding. Extracted the duplicated lead-byte continuation-count calculation into `utf8-continuation-count`, used by both input framing and incremental output decoding.
- Added a string-port regression comparing `write-string` output with `string->utf8` for empty, ASCII, 2-byte, 3-byte, and 4-byte examples, including `é`, `あ`, `—`, and `😄`.
- Focused differential checks also compared input and incremental-output decoding for valid, truncated, invalid-continuation, and overlong byte sequences; the paths agree under the existing behavior.

### Consolidation validation

- `./tools/integration_test.sh -f std -i stak features/types/port.feature`: **28 scenarios and 87 steps passed**.
- `./tools/integration_test.sh -f std -i stak features/types/string.feature`: **163 scenarios and 489 steps passed**.
- `./tools/integration_test.sh -f std -i stak features/read.feature`: **93 scenarios and 479 steps passed**.
- UTF-8 encoder boundary comparison passed for code points `0`, `127`, `128`, `2047`, `2048`, `65535`, `65536`, and `1114111`.
- `cargo build --profile release_test --features std`, `cargo fmt --all -- --check`, and `git diff --check` passed.
- `cargo test -p stak-r7rs --locked --lib`: 3 tests passed.

### Strict decoder follow-up

Strict UTF-8 validation remains a separate medium-sized change. The current decoder still does not reject invalid continuation bytes, overlong encodings, surrogate values, or code points above `U+10FFFF`. Implementing that safely requires choosing compatible behavior for malformed and incomplete sequences across `read-char`, `peek-char`, `read-string`, `utf8->string`, and incremental `open-output-string` decoding. No strict-validation behavior was changed in this consolidation.

The encoder and continuation-count changes are ready for review in the current work-in-progress commit. Unrelated untracked files remain untouched.

## 2026-08-09 - UTF-8 review follow-up

- Replaced output-string list append and getter rescanning with a mutable string plus tail pointer; each emitted character is appended once, and pending UTF-8 state is bounded to at most four bytes.
- Changed streaming character framing to classify each continuation byte as it arrives. The first non-continuation byte is restored immediately, so a known-invalid prefix such as `E2 41` does not read past `41`.
- Made the public list-level decoder policy explicit with `decode-utf8-bytes-replacement` and `decode-utf8-bytes-strict`; `utf8->string` uses strict decoding and replacement-oriented paths remain explicit.
- Added regression scenarios for overlong, truncated, surrogate, out-of-range, and invalid-lead UTF-8 rejection, plus a custom input-port test proving `E2 41` returns replacement then `A` with exactly two underlying reads.
- `features/types/string.feature` and `features/types/port.feature`: **193 scenarios and 582 steps passed** together.
- `cargo build -p stak --profile release_test --features std` passed; `git diff --check` passed.

The changes remain uncommitted. Unrelated untracked files remain untouched.

## 2026-08-09 - UTF-8 decoder composition fixes

- Output-string retrieval now projects incomplete pending bytes without mutating decoder state, so retrieval between bytes no longer changes a sequence that later completes.
- Replacement recovery consumes one malformed byte at a time consistently: complete invalid prefixes emit one replacement per byte, and incomplete prefixes project or eventually produce one replacement per consumed byte.
- Added differential coverage comparing `read-char`, `decode-utf8-bytes-replacement`, and `open-output-string`, including three-/four-byte overlong forms, isolated continuation bytes, valid UTF-8 boundaries, retrieval between bytes, and repeated malformed `peek-char` calls.
- Focused port/string validation: **196 scenarios and 591 steps passed**.
- Full standard integration validation: **1,780 scenarios and 5,737 steps passed**. The read suite remains at **93 scenarios and 479 steps passed**.
- `cargo build -p no-std-no-alloc --profile release_test`, `cargo build -p stak --profile release_test --features std`, formatter, and `git diff --check` passed.

The changes remain uncommitted; unrelated untracked files remain untouched.

## 2026-08-09 - UTF-8 partial-prefix and retrieval scaling follow-up

- Input recovery now restores every consumed continuation prefix plus the first non-continuation in original order. `E2 82 41` and `F0 90 80 41` therefore decode as one replacement per malformed byte followed by `A`.
- Incomplete output bytes remain pending and invisible to `get-output-string` until a sequence completes. This preserves continued writes and keeps retrieval O(1), including interleaved retrieval during multibyte output.
- Added regressions for partial-prefix interruption, final output length, and 1,000-character multibyte retrieval interleaving.
- Focused port/string validation: **197 scenarios and 594 steps passed**.
- Direct checks produced `��A` and `���A` for the two partial-prefix cases. Interleaved multibyte retrieval measured approximately 0.02s, 0.03s, and 0.06s for 1,000, 2,000, and 5,000 characters.

The changes remain uncommitted; unrelated untracked files remain untouched.

## 2026-08-09 - Post-commit IO benchmark rerun

The IO benchmark was rerun after commit `89bf4fd5` using the established fast Criterion profile:

```sh
TMPDIR=$PWD/target/bench-tmp GOTMPDIR=$PWD/target/bench-tmp \
cargo bench -p stak-bench --bench io --locked -- \
  --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5
```

The first attempt used relative temporary paths and failed when the filesystem benchmark resolved them below the benchmark crate directory. The absolute-path rerun completed successfully. Current 100k payload throughput compared with the original values in `io-benchmark-results.txt`:

| Case | Original | Current | Change |
| --- | ---: | ---: | ---: |
| In-memory ASCII read | 279.30 KiB/s | 450.31 KiB/s | +61.2% |
| In-memory ASCII write | 256.53 KiB/s | 557.19 KiB/s | +117.2% |
| In-memory UTF-8 read | 367.51 KiB/s | 393.99 KiB/s | +7.2% |
| In-memory UTF-8 write | 275.11 KiB/s | 576.48 KiB/s | +109.5% |
| OS ASCII read | 654.99 KiB/s | 731.20 KiB/s | +11.6% |
| OS ASCII write | 369.93 KiB/s | 695.54 KiB/s | +88.0% |
| OS UTF-8 read | 698.37 KiB/s | 566.98 KiB/s | -18.8% |
| OS UTF-8 write | 444.42 KiB/s | 1,129.6 KiB/s | +154.2% |

Bytevector throughput also improved from the original run: in-memory read/write moved from 140.59/250.12 KiB/s to 174.13/274.03 KiB/s, while OS read/write moved from 225.51/194.12 KiB/s to 245.05/817.96 KiB/s.

These figures include VM setup and Scheme execution, and the short 10-sample profile is sensitive to normal system variance. The multibyte interleaved retrieval workload remained linear at approximately 0.02s, 0.03s, and 0.06s for 1,000, 2,000, and 5,000 characters. No source or test files were changed by the benchmark run.

## 2026-08-09 - FileSystem bulk-I/O contract (PR 2)

- Confirmed the buffered OS prerequisite is already present on this branch in commit `d7d49900` (`BufReader`/`BufWriter` with 64 KiB buffers); the current work stays at the `FileSystem` boundary and does not change port representation.
- Added caller-buffered `FileSystem::read_into` and `write_from` methods with scalar defaults, preserving source compatibility for `VoidFileSystem` and other scalar-only implementors. `read_into` permits short reads and reports EOF as `Ok(0)`; `write_from` completes the source or returns an error; empty slices are successful no-ops.
- Added block implementations for `OsFileSystem`, `LibcFileSystem`, and `MemoryFileSystem`. OS reads/writes use the existing buffered handles; libc writes handle partial progress without allocation; memory reads copy directly and advance the descriptor offset.
- Added memory, OS, and libc contract coverage for block boundaries, EOF, empty buffers, wrong-direction operations, invalid descriptors, and close persistence.

### PR 2 validation

- `cargo check -p stak-file --no-default-features`: passed.
- `cargo check -p stak-file --features std`: passed.
- `cargo check -p stak-file --features libc`: passed.
- `cargo test -p stak-file --features std --lib file_system::memory::tests::read_into`: passed.
- OS backend tests with `TMPDIR=$PWD/target/stak-tmp`: **10 passed**.
- libc backend tests with `TMPDIR=$PWD/target/stak-tmp`: **7 passed**.
- `cargo test -p stak-file --features libc --lib --no-run`, `cargo fmt --all -- --check`, and `git diff --check`: passed.

The changes remain uncommitted. Existing unrelated untracked files remain untouched.

## 2026-08-09 - Bulk port dispatch bootstrap fixes (PR 3)

- Confirmed that the root `prelude.scm` participates in compiler bootstrapping twice: `compiler/src/prelude.scm` is a symlink to it, the compiler build script compiles that prelude together with `compile.scm` into compiler bytecode, and `compile_r7rs` prepends the same prelude to every R7RS input.
- Traced the compiler `OutOfMemory` failure to a balanced-but-misplaced pair of parentheses. An extra close at the end of `read-bytevector!` ended `(stak io)` early, while a missing close at `get-output-bytevector` balanced the whole file later. The intervening definitions were consequently compiled in the wrong library context and caused runaway expansion. Corrected the library boundary.
- Replaced `caddr` in `make-output-port` with `(car (cddr rest))`; `(stak io)` imports `(stak base)`, which does not export `caddr`.
- Corrected native bytevector validation to inspect the tagged bytevector root in the outer rib's cdr. Checking the outer rib's tag rejected valid native bulk bytevectors with `cons expected`.
- Disabled the bulk-read callback while `port-data` contains bytes restored by `peek-u8`, preserving byte order before returning to the allocation-free bulk path.
- Added regressions for a buffered byte followed by a bulk bytevector read and for an empty native bulk file write.

### PR 3 validation

- `cargo test -p stak-compiler --lib`: **6 tests passed**.
- `cargo check -p stak --features std`: passed, including the proc-macro/compiler bootstrap.
- `cargo test -p stak-file --features std --lib` with `TMPDIR=$PWD/target/stak-tmp`: **14 tests passed**.
- `cargo test -p stak-r7rs --lib`: **3 tests passed**.
- `./tools/integration_test.sh -f std -i stak features/types/port.feature` with the same `TMPDIR`: **37 scenarios and 115 steps passed**.
- `cargo check -p stak-file --no-default-features`, `cargo fmt --all -- --check`, and `git diff --check`: passed.

The compiler heap-size experiment was reverted: clean rebuilds confirmed that increasing the heap changed the allocation used by `compile_bare`, but even four times the normal heap still failed. The structural prelude correction fixed the failure at the default heap size.

### PR3 follow-up validation

- The first locked benchmark run exposed a size-dependent `number expected` failure in native bulk writes for bytevectors longer than 64 elements. The native traversal skipped the intermediate vector node's `car`, so it did not mirror the Scheme `vector-cell` walk. Corrected the traversal and added a 65-byte native file read/write regression at the chunk boundary.
- Revalidated the corrected implementation: compiler tests, root std check, filesystem std/libc tests, R7RS tests, formatting, and diff checks passed; the port integration suite passed with 38 scenarios and 118 steps.
- The locked I/O benchmark completed successfully after the correction. Representative 100,000-byte medians were approximately 176.76 KiB/s in-memory bytevector read, 285.53 KiB/s in-memory bytevector write, 258.77 KiB/s OS bytevector read, and 9.88 MiB/s OS bytevector write. Benchmark output includes VM/setup overhead and is a short 10-sample profile.

## 2026-08-09 - PR3 review corrections and read-side bulk dispatch

- Corrected `write-bytevector` to honor its optional `start` and `end` range for both native bulk callbacks and scalar callback ports.
- Corrected `read-bytevector!` to return `0` for an empty range without probing the port, the number of bytes read for successful or partial reads, and the EOF object when no byte is available. `read-bytevector` now fills through the same bulk-capable path and trims short results.
- Centralized public bytevector range handling: reversed ranges are rejected, while oversized `end` values are clamped to the bytevector length to preserve existing Stak behavior before native or scalar dispatch.
- Fixed the two denied clippy lints (`needless_question_mark` and `missing_const_for_fn`).
- Strengthened boundary coverage with scalar file readback, distinct markers at the 4096/4097 vector-height boundary, native/scalar write-range cases, and native/scalar read count/EOF cases.
- Clarified that empty block-I/O buffers are successful no-ops for valid descriptors; implementations may validate descriptor and direction first.

### PR3 review-correction validation

- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p stak-file --all-features --lib -- -D warnings`: passed.
- `cargo test -p stak-compiler --lib`: **6 tests passed**.
- `cargo test -p stak-file --features std` with repository-local `TMPDIR`: **14 tests passed**.
- `cargo test -p stak-file --features libc` with repository-local `TMPDIR`: **11 tests passed**.
- `cargo test -p stak-r7rs` with repository-local `TMPDIR`: **3 tests passed**.
- `cargo check -p stak --features std`: passed.
- Complete non-extra port integration: **43 scenarios and 133 steps passed**.
- `git diff --check`: passed.

### Post-correction locked benchmark

Command: `TMPDIR=$PWD/target/bench-tmp GOTMPDIR=$PWD/target/bench-tmp cargo bench -p stak-bench --bench io --locked -- --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5`

100,000-byte median throughput:

- In-memory bytevector read: **443.71 KiB/s**.
- In-memory bytevector write: **287.11 KiB/s**.
- OS bytevector read: **10.02 MiB/s**.
- OS bytevector write: **9.68 MiB/s**.

The large OS read improvement confirms that `read-bytevector` now reaches the native bulk-read path rather than the scalar loop. All changes remain uncommitted.

## 2026-08-09 - Codex read-side correctness follow-up

- Removed scalar read lookahead: empty ranges now return `0` without touching the callback, and successful reads never probe beyond `end`.
- Centralized public bytevector range handling. Reversed ranges now fail consistently; oversized `end` values are clamped to the bytevector length to preserve existing Stak behavior before either native or scalar dispatch.
- Added a true native file bulk-read regression: the input is prepared with scalar writes, then distinct markers are read across the 64/65 and 4096/4097 vector boundaries, with count and EOF assertions plus scalar file verification.

### Follow-up validation

- Compiler tests: **6 passed**.
- Filesystem std tests: **14 passed**.
- Filesystem libc tests: **11 passed**.
- R7RS tests: **3 passed**.
- Root std check, formatting, Clippy, and `git diff --check`: passed.
- Read feature suite: **93 scenarios / 479 steps passed**.
- Port feature suite: **47 scenarios / 147 steps passed**.
- Locked I/O benchmark: completed successfully with the repository-local temporary-directory setup.

All changes remain uncommitted.

## 2026-08-09 - Native in-memory bytevector ports

- Added primitive `601` for native bytevector range copying over the existing 64-way vector tree.
- Replaced the in-memory bytevector port bulk callbacks' per-byte Scheme loops with native range copies while retaining the scalar callbacks, 64-byte output chunks, repeated retrieval behavior, and callback-port dispatch.
- The native primitive validates bytevector representations, source and destination ranges, empty ranges, and matches R7RS temporary-storage semantics for overlapping ranges by copying backward when the destination overlaps to the right.
- Added Rust tests for empty ranges, invalid ranges, both overlap directions, and markers across indices `0`, `63`, `64`, `4095`, and `4096`; the fixtures keep bytevectors rooted and pass under `gc_always`.
- Corrected the existing nonnumeric-fill fixture so the complete R7RS unit suite also passes under `gc_always`.
- Added bytevector and port feature regressions for overlap semantics and 4097-byte in-memory reads and writes across chunk/tree boundaries.

### Final benchmark

Locked command:

```text
TMPDIR=$PWD/target/bench-tmp GOTMPDIR=$PWD/target/bench-tmp cargo bench -p stak-bench --bench io --locked -- --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5
```

For a 100,000-byte payload, the final quiet-run medians were:

- In-memory bytevector read: **5.0681 MiB/s**.
- In-memory bytevector write: **3.5327 MiB/s**.
- OS bytevector read: **10.094 MiB/s**.
- OS bytevector write: **9.8041 MiB/s**.

The quiet rerun completed successfully after the earlier CPU-loaded run. Relative to the original approximately **443.90 KiB/s** read and **287.32 KiB/s** write, the in-memory paths now measure roughly **11.7x** and **12.6x** faster, respectively.

### Focused validation

- R7RS primitive tests: **7 passed** in both normal and `gc_always` configurations.
- Bytevector and port feature suites: **129 scenarios / 423 steps passed**.
- Formatting and `git diff --check`: passed.
- The full workspace test invocation was not completed because it exceeded the available interactive run time; the focused suites and release integration build passed.

All changes remain uncommitted for review.

## 2026-08-09 - Bulk textual string I/O

- Routed `read-string` through existing `port-bulk-read` callbacks when the input port has no buffered bytes, using bounded 64-byte chunks, incremental UTF-8 decoding, replacement behavior matching scalar `read-char`, and restoration of unread bytes when the requested character count is reached.
- Routed `write-string` through existing `port-bulk-write` callbacks using bounded 64-byte UTF-8 chunks; ports without bulk callbacks retain the scalar character path, and output ports may receive chunks split across UTF-8 sequences without losing decoder state.
- Added regressions for in-memory chunk boundaries, malformed UTF-8 replacement and unread-byte preservation, bulk-only output callback dispatch, and OS-backed UTF-8 string round trips.

### Validation

- Workspace compiler check, formatting, and `git diff --check`: passed.
- R7RS primitive tests: **7 passed**.
- Filesystem std tests: **14 passed**.
- Filesystem libc tests: **11 passed**.
- Compiler unit tests: **6 passed**; compiler doctests: **2 passed**.
- Feature-matrix checks for R7RS async, filesystem std, and filesystem libc: passed.
- Port feature suite: **52 scenarios / 162 steps passed**.
- File feature suite: **21 scenarios / 81 steps passed**.
- String feature suite: **165 scenarios / 495 steps passed**.
- Combined bytevector and port suites: **132 scenarios / 432 steps passed**.

### Locked string benchmark

Command:

```text
TMPDIR=$PWD/target/bench-tmp GOTMPDIR=$PWD/target/bench-tmp cargo bench -p stak-bench --bench io --locked -- --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5 'io/(in-memory-port|os-file).*string'
```

For a 100,000-character payload, the fresh median throughputs were:

- In-memory ASCII read: **409.39 KiB/s**.
- In-memory ASCII write: **598.40 KiB/s**.
- In-memory UTF-8 (`é`) read: **365.32 KiB/s**.
- In-memory UTF-8 (`é`) write: **475.50 KiB/s**.
- OS ASCII read: **925.56 KiB/s**.
- OS ASCII write: **1.2770 MiB/s**.
- OS UTF-8 (`é`) read: **685.01 KiB/s**.
- OS UTF-8 (`é`) write: **1.4124 MiB/s**.

The OS UTF-8 paths improved materially over their prior baselines; the in-memory paths now use the same bounded bulk boundary but remain dominated by Scheme-level in-memory encoding/decoding work. All changes remain uncommitted for review.

## 2026-08-10 - Native UTF-8 and shared vector traversal

- Added a shared VM cursor for list-backed tree vectors. Adjacent accesses now follow leaf-list links and only descend from the root at 64-cell boundaries; native file I/O and forward bytevector copies use the same traversal logic.
- Added streaming native UTF-8 length, encode, and decode primitives. Encoding reports both the remaining code-point list and bytes written; decoding reports code points and bytes consumed, defers incomplete non-final suffixes, preserves replacement decoding, and rejects malformed strict input.
- Routed `string->utf8`, `utf8->string`, and byte-backed textual bulk I/O through the native codecs while retaining bounded buffers and unread-byte restoration.
- Extended ports with optional textual read/write callbacks without changing the existing five-argument `make-port` contract or scalar/bulk callback positions. In-memory string ports use the textual path when their byte decoder has no pending partial sequence and fall back through byte-bulk or scalar dispatch otherwise.
- Increased native file batches to 1024 bytes and in-memory bytevector output chunks to 256 bytes.
- Added regressions for native codec validity and incomplete input, forced-GC relocation, direct textual dispatch, declined-callback fallback, zero-length reads, pending partial UTF-8 state, malformed input, and tree/chunk boundaries.

### Validation

- Existing release-test compiler bootstrap of `prelude.scm`: passed.
- Compiler unit tests: **6 passed**.
- R7RS primitive tests: **10 passed** in both normal and `gc_always` configurations.
- Filesystem std tests: **14 passed**; libc tests: **11 passed**.
- Port feature suite: **57 scenarios / 177 steps passed**.
- Combined string and file feature suites: **187 scenarios / 579 steps passed**.
- No-default filesystem check, R7RS async check, root std check, formatting, focused Clippy with warnings denied, and `git diff --check`: passed.
- The `compile_r7rs` compiler doctest was stopped after exceeding 60 seconds; the focused compiler and bootstrap checks above completed successfully.

### Locked focused I/O benchmark

Command:

```text
TMPDIR=$PWD/target/bench-tmp GOTMPDIR=$PWD/target/bench-tmp cargo bench -p stak-bench --bench io --locked -- --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5 'io/(in-memory-port|os-file)(/utf8)?/(read-string|write-string|read-bytevector|write-bytevector)/100000'
```

Fresh median throughputs for the 100,000-element cases were:

- In-memory bytevector read: **84.273 MiB/s**; write: **44.104 MiB/s**.
- In-memory ASCII string read: **3.1963 MiB/s**; write: **8.7652 MiB/s**.
- In-memory UTF-8 string read: **6.3290 MiB/s**; write: **17.564 MiB/s**.
- OS bytevector read: **127.19 MiB/s**; write: **120.99 MiB/s**.
- OS ASCII string read: **10.646 MiB/s**; write: **12.517 MiB/s**.
- OS UTF-8 string read: **16.205 MiB/s**; write: **23.281 MiB/s**.

The native codecs remove the Scheme-level per-byte arithmetic bottleneck, while the shared cursor also materially improves bytevector traversal. All changes remain uncommitted for review.

## 2026-08-10 - Native string copying and operation-only I/O timing

- Added native bounded code-point copying for in-memory string ports. Input reads now return an independent exact-length string and a remainder cursor; output writes append an independent copy, fixing the previous source-aliasing bug.
- Extended native UTF-8 decode results with the decoded count and both list endpoints. Scheme joins decoded chunks destructively and constructs the final string with its known length, avoiding repeated `length`, `reverse`, and `append` traversals and their extra list allocations.
- Increased the native codec buffer from 64 to 512 bytes and replaced per-sequence `core::str::from_utf8` calls with direct UTF-8 validation and decoding. The decoder still rejects overlong encodings, surrogates, out-of-range code points, and malformed strict input, while non-strict port decoding retains replacement behavior.
- Updated the codec-boundary feature to split multibyte input at the new 512-byte boundary and added a regression proving that an output string cannot mutate its source.
- Split throughput benchmark preparation from the measured operation with a benchmark-only VM gate. Source creation (`make-string` or `make-bytevector`), VM initialization, and port creation happen before Criterion starts timing; the existing `/prepare/` groups continue to measure that setup explicitly. A failed setup now reports an error instead of waiting indefinitely at the gate.

### Validation

- R7RS primitive tests: **11 passed** in normal and `gc_always` configurations.
- Compiler unit tests: **6 passed**.
- Combined string and port feature suites: **224 scenarios / 678 steps passed**.
- Root std check, R7RS no-default-features check, focused Clippy with warnings denied, formatting, benchmark compilation, and `git diff --check`: passed.

### Complete operation-only I/O benchmark

Command:

```text
TMPDIR=$PWD/target/bench-tmp GOTMPDIR=$PWD/target/bench-tmp cargo bench -p stak-bench --bench io --locked -- --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5
```

The complete 48-case run finished successfully. Criterion central estimates for the 100,000-element payload operations were:

- In-memory bytevector read: **132.18 MiB/s**; write: **54.224 MiB/s**.
- In-memory ASCII string read: **210.98 MiB/s**; write: **211.44 MiB/s**.
- In-memory UTF-8 (`é`) string read: **418.76 MiB/s**; write: **423.16 MiB/s**.
- OS bytevector read: **204.55 MiB/s**; write: **267.51 MiB/s**.
- OS ASCII string read: **102.96 MiB/s**; write: **130.21 MiB/s**.
- OS UTF-8 (`é`) string read: **98.828 MiB/s**; write: **172.32 MiB/s**.

The separate source-constructing 100,000-character string preparation cases measured approximately **7.15-7.38 ms**, while operation-only in-memory string reads and writes measured approximately **0.45 ms**. Unlike earlier benchmark results, the throughput denominator now excludes VM initialization, bytecode startup, `make-string` or `make-bytevector`, and port construction/opening. It still includes output retrieval for in-memory ports and close/flush for file operations. Therefore the earlier end-to-end and new operation-only throughput values are not direct speedup comparisons; their difference combines implementation improvements with the narrower timed region. This initial operation-only snapshot was later superseded in `io-benchmark-results-operation-only.txt` by the clean post-native-`make-string` rerun recorded below; the earlier end-to-end `io-benchmark-results.txt` remains unchanged.

All changes remain uncommitted for review.

## 2026-08-10 - Native `make-string`

- Added primitive `606` as a native filled-string constructor, matching the role of native `make-bytevector` while preserving the existing `(length . tagged-code-point-list)` representation.
- Kept the public Scheme procedure and its optional fill-character behavior unchanged. The wrapper converts the fill character to a code point; the native primitive validates the length and Unicode scalar, allocates the code-point list while keeping its root visible to the collector, and constructs the tagged string directly.
- Reused the UTF-8 codec's Unicode scalar validation instead of creating a second validity rule.
- Added native representation and invalid-code-point tests. The allocation test passes with `gc_always`, and the existing zero-length, default-fill, explicit-fill, mutation, conversion, and 512-byte codec-boundary scenarios continue to pass.

### Validation

- R7RS primitive tests: **13 passed** in normal and `gc_always` configurations.
- String feature suite: **166 scenarios / 498 steps passed**.
- Compiler unit tests: **6 passed**.
- R7RS no-default-features check, focused Clippy with warnings denied, formatting, release-test build, and `git diff --check`: passed.

### Focused preparation benchmark

Command:

```text
TMPDIR=$PWD/target/bench-tmp GOTMPDIR=$PWD/target/bench-tmp cargo bench -p stak-bench --bench io --locked -- --sample-size 10 --warm-up-time 0.2 --measurement-time 0.5 '^io/(in-memory-port|os-file)(/utf8)?/prepare/(read-string|write-string)/(10000|100000)$'
```

Criterion central estimates for 100,000-character preparation changed as follows:

- In-memory ASCII input: **7.1609 ms -> 0.41971 ms**; output: **7.1464 ms -> 0.43018 ms**.
- In-memory UTF-8 (`é`) input: **7.1765 ms -> 0.41989 ms**; output: **7.1740 ms -> 0.66513 ms**. The UTF-8 output sample was visibly noisy, with a **0.583-0.752 ms** interval.
- OS ASCII output: **7.3778 ms -> 0.46187 ms**.
- OS UTF-8 (`é`) output: **7.3690 ms -> 0.46241 ms**.
- OS input preparation does not construct a string and remained approximately unchanged at **0.32098 ms** for ASCII and **0.31969 ms** for UTF-8.

The clean 100,000-character cases show roughly a **16-17x** reduction in source-constructing preparation time. The final representation still contains one cons cell per character, so the remaining approximately **0.42-0.46 ms** is primarily the required native allocation work plus VM and port setup.

### Clean complete benchmark rerun

The complete 48-case benchmark was rerun after native `make-string` so every preparation and payload estimate comes from one build and Criterion process. The command was the same locked 10-sample profile used for the earlier complete run.

Criterion central estimates for the 100,000-element payload operations were:

- In-memory bytevector read: **135.18 MiB/s**; write: **55.175 MiB/s**.
- In-memory ASCII string read: **222.94 MiB/s**; write: **222.51 MiB/s**.
- In-memory UTF-8 (`é`) string read: **443.02 MiB/s**; write: **440.82 MiB/s**.
- OS bytevector read: **207.37 MiB/s**; write: **267.77 MiB/s**.
- OS ASCII string read: **103.43 MiB/s**; write: **146.60 MiB/s**.
- OS UTF-8 (`é`) string read: **99.519 MiB/s**; write: **187.77 MiB/s**.

The clean 100,000-character in-memory ASCII preparation estimates were **0.41375 ms** for input and **0.42419 ms** for output. UTF-8 input was **0.41215 ms**; UTF-8 output was noisier at **0.71868 ms** with a **0.601-0.768 ms** interval. Several OS preparation-only samples were likewise noisy, while payload intervals remained tight. The complete tables in `io-benchmark-results-operation-only.txt` now contain only this post-native-`make-string` run; the temporary mixed-run follow-up note was removed.

All changes remain uncommitted for review.

## 2026-08-10 - Operation-only benchmark measurement review

- Replaced Criterion's small-input batching with sequential custom iterations. Each payload iteration now owns exactly one prepared VM, worker thread, and set of port resources, instead of retaining a batch of suspended 32 MiB VM heaps and open ports.
- Moved payload timing into the VM worker. The clock starts after the benchmark gate receives its resume signal and stops when the remaining bytecode finishes, excluding channel handoff, worker wake-up, and thread join from the reported duration.
- Renamed the existing in-memory textual-callback scenario and added a true byte-bulk string-read regression. The new case splits a three-byte UTF-8 character at byte 511 of the 512-byte codec buffer and verifies that bytes read ahead are restored for the next `read-string` call.
- Documented that `ListVectorCursor` stores direct heap locations and is valid only with the same allocation-free `Memory` between cursor operations.

### Validation

- Port feature suite: **59 scenarios / 183 steps passed**.
- R7RS primitive tests: **13 passed** in normal and `gc_always` configurations.
- Filesystem unit tests: **4 passed**.
- Benchmark compilation, no-default filesystem check, formatting, and `git diff --check`: passed.

### Clean worker-timed I/O benchmark

The complete 48-case locked benchmark was rerun with the same 10-sample fast profile. Criterion central estimates for the 100,000-element payload operations were:

- In-memory bytevector read: **137.87 MiB/s**; write: **55.481 MiB/s**.
- In-memory ASCII string read: **231.95 MiB/s**; write: **231.58 MiB/s**.
- In-memory UTF-8 (`é`) string read: **465.87 MiB/s**; write: **464.04 MiB/s**.
- OS bytevector read: **220.72 MiB/s**; write: **261.53 MiB/s**.
- OS ASCII string read: **107.41 MiB/s**; write: **134.71 MiB/s**.
- OS UTF-8 (`é`) string read: **101.68 MiB/s**; write: **186.23 MiB/s**.

The 10,000-element cases improved most after removing the fixed synchronization and join cost from the measured interval; the 100,000-element cases changed less. The refreshed `io-benchmark-results-operation-only.txt` contains the complete preparation and payload tables from this worker-timed run. All changes remain uncommitted for review.
