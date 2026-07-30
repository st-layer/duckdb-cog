# Changelog

All notable changes to duckdb-cog. Format follows
[Keep a Changelog](https://keepachangelog.com/); versions follow
[SemVer](https://semver.org/) (0.x: minor bumps may signal breaking changes).
Deployment to `INSTALL cog FROM community` lags tags by one
community-extensions ref-bump PR — the "Deployed" note per release tracks that.

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
