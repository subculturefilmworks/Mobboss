# NAS timing follow-up

The completed release CLI validation found all 344 files under BD Demo Reel:

- Deep: 31.530 s, 344 headers read.
- Warm quick: 0.077 s, **0 headers read**, 344 cached.
- A subsequent warm quick run during a separate NAS I/O probe took 2.952 s, also **0 headers read**. NAS load/cache state clearly affects elapsed time.

For a rough split, the read-only `profile_headers` example wraps the actual core parser's file I/O and counts physical bytes read. Run it with:

```sh
cargo run --release -p mobboss-core --example profile_headers -- /path/to/media
```

A follow-up run, after the sequential read probe had warmed the cache, measured:

| Work | Time |
| --- | ---: |
| File opens | 3.511 s |
| Header reads: 57,111,280 bytes (54.47 MiB) | 3.618 s |
| Seeks / position queries | 0.044 s |
| File closes | 0.001 s |
| Other work, including parsing and timing overhead | 0.913 s |
| Total header pass | 8.086 s |

This profile excludes discovery, PMR/MDB parsing, file stat comparisons and SQLite writes. It uses the same bounded header parser, with no essence reads. It is a separate run, not an attribution of the earlier 31.530-second CLI result.

An earlier single-threaded raw probe opened each file and read its first 160 KiB (within this sample's reserved header region): opens 6.564 s, reads of 56,360,960 bytes 33.330 s, closes 0.004 s. That probe and the parser profile had different cache conditions and read patterns; their times should not be subtracted from each other or from the CLI timing.

Both measurements suggest I/O deserves attention before parser CPU tuning. Bounded parallel opens/reads are worth benchmarking in a later phase, but these results do not establish whether latency, throughput or server caching dominates cold scans. No parallel scanning was introduced here.

Three complete, unmodified exported rows are in [sample-rows.json](sample-rows.json). Their durations convert as follows:

| Clip | Codec | Descriptor rate | Duration units | Seconds |
| --- | --- | --- | ---: | ---: |
| 20151911_DJI_0003.new.01 | DNxHR (SQ UL) | 24000/1001 | 198 | 8.25825 |
| /01.new.01 | PCM | 48000/1 | 690690 | 14.389375 |
| Untitled.new.01 | DNxUncompressed alpha | 24000/1001 | 1 | 0.0417083 |

The audio duration is in samples; the video durations are in frames. Duration seconds = units × rate denominator / rate numerator.

## Phase 2: progressive inventory and parallel headers (2026-11-23)

`benchmark_scan` runs actual Quick scans against the read-only sample with a fresh SQLite index in `/tmp` for each run. Every run identified and enriched all **344 files**, all project **BD Demo Reel**, and read **344 headers**. The first callback containing rows was the inventory callback, before any header jobs started. It included all 344 project names and PMR file UIDs; codec/rate/duration followed as verified header results.

```sh
cargo run --release -p mobboss-core --example benchmark_scan -- <sample-folder>
cargo run --release -p mobboss-core --example benchmark_scan -- <sample-folder> 16,8,4,1
```

| Workers | First PMR inventory, pass A | Fully enriched, pass A | First PMR inventory, pass B | Fully enriched, pass B |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 0.086 s | 30.904 s | 0.051 s | 5.179 s |
| 4 | 0.062 s | 26.945 s | 0.093 s | 4.536 s |
| 8 | 0.053 s | 12.732 s | 0.045 s | 4.498 s |
| 16 | 0.052 s | 6.090 s | 0.147 s | 13.033 s |

Pass A ran in ascending order (1/4/8/16); pass B later ran in descending order (16/8/4/1). **Cold index does not mean cold NAS/OS cache.** Neither pass flushed client/server caches, and their state cannot be established. Earlier parser probes and tests had accessed the sample; sample integration validation may also have overlapped early pass A. Pass B had no intentional concurrent sample scans. NAS/server load was uncontrolled. The slower first run in each order and faster later runs demonstrate why pass A alone cannot establish a 5× concurrency speedup. These are honest end-to-end observations, not a controlled throughput experiment. The default remains a conservative 8 simultaneous reads; 16 is available for measurement on the user's storage.

Each header reader retains its 512 KiB maximum I/O budget and never reads essence. The worker count is limited to 1–64, with a bounded completion queue (2× workers). Cancellation is checked while discovering/planning and between header jobs; in-flight OS reads must finish. Cancellation does not commit the current root's provisional data. UI events are coalesced on a 100 ms timer to avoid a full table update for every header. The measured first-result times above refer to the core callback, excluding UI startup/rendering and this coalescing interval.

The old Phase 1 profile above remains historical context; Phase 2 implements the previously deferred bounded parallel reads.
