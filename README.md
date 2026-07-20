# duckdb-cog

**GDAL-free Cloud-Optimized GeoTIFF (COG) reader for DuckDB.**

`duckdb-cog` exposes COG rasters as SQL tables — in place, over HTTP/S3 range
reads, with no re-encoding, no reprojection, and no GDAL/PROJ/GEOS anywhere in
the read path. TIFF decoding and fetching are delegated to
[async-tiff](https://github.com/developmentseed/async-tiff); the extension adds
the tile-table model, the SQL surface, and lazy windowed IO on top.

```sql
LOAD cog;

-- List the tile grid of a remote COG (metadata-only range reads — a couple of
-- small GETs, never the pixels):
SELECT level, tile_x, tile_y, bbox, crs
FROM read_cog('https://example.com/sentinel2_B04.tif');

-- Walk a STAC document and read COG metadata straight from its assets:
SELECT s.item_id, RS_Width(s.href), RS_SRID(s.href)
FROM read_stac('https://example.com/catalog/items.json') s
WHERE s.media_type LIKE '%geotiff%';

-- Search a live STAC API (POST /search, pagination followed automatically):
SELECT item_id, datetime, href
FROM read_stac_search('https://earth-search.aws.element84.com/v1/search',
                      collections := ['sentinel-2-l2a'],
                      bbox := [126.0, 37.0, 127.0, 38.0],
                      datetime := '2026-07-01T00:00:00Z/2026-07-12T23:59:59Z');

-- Sedona-style metadata accessors:
SELECT RS_Width(f), RS_Height(f), RS_NumBands(f), RS_SRID(f), RS_MetaData(f)
FROM (SELECT 's3://my-bucket/ortho.tif' AS f);
```

## Status

Phase 1–2 engineering complete — metadata, pixel access, STAC (documents and
API search) are functional and oracle-tested; release gates (naming,
community-extensions registration) are pending.

| Capability | State |
| -- | -- |
| `read_cog(path)` tile-grid listing (levels, overviews, per-tile bbox, CRS) | ✅ |
| Local file, `http(s)://`, `s3://` sources (object_store; env credentials) | ✅ |
| `RS_*` metadata accessors (Width/Height/NumBands/Scale/Skew/UpperLeft/SRID/BandNoDataValue/MetaData/GeoReference) | ✅ |
| Lazy IO contract (metadata listing ≤ a few range GETs, pixels untouched) | ✅ |
| `RS_Value(path, x, y[, band])` pixel access (level 0, no interpolation, rasterio-verified) | ✅ |
| `RS_Values` batch pixel access (per-tile single fetch+decode) | ✅ |
| `RS_NormalizedDifference` (point-form band math, NDVI-style) | ✅ |
| `RS_ZonalStats` (bbox zones, count/sum/mean/min/max) | ✅ |
| `RS_BandAsArray` (full band or bbox window, row-major) | ✅ |
| `RS_BandStats` — GDAL_METADATA statistics without decoding | ✅ |
| `read_stac(url)` — STAC items to (item, asset) rows incl. `raster:bands` statistics (decode-free aggregation) | ✅ |
| `read_stac_search(url, collections/bbox/datetime/page_size, max_rows)` — STAC API POST /search with `rel=next` pagination (row cap 1,000 by default) | ✅ |

## Performance

Apple Silicon (macOS arm64), release build, 2026-07-12. Local workloads run
against a 4096² uint16 DEFLATE COG with 256 px tiles (median of 5, cold =
fresh connection); remote against a real Sentinel-2 B04 scene (10980²) over
`https://`.

| workload | time |
| -- | -- |
| cold metadata → first answer (local) | 3.8 ms |
| tile inventory, all levels (warm) | 0.5 ms |
| 1,000-point sampling — `RS_Values` batch / `RS_Value` per-row | 8.0 ms / 21.3 ms |
| zonal mean over a 1024² window | 2.9 ms |
| remote cold metadata (real Sentinel-2 B04) | 0.84 s |
| remote repeat metadata (process-wide cache) | 0.4 ms |

Ingest cost is zero by design — the file is queryable in place. Cross-engine
context (PostGIS raster, GDAL-based `raster` extension, raquet) and the I/O
path study live in
[`docs/benchmarks/2026-07-12-comparative.md`](docs/benchmarks/2026-07-12-comparative.md)
and
[`docs/benchmarks/2026-07-12-io-path-ab.md`](docs/benchmarks/2026-07-12-io-path-ab.md);
reproduce with `scripts/bench_compare.py` and `scripts/bench_io_compare.py`.

## SQL surface

### `read_cog(path VARCHAR, bbox := [xmin, ymin, xmax, ymax])` → table

The optional `bbox` named parameter (native-CRS coordinates) prunes the tile
list to tiles intersecting the box.

One row per physical tile across all resolution levels (level 0 = full
resolution, 1.. = embedded overviews):

| column | type | meaning |
| -- | -- | -- |
| `id` | BIGINT | packed tile key (level 8b \| x 24b \| y 24b) |
| `level` | INTEGER | 0 = base, 1.. = overviews |
| `tile_x`, `tile_y` | INTEGER | tile grid coordinates |
| `cols`, `rows` | INTEGER | physical tile size (not edge-clipped) |
| `bbox` | STRUCT(xmin,ymin,xmax,ymax DOUBLE) | data extent in native CRS, edge tiles clipped; NULL if not georeferenced |
| `crs` | VARCHAR | `"EPSG:NNNN"`; NULL if unknown |

### `RS_*` scalar functions

Named and shaped after the [Apache Sedona RS_ catalog](https://sedona.apache.org/)
(a design reference, not a compatibility contract — deviations are documented in
[`docs/sedona-semantics.md`](docs/sedona-semantics.md)). The raster argument is
the COG path/URL. Conventions: 1-based band indexes; out-of-range band, missing
nodata, or NULL input → NULL; GDAL-order geotransform; `RS_SRID` returns 0 when
unknown.

`RS_Width` · `RS_Height` · `RS_NumBands` · `RS_ScaleX/Y` · `RS_SkewX/Y` ·
`RS_UpperLeftX/Y` · `RS_SRID` · `RS_BandNoDataValue(path[, band])` ·
`RS_MetaData` (named STRUCT) · `RS_GeoReference` (GDAL 6-line text) ·
`RS_Value(path, x, y[, band])` · `RS_Values(path, xs[], ys[][, band])` ·
`RS_NormalizedDifference(path, x, y, band1, band2)` (point-form, NDVI-style) ·
`RS_ZonalStats(path, bbox[], band, stat)` (stat: `count`/`sum`/`mean`/`min`/`max`) ·
`RS_BandAsArray(path, band[, bbox[]])` (row-major `DOUBLE[]`) ·
`RS_BandStats(path[, band])` (GDAL_METADATA statistics, decode-free) ·
`RS_WorldToRasterCoord` / `RS_RasterToWorldCoord` (1-based)

### Remote sources

URLs are dispatched to [object_store](https://docs.rs/object_store): `https://`
works out of the box, plain `http://` is enabled automatically, `s3://` reads
credentials from the standard `AWS_*` environment variables (for plain-http S3
endpoints such as MinIO, set `AWS_ALLOW_HTTP=true`).

**Public S3 buckets** (e.g. the AWS Sentinel-2 open data): virtual-host style
`https://<bucket>.s3.<region>.amazonaws.com/...` URLs are recognized as S3 —
without credentials the client probes the EC2 metadata service and hangs
through retries. Set `AWS_SKIP_SIGNATURE=true` for anonymous access:

```sh
AWS_SKIP_SIGNATURE=true duckdb -unsigned -c "
  LOAD 'cog.duckdb_extension';
  SELECT RS_Width(f), RS_SRID(f) FROM (SELECT
    'https://sentinel-cogs.s3.us-west-2.amazonaws.com/.../B04.tif' AS f);"
```

**Remote metadata cache**: opened remote COGs (metadata/IFDs and the reader)
are cached process-wide for 60 seconds, so repeated queries against the same
URL skip the cold metadata round-trips. The trade-off is staleness: if the
object changes on the server within the TTL, you keep reading the old
metadata (tile data is still fetched per request). Tune or disable with
`COG_REMOTE_CACHE_TTL_S` (seconds, `0` disables). Local paths are never
cached.

## Design invariants

These are enforced by tests and hooks, not just convention (see
[`docs/RFC-001-rev3.md`](docs/RFC-001-rev3.md)):

- **No GDAL/PROJ/GEOS in the read path.** GDAL (via rasterio) exists only as the
  *test oracle*: every fixture property and pixel-facing result is
  cross-checked against rasterio in CI.
- **No TIFF parsing of our own** — decode/fetch delegated to async-tiff behind a
  single reader boundary trait.
- **No reprojection at read time.** Pixels and coordinates stay in the native CRS.
- **Lazy IO is a tested contract**: an IO-counting test pins that listing a
  COG's tile grid costs a constant number of range reads and never touches
  pixel data.
- The engine crate stays `wasm32-unknown-unknown`-compilable.

## Installation

Once the community-extensions registration is accepted (submission pending
review), installation is:

```sql
INSTALL cog FROM community;
LOAD cog;
```

Until then, build locally (below) and load the unsigned binary.

## Building

Requires Rust (≥ 1.87 via rustup), [just](https://github.com/casey/just),
[uv](https://docs.astral.sh/uv/), and GNU make.

```sh
just setup      # once: extension-ci-tools submodule + python venv
just ext        # build the extension (debug)
just check      # full local gate: fmt, clippy, unit+integration tests, rasterio oracle
just ext-test   # end-to-end sqllogictests (includes an HTTP range-server round trip)
```

Load the built extension in DuckDB (unsigned, for local development):

```sh
duckdb -unsigned -c "LOAD 'build/debug/cog.duckdb_extension'; SELECT * FROM cog_version();"
```

## Development

Agent-driven TDD with an oracle-backed harness — the full process is documented
in [`docs/HARNESS.md`](docs/HARNESS.md) and [`AGENTS.md`](AGENTS.md). The short
version: contracts are written as failing tests first (sqllogictest for the SQL
surface, pytest+rasterio as the accuracy oracle, proptest for arithmetic
invariants, an IO-counting source for fetch efficiency); `just check` is the
only definition of done; fixtures are byte-deterministic and hash-locked.

## License

MIT
