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
