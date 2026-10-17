# Memory optimization results — 2026-10-17

Measured on macOS 26.3 (25D125), ARM64 Mac16,12 with 16 GiB RAM,
Rust 1.99.1, a 1200 × 800 logical-pixel window and display scale 2.
Both releases use the same fixtures and benchmark harness. Six scenarios ×
three fresh processes per version produced **168 valid checkpoints**; every
sampled window was inactive. These are background measurements, not foreground
interaction or startup-latency measurements.

The baseline is `a01571e44113adef135a20c0335a7ae0ac038bf6` plus
[memory-baseline.patch](memory-baseline.patch). The optimized version uses the
same release profile. Baseline profiling is always enabled; the optimized
ordinary build disables it. Both binaries enable only `memory-bench` explicitly.
See [methodology and reproduction](memory-benchmark.md).

## Findings and limits

- Empty-window live heap falls from 12.9 to 8.4 MiB (34.6%). Startup lifetime peak footprint falls from 103.9 to 96.0 MiB (7.6%).
- After three distinct large images, live heap falls from 150.9 to 54.6 MiB; after replacing the document with empty text, it falls from 150.8 to 8.7 MiB (94.2%). Image memory no longer accumulates across document navigation.
- The 2,000-item list uses 18.4 MiB live heap instead of 82.7 MiB (77.8%). Large-document Editor-only mode uses 99.1 instead of 118.5 MiB.
- **Not every footprint checkpoint improves.** In the initial matrix, clearing the large document raises footprint from 158.9 to 230.3 MiB, while live heap falls from 16.0 to 11.2 MiB. Closed-window footprint also remains higher. A subsequent six-pair investigation reverses the median footprint difference but has wide, overlapping ranges. Release footprint therefore has no demonstrated stable improvement; the original regression remains recorded below.
- Background footprint varies with window occlusion, graphics-buffer reclamation and allocator state. Startup's current-footprint reduction is much larger than its stable heap/peak reduction; do not attribute that entire difference to these code changes. Small-file release and folder footprint ranges overlap substantially.
- The `blocks` workload opens list → table → code in one process. Later phases inherit the earlier allocation high-water mark. Tables and code blocks were **not** internally virtualized; their reductions do not establish an isolated table/code rendering improvement.
- Visible images still decode at full resolution. The 64 MiB cache target may be exceeded by images needed for the current layout. Oversized nested items, tables and code blocks remain further optimization opportunities.

## Process lifetime peaks

Median [min, max] MiB across three processes. Each peak covers its whole scenario,
not an individual checkpoint.

| Scenario | Before peak footprint | After peak footprint | Change |
| --- | ---: | ---: | ---: |
| startup | 103.9 [103.8, 104.0] | 96.0 [96.0, 96.1] | -7.6% |
| small | 139.3 [139.0, 139.6] | 131.9 [131.8, 159.5] | -5.3% |
| large | 513.0 [470.6, 547.9] | 426.0 [416.8, 458.9] | -17.0% |
| images | 480.6 [480.6, 485.7] | 287.7 [287.0, 334.8] | -40.1% |
| blocks | 231.0 [230.7, 231.4] | 177.1 [176.3, 196.7] | -23.3% |
| folder | 104.0 [103.7, 104.1] | 97.4 [96.7, 112.0] | -6.3% |

## Complete checkpoint matrix

Median [min, max] MiB. Live heap is the allocated-byte column for
DefaultMallocZone; it excludes other malloc zones and GPU allocations.

| Scenario | Checkpoint | Before footprint | After footprint | Change | Before live heap | After live heap | Change |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| startup | empty | 103.6 [53.5, 103.8] | 45.8 [45.7, 45.9] | -57.8 (-55.8%) | 12.9 [12.9, 13.0] | 8.4 [8.4, 8.4] | -4.5 (-34.6%) |
| startup | settings | 103.9 [53.7, 104.0] | 46.0 [46.0, 46.1] | -57.9 (-55.7%) | 13.1 [13.1, 13.1] | 8.5 [8.5, 8.6] | -4.6 (-34.8%) |
| startup | settings_closed | 53.8 [53.7, 53.9] | 46.2 [46.1, 46.2] | -7.6 (-14.1%) | 13.2 [13.2, 13.2] | 8.6 [8.6, 8.7] | -4.6 (-34.7%) |
| startup | closed | 53.8 [53.7, 53.8] | 46.2 [46.0, 46.2] | -7.6 (-14.1%) | 11.8 [11.8, 11.8] | 7.7 [7.7, 7.7] | -4.1 (-34.7%) |
| small | loaded | 133.2 [133.0, 133.2] | 125.9 [125.8, 126.2] | -7.3 (-5.5%) | 19.5 [19.5, 19.5] | 14.8 [14.8, 14.8] | -4.7 (-24.1%) |
| small | typed_split | 135.2 [87.7, 135.8] | 128.6 [128.4, 144.6] | -6.6 (-4.9%) | 19.8 [19.8, 19.8] | 15.1 [15.1, 15.6] | -4.7 (-23.7%) |
| small | typed_editor | 135.9 [127.7, 136.1] | 128.7 [80.8, 145.0] | -7.2 (-5.3%) | 19.9 [19.9, 19.9] | 14.6 [14.6, 15.2] | -5.3 (-26.6%) |
| small | preview | 128.1 [119.9, 136.3] | 106.9 [78.8, 145.3] | -21.2 (-16.5%) | 20.1 [20.1, 20.1] | 15.4 [15.4, 15.9] | -4.7 (-23.4%) |
| small | released | 86.1 [85.7, 136.2] | 129.0 [95.1, 129.8] | +42.9 (+49.8%) | 15.9 [15.9, 16.3] | 11.7 [11.2, 11.7] | -4.2 (-26.4%) |
| small | closed | 86.1 [85.7, 136.2] | 95.1 [79.6, 129.0] | +9.0 (+10.5%) | 12.4 [12.4, 12.8] | 8.8 [8.3, 8.8] | -3.6 (-28.9%) |
| large | loaded | 307.1 [306.3, 308.2] | 299.5 [251.0, 299.6] | -7.6 (-2.5%) | 118.2 [118.2, 118.2] | 113.4 [113.4, 113.9] | -4.8 (-4.1%) |
| large | typed_split | 399.8 [399.3, 470.5] | 416.8 [408.7, 426.0] | +17.0 (+4.3%) | 118.7 [118.7, 119.6] | 114.0 [114.0, 114.5] | -4.7 (-4.0%) |
| large | typed_editor | 499.9 [436.3, 510.9] | 274.6 [274.2, 305.8] | -225.3 (-45.1%) | 118.5 [118.5, 119.4] | 99.1 [99.1, 99.6] | -19.4 (-16.4%) |
| large | preview | 498.0 [430.5, 513.0] | 270.6 [239.6, 279.5] | -227.4 (-45.7%) | 118.7 [118.7, 119.6] | 113.9 [113.9, 114.4] | -4.8 (-4.0%) |
| large | released | 158.9 [135.9, 182.7] | 230.3 [214.6, 235.2] | +71.4 (+44.9%) | 16.0 [16.0, 16.9] | 11.2 [11.2, 11.7] | -4.8 (-30.0%) |
| large | closed | 150.9 [135.9, 182.7] | 214.9 [214.6, 235.2] | +64.0 (+42.4%) | 12.4 [12.4, 13.3] | 8.8 [8.3, 8.8] | -3.6 (-28.9%) |
| images | image_0 | 244.6 [244.5, 294.8] | 270.8 [237.1, 287.4] | +26.2 (+10.7%) | 59.3 [59.3, 59.3] | 54.5 [54.5, 54.5] | -4.8 (-8.1%) |
| images | image_1 | 337.8 [337.8, 372.0] | 267.3 [236.6, 288.1] | -70.5 (-20.9%) | 105.1 [105.1, 105.1] | 54.6 [54.6, 55.0] | -50.5 (-48.0%) |
| images | image_2 | 430.4 [430.3, 480.7] | 287.0 [237.5, 288.1] | -143.4 (-33.3%) | 150.9 [150.9, 150.9] | 54.6 [54.6, 55.1] | -96.3 (-63.8%) |
| images | released | 430.4 [430.3, 480.7] | 240.2 [190.7, 241.3] | -190.2 (-44.2%) | 150.8 [150.8, 150.8] | 8.7 [8.7, 9.2] | -142.1 (-94.2%) |
| images | closed | 430.4 [430.3, 472.7] | 191.2 [190.7, 232.2] | -239.2 (-55.6%) | 149.3 [149.3, 149.3] | 7.9 [7.9, 8.3] | -141.4 (-94.7%) |
| blocks | list | 192.9 [140.0, 193.5] | 135.4 [85.2, 151.3] | -57.5 (-29.8%) | 82.7 [80.1, 82.7] | 18.4 [18.4, 18.8] | -64.3 (-77.8%) |
| blocks | table | 157.2 [152.8, 206.8] | 152.2 [101.2, 173.8] | -5.0 (-3.2%) | 89.2 [87.9, 89.2] | 46.8 [45.3, 47.3] | -42.4 (-47.5%) |
| blocks | code | 230.6 [180.8, 230.7] | 175.6 [175.3, 194.5] | -55.0 (-23.9%) | 113.0 [111.6, 113.0] | 70.5 [70.3, 71.0] | -42.5 (-37.6%) |
| blocks | released | 230.6 [180.5, 231.0] | 176.3 [175.7, 194.8] | -54.3 (-23.5%) | 104.2 [102.9, 104.3] | 61.8 [61.6, 62.3] | -42.4 (-40.7%) |
| blocks | closed | 172.1 [137.3, 179.4] | 115.2 [95.5, 144.5] | -56.9 (-33.1%) | 29.4 [29.4, 29.4] | 14.4 [14.2, 15.2] | -15.0 (-51.0%) |
| folder | loaded | 53.8 [53.6, 88.1] | 96.7 [47.2, 111.9] | +42.9 (+79.7%) | 13.2 [13.2, 13.2] | 9.0 [8.6, 9.2] | -4.2 (-31.6%) |
| folder | closed | 53.8 [53.6, 53.9] | 61.7 [47.2, 80.7] | +7.9 (+14.7%) | 11.7 [11.7, 11.7] | 8.0 [7.6, 8.2] | -3.7 (-31.5%) |

Live heap is DefaultMallocZone bytes allocated, not total process memory.
Per-run lifetime peak footprint and elapsed checkpoint times remain in each report.json/report.md.
Checkpoint times include harness delays and must not be interpreted as startup latency.

## Large-document release investigation

The original release samples show DefaultMallocZone dirty pages increasing
from a median 36.1 to 134.8 MiB, while allocated bytes decrease from 16.0 to
11.2 MiB. The zone's reported fragmentation increases from 20.1 to 123.0 MiB.
This locates most of the difference in allocator-retained pages rather than
additional live document objects; those pages still count toward footprint.
One baseline run also retains 133.1 MiB dirty, so the effect is not exclusive
to the optimized binary.

An isolated diagnostic used the unchanged optimized binary in six fresh
background processes. Both groups loaded the same small probe library and
worker thread. After clearing the document and waiting five seconds, three
processes called `malloc_zone_pressure_relief(NULL, 0)` and three ran a no-op
control, in control/treatment/treatment/control/control/treatment order.
All three calls returned zero bytes, took 0.003–0.006 ms, and changed footprint
by at most 0.1 MiB; a control also changed by 0.1 MiB. No effective reclamation
was demonstrated, so this call was **not added to Neon**. Apple's
[public API contract](https://github.com/apple-oss-distributions/libmalloc/blob/c49dafa25f1efe8607701ae6014a663ad2ee437f/include/malloc/malloc.h#L516-L522)
describes best-effort reclamation, not a guarantee.

The probe skips intermediate `vmmap` pauses and injects a worker, so its
132.2–248.9 MiB release range is diagnostic only; it does not replace the
matrix above. Its source, runner, raw samples and results are retained locally
under `output/memory/relief-probe/`.

A separate confirmation reran the **unmodified official runner and saved
binaries**, without the probe library, in six pairs. Execution order was
before/after, after/before, repeated three times, with only one benchmark
process running at once. All 72 additional checkpoints were inactive, at the
same scale, with matching fixtures. No application source changed between the
original matrix and this confirmation. The original matrix is not overwritten
or pooled with these results.

| Confirmation metric | Before MiB, median [min, max] | After MiB, median [min, max] |
| --- | ---: | ---: |
| Whole-process peak footprint | 550.9 [450.7, 576.8] | 429.4 [411.4, 461.7] |
| Editor-only footprint | 538.2 [413.8, 554.6] | 254.5 [225.5, 305.0] |
| Released footprint | 215.4 [97.9, 262.3] | 188.7 [80.7, 255.3] |
| Closed footprint | 191.8 [98.3, 262.3] | 188.9 [70.7, 212.5] |
| Released live heap | 17.0 [16.0, 17.7] | 11.8 [11.2, 12.3] |

The lower working-set and peak costs reproduce. The release/closed footprint
differences do not establish a consistent benefit or regression: the direction
changes between batches and the confirmation ranges substantially overlap.
Keep the ownership, lazy-loading and virtualization improvements, without
claiming that freeing a document guarantees an immediate physical-footprint
reduction. Neither changing the allocator nor retaining hidden preview data
is justified by these measurements. The unused pressure-relief call is omitted.

The confirmation's [full comparison](../output/memory/after-large-confirmation/compare-before-large-confirmation.md),
[before report](../output/memory/before-large-confirmation/report.json), and
[after report](../output/memory/after-large-confirmation/report.json) retain the
individual run labels and provenance. Its orchestration script is
`output/memory/large-confirmation/run.py`; it calls the existing runner for
each single-process run and combines the unchanged samples by version.

## Validation

- `cargo check` passed for the ordinary build.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passed.
- `cargo test --workspace --all-features`: 30 existing tests passed. The four preview tests were rerun after the final scroll fixes and passed.
- `cargo fmt --all --check` and `git diff --check` passed.
- The runner passed fixture/PNG validation, vmmap parser checks, comparison calculations and rejection checks for active windows, mismatched scales and old report formats.
- Independent source review covered image ownership, task lifetimes, list chunking, scroll restoration and baseline provenance. It found three scroll issues, which were fixed and reviewed again.
- Foreground manual interaction and latency benchmarks were not rerun, to respect the request not to occupy the foreground. Background workloads verify document loading, typed-byte counts, layout changes and release checkpoints; they do not replace visual QA of typography controls.

## Artifacts and provenance

The following measurement artifacts are local and gitignored; the runner
recreates them. The matrix above is retained in the repository.

- [Before report and raw samples](../output/memory/before-background/report.json)
- [After report and raw samples](../output/memory/after-background/report.json)
- [Machine-readable comparison](../output/memory/after-background/compare-before-background.json)

- Before binary SHA-256: `925de222a1ae8e62b04e83216a996fc59d91ac5e403efdcfab354afa5ac97882`
- After binary SHA-256: `a0226f9701c7ed8f33b770a0533b6d6a10400e58be2521d986cd5062c798cda8`
- Runner SHA-256: `d2505ce402217e0177a619b712512d2d7d0bdb39c4d51a00ff00b8ef66191368`
- Baseline patch SHA-256: `4032dde84ebbb39454708069297fa22caa89b2f54058f0030109ac822ff8997c`
- Fixture manifest SHA-256: `e5a118aa46c8b6720fc2b70c44ec8e3cab18dd4fcad12bea767ec7e3cd8a47eb`
