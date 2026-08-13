# Changelog

All notable changes to duckdb-cog. Format follows
[Keep a Changelog](https://keepachangelog.com/); versions follow
[SemVer](https://semver.org/) (0.x: minor bumps may signal breaking changes).
Deployment to `INSTALL cog FROM community` lags tags by one
community-extensions ref-bump PR — the "Deployed" note per release tracks that.

## [0.4.0] — 2026-08-13

Two threads: the browser sidecar (RFC Decision B) became real, and the remote
read path got fast enough to outrun a sequential GDAL loop.

### Added
- **Browser sidecar `engine-wasm`** (#66 → #67/#68/#69/#70): wasm-bindgen
  bindings over the shared engine — `cogMeta`/`zonalStats` (bytes and URL via
  a fetch-backed ByteSource), time-series `zonalStatsBatch` and zone-axis
  `zonalStatsBatchZones`, a URL→reader registry with a browser-tuned tile
  cache (64 MB default, `configureTileCache`/`tileCacheStats`). Distributed
  as a `.tgz` attached to GitHub Releases on publish (WasmArtifact workflow),
  not npm. Headless-Chrome parity suites pin bit-exact agreement with the
  native goldens.
- **AOI raster windows in the browser** (#76): `bandWindowPolygon`/`bandWindow`
  return the polygon's pixels (`Float64Array` + width/height/placement bbox) —
  the rendered raster is provably the same pixel set the zonal statistic saw;
  no external tile service, no whole-scene preview.
- **Overview fast mode** (#71): optional `max_pixels` on `RS_ZonalStats`
  (all three overloads) and `maxPixels` on the wasm zonal functions — picks
  the coarsest-sufficient overview level from a pixel budget. Explicitly
  approximate: count/sum are level-0 rescaled estimates, min/max damp, mean
  drifts slightly; `0`/omitted = exact, byte-identical to before.

### Changed (performance — 22-read Sentinel-2 time series, Korea → us-west-2)
- **Per-origin HTTP client sharing** (#72 → #73): every remote open paid
  DNS+TCP+TLS on a fresh connection pool; 51.7 s → ~30 s.
- **Concurrent chunk rows in `RS_ZonalStats`** (#74 → #75): rows run
  concurrently (order-preserving; `COG_FETCH_CONCURRENCY`, default 16);
  → **9.6–13.0 s**, past the 24–25 s sequential GDAL/rasterio baseline.
  Values identical to the digit throughout.

### Fixed
- Tile-cache keys are now level-scoped (#78) — a fast-mode overview tile could
  silently poison the exact level-0 read of the same tile coordinate.
- TileCache contention no longer panics on single-threaded wasm (#78) —
  Condvar wait replaced with duplicate-fetch settlement there.

Deployed: pending — community-extensions ref-bump PR follows the tag.

## [0.3.0] — 2026-07-30

Driven by production field reports from a season-scale parcel-statistics
workload (2,511 parcels × 28 dates, remote Sentinel-2).

### Added
- **Process-wide tile-data cache** (#56): decoded tiles in a byte-bounded LRU
  (default 256 MB, `COG_TILE_CACHE_MB`, `0` disables), single-flight on cold
  tiles, invalidation scoped to the reader cache's lifetime. User-measured
  526 s → 38.8 s per scene; fetch counts drop from per-zone to per-tile.
- **Batch zonal** (#62): `RS_ZonalStats(path, VARCHAR[] wkt, band, stat) →
  DOUBLE[]` — the tile union across all zones is fetched once per call,
  amortizing per-call overhead for many-small-zones workloads.
- **`cog_cache_stats()`** (#63): hits/misses/evictions/bytes/max_bytes —
  cache thrashing becomes a one-query diagnosis.
- `COG_IO_THREADS` (#57): remote IO runtime sized to CPU count (capped at 8)
  instead of a single worker thread.

### Changed
- `read_stac_search` now **errors** when the default 1,000-row cap would drop
  data (#58) — silent truncation was a data-loss bug; explicit `max_rows`
  opts into truncation and lifts the ceiling.

### Docs
- Access-locality guide (group zone calls by scene: 74 min vs 17 min measured),
  tile-cache benchmark (`docs/benchmarks/2026-07-29-tile-cache.md`),
  stale-pixels staleness warning.

Deployed: community-extensions ref update via duckdb/community-extensions#2400.

## [0.2.0] — 2026-07-29

### Added
- **WKT polygon zones** (#49, #54): `RS_ZonalStats` and `RS_BandAsArray`
  accept `POLYGON`/`MULTIPOLYGON` WKT (holes included) — pure-Rust
  point-in-polygon, no GEOS link; pixel-centre inclusion shared between both
  functions so the same zone sees the same pixel set.

### Changed
- Target DuckDB v1.5.5 (#46) — extension stamp + test toolchain moved in
  lockstep.

Deployed: rolled into the v0.3.0 community-extensions update (never deployed
standalone).

## [0.1.0] — 2026-07-20

Initial release, registered to duckdb/community-extensions (#2274; deploy
pipeline fix in #2313).

- `read_cog(path[, bbox])` tile-grid listing (levels, overviews, per-tile
  bbox, CRS) over local files, `http(s)://`, and `s3://` (object_store)
- Sedona-shaped `RS_*` catalog: metadata accessors, `RS_Value`/`RS_Values`,
  `RS_NormalizedDifference`, `RS_ZonalStats` (bbox), `RS_BandAsArray`,
  `RS_BandStats`, coordinate transforms
- STAC: `read_stac(url)` document walker and `read_stac_search(url, ...)`
  (POST /search with `rel=next` pagination)
- Process-wide remote reader cache (`COG_REMOTE_CACHE_TTL_S`), lazy-IO
  contracts, rasterio oracle parity in CI, WASM build

[0.3.0]: https://github.com/st-layer/duckdb-cog/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/st-layer/duckdb-cog/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/st-layer/duckdb-cog/releases/tag/v0.1.0
