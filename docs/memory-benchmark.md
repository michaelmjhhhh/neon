# Memory benchmark

The [2026-10-17 results](memory-results.md) include the complete before/after
matrix, process peaks, validation and observed regressions.

`script/memory-bench` uses Python's standard library and macOS `vmmap`. It runs
real Neon windows in the background, samples memory at acknowledged checkpoints,
and keeps the raw measurements. It never builds or modifies application binaries.

Build an instrumented release binary explicitly:

```sh
cargo build --release --features memory-bench
script/memory-bench run after --binary target/release/neon --runs 3
```

The `memory-bench` feature provides deterministic workloads without enabling
the optional diagnostics profiler. The ordinary application contains no memory
benchmark code. Benchmark launches skip application activation and create normal
windows without requesting focus. Input goes directly to that window through
GPUI; the harness does not send system keyboard events or bring Neon forward.
Every checkpoint must report an inactive window; an active window fails the run.
Keep the same display and scale and avoid interacting with benchmark windows.
Other applications can remain foreground; keep their workload consistent between
before and after measurements. Each scenario uses a fresh
process, with a 90-second timeout. An unexpected exit, benchmark error, missing
checkpoint, incorrect document size, or failed memory sample fails the run.
Use a new label to repeat a run; existing results are never overwritten.

## Before and after

Create a separate checkout of the baseline revision. Apply only the benchmark
harness and its feature/launch wiring to that checkout; retain its original
runtime code and dependency features. Build both binaries with the same release
profile and toolchain. Record the exact harness-only patch so the baseline can
be reproduced. The runner stores the source revision, working-tree status,
diff checksum, untracked source-file checksums, binary checksum, platform,
hardware model/physical memory, installed `rustc` version, execution mode, and
fixture manifest in each report. The recorded `rustc` version describes the
runner's environment; it does not prove which compiler produced an existing binary.
`--source` must identify the checkout that produced the supplied binary; the
runner records this provenance but cannot prove that a binary was built from it.

For example, with a baseline checkout at `../neon-before`:

```sh
cargo build --release --features memory-bench --manifest-path ../neon-before/Cargo.toml
script/memory-bench run before --binary ../neon-before/target/release/neon --source ../neon-before --runs 3
script/memory-bench run after --binary target/release/neon --source . --runs 3
script/memory-bench compare before after
```

The recorded baseline for this change is commit
`a01571e44113adef135a20c0335a7ae0ac038bf6` plus
[`memory-baseline.patch`](memory-baseline.patch), which adds only the benchmark
harness and launch wiring. Apply it with `git apply` in a clean checkout of that
commit. Use separate Cargo target directories when rebuilding both checkouts;
copy completed binaries before switching checkouts or feature sets.

Use the same runner checkout for both commands, so both binaries read the shared
`output/memory/fixtures` files. Comparison rejects different fixture manifests,
report versions, execution modes, platforms, hardware, display scales, scenario
orders, or repeat counts. Version 2 reports use background execution and cannot
be compared with earlier foreground runs. Closed-window
checkpoints report scale zero and are excluded from the display-scale check.

For a shorter investigation, pass the same subset to both runs:

```sh
script/memory-bench run small-before --binary ../neon-before/target/release/neon --source ../neon-before --runs 3 --scenarios startup,large
script/memory-bench run small-after --binary target/release/neon --runs 3 --scenarios startup,large
script/memory-bench compare small-before small-after
```

## Workloads

The fixtures are deterministic and checksummed. Their generator version is
recorded in `output/memory/fixtures/manifest.json`. Images are generated and
verified in chunks rather than allocating full decoded pixel buffers.

| Scenario | Workload and checkpoints |
| --- | --- |
| `startup` | Empty document, Appearance settings open, settings closed, window closed |
| `small` | 300 Markdown sections; loaded, 40 typed bytes in Split, another 40 in Editor, Preview, empty document, window closed |
| `large` | 8,000 sections / 56,000 lines; same phases as `small` |
| `images` | Three different local 4,000 × 3,000 RGB PNGs opened sequentially, then empty document and window closed |
| `blocks` | A 2,000-item list, 1,000-row three-column table, and 5,000-line code block, then empty document and window closed |
| `folder` | 20 directories containing 50 Markdown files each, then window closed |

Each Markdown section has a heading, paragraph, and two-item list. `startup`
does not open the Typography section; it measures settings before font pickers
are needed. The in-app harness waits for file loading and Markdown parsing,
allows one second to settle, and draws a real inactive window before each checkpoint.
Image phases also allow two seconds for asynchronous decoding and upload.
Typing is sent through GPUI keystroke dispatch, and the app verifies that each
typing phase inserts exactly 40 bytes. The runner independently checks document
byte counts and pane layouts. An Editor-only checkpoint may report zero preview
blocks because preview parsing is suspended there. The final `closed` sample is taken after the
window has been removed and given time to release its entities.

## Measurements and artifacts

Results are written under `output/memory/LABEL/`:

- `report.json`: raw checkpoint values, provenance, and median/min/max summaries.
- `report.md`: the scenario/checkpoint matrix.
- `run-N/SCENARIO/`: stdout, stderr, each raw `vmmap` result, checkpoint samples,
  and the acknowledgement directory.
- `compare-BEFORE.md` and `.json` under the after label: validated comparison.

Physical footprint is the primary process metric. Live heap is the **bytes
allocated** column for `DefaultMallocZone`, not its virtual, resident, dirty,
or fragmentation columns. This heap figure excludes other malloc zones and GPU
allocations. `vmmap`'s `K`, `M`, and `G` suffixes are interpreted as binary units;
tables display MiB. The matrix reports median [minimum, maximum] across runs.
Treat differences smaller than the run-to-run spread cautiously.

Peak physical footprint is cumulative since process launch, **not the peak of
an individual phase**. The report retains it separately from current memory.
The process-launch-to-first-checkpoint time includes deliberate launch/settling
delays (at least one second for the empty startup scenario), plus any file
loading in that scenario. It is **not a startup-latency measurement**. Later
checkpoint elapsed times also include earlier `vmmap` sampling and acknowledgements.

These are timed checkpoints, not proof that every background task is idle.
Syntax highlighting and image decoding have no public completion signal in the
current GPUI Kit API. Verify that images actually render when investigating
image results; the byte and block checks alone cannot establish that. Allocator
and graphics caches may retain freed pages, so live heap and footprint can fall
by different amounts.

The protocol is a stdout line:

```text
NEON_MEMORY_CHECKPOINT<TAB>NAME<TAB>BYTES<TAB>BLOCKS<TAB>SCALE<TAB>LAYOUT<TAB>ACTIVE
```

`ACTIVE` is `true` or `false`, including `false` after window closure. The runner
requires `false` and records `window_active` with every sample. It invokes
`/usr/bin/vmmap -summary PID`, saves and parses the output,
then creates `NEON_MEMORY_CONTROL/NAME.ack` so the app can continue. Every run
uses its own control directory. The runner terminates only the process it
launched when a timeout or failure occurs.

## Runner validation

```sh
script/memory-bench --help
script/memory-bench fixtures
script/memory-bench fixtures --verify
script/memory-bench selfcheck
script/memory-bench selfcheck --vmmap path/to/saved.vmmap.txt
```

These commands validate fixtures, PNG dimensions/pixel counts/checksums, memory
units, the allocated-byte column, incomplete-output rejection, and active-window
rejection. They do not launch Neon or add UI unit tests.

## Implementation and upstream research

Neon uses GPUI Kit 0.7.3 with GPUI Pre 0.3.9. The implementation was checked
against that installed dependency, and compared with Zed's official repository
at `56934ec0560bce8929025497411de7f5322fb6ca`:

- [GPUI image-cache ownership and eviction](https://github.com/zed-industries/zed/blob/56934ec0560bce8929025497411de7f5322fb6ca/crates/gpui/src/elements/image_cache.rs): document-owned caches permit decoded images and GPU tiles to be released together. Neon's cache targets 64 MiB / 128 entries; images needed by the current layout remain pinned, so this is not a hard memory ceiling or decode-size limit.
- [GPUI arena clearing](https://github.com/zed-industries/zed/blob/56934ec0560bce8929025497411de7f5322fb6ca/crates/gpui/src/arena.rs): clearing elements retains arena chunks. Splitting oversized lists into visible chunks avoids growing these retained allocations in the first place.
- [Zed background parsing](https://github.com/zed-industries/zed/blob/56934ec0560bce8929025497411de7f5322fb6ca/crates/language/src/buffer.rs): keep a small synchronous budget and let background parsing catch up. Neon now records edit coordinates without retaining a Rope snapshot for every intervening edit, and task completion holds weak ownership.

Hidden previews discard parsed blocks, row measurements and images while
retaining the latest shared Rope. A background word counter scans Rope chunks
without flattening the document, so Editor-only mode still reports words.
Typography picker entities exist only while that settings section is visible.
Ordinary builds omit the optional profiler and FPS sampling; use `diagnostics`
for frame measurements and the latency benchmark.

Large tables, code blocks and single deeply nested list items still render as
whole blocks. Visible large images still decode at full resolution. Those
workloads remain in the matrix so future changes can address their costs
without hiding them in aggregate averages.
