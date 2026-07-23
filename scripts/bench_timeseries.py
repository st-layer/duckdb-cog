"""시계열 추출 벤치 — cog vs duckdb-raster vs PostGIS vs raquet (셋업 완료 전제).

docs/benchmarks/2026-07-23-timeseries.md 의 수치 산출. 워크로드: 고정 창
(1024² px, UTM [305120, 3984640, 315360, 3994880]) zonal mean × 12씬 시계열.
"모든 후보가 이미 셋업된 정상 상태(warm)"를 전제로 순수 추출 속도만 잰다 —
셋업 비용(임포트/재인코딩)은 보고서에 별도 표기.

사전 조건:
  cog     : `make release` (rustup PATH) — build/release/cog.duckdb_extension
  fixtures: `python scripts/bench_timeseries.py gen` — /tmp/cogbench-ts/ 에 12씬 생성
  raquet  : 씬별 `uv run --with raquet-io --with "gdal==3.11.0" geotiff2raquet
            --compression gzip <in.tif> <out.parquet>` (pip gdal 은 시스템 libgdal
            버전과 일치시켜야 빌드된다 — brew gdal 3.11 기준. 실측 변환 17s/12씬)
  PostGIS : docker 필요 (실측 임포트 37s/12씬):
    docker run -d --name pg-ts -e POSTGRES_PASSWORD=bench postgis/postgis:16-3.4
    docker exec pg-ts bash -c "apt-get update -qq && apt-get install -y -qq postgis"
    docker exec pg-ts psql -U postgres -c "CREATE EXTENSION postgis; CREATE EXTENSION postgis_raster;"
    # 씬별: raster2pgsql -c/-a -F -s 32652 -t 256x256 → public.ts_rast (+ 파일명 UPDATE)
    # 마지막: CREATE INDEX ON ts_rast USING gist (ST_ConvexHull(rast)); ANALYZE ts_rast;

주의(정합): PostGIS ST_Clip 은 경계 '교차' 픽셀 포함(실측 1026²), 우리는 픽셀
중심 포함(정확히 1024²) — mean 이 ~0.2% 다른 것은 규약 차이다. raquet 은
WebMercator 재인코딩으로 픽셀값 자체가 변형(리샘플링)된다. RT_* 는 fill 값을
반환해 정합 실패(2026-07-12 비교벤치와 동일) — 시간은 참고치로만.
"""
import glob
import re
import statistics
import subprocess
import sys
import time

import duckdb

DIR = "/tmp/cogbench-ts"
W = (305120.0, 3984640.0, 315360.0, 3994880.0)
LL = (126.83573, 35.98675, 126.95178, 36.08101)  # W 를 EPSG:4326 으로 (사전 계산)
EXT_COG = "build/release/cog.duckdb_extension"


def gen_fixtures():
    """12씬 × 2048² u16 DEFLATE COG (256px 타일, EPSG:32652, seed 42 결정적)."""
    import numpy as np
    import rasterio
    from rasterio.transform import from_origin

    rng = np.random.default_rng(42)
    base = rng.integers(1000, 3000, (2048, 2048)).astype(np.float64)
    xx, yy = np.meshgrid(np.linspace(0, 4 * np.pi, 2048), np.linspace(0, 4 * np.pi, 2048))
    for i in range(12):
        date = f"2026{6 + i // 6:02d}{(i * 5) % 30 + 1:02d}"
        season = 1500 * (1 + np.sin(xx + i * 0.5) * np.cos(yy - i * 0.3))
        data = np.clip(base + season + rng.normal(0, 50, base.shape), 0, 65535).astype(np.uint16)
        with rasterio.open(
            f"{DIR}/scene_{i:02d}_{date}.tif", "w",
            driver="GTiff", height=2048, width=2048, count=1, dtype="uint16",
            crs="EPSG:32652", transform=from_origin(300000, 4000000, 10, 10),
            tiled=True, blockxsize=256, blockysize=256, compress="deflate",
        ) as d:
            d.write(data, 1)
            d.build_overviews([2, 4, 8])
    print("12 scenes written to", DIR)


def med(fn, n=5):
    ts = []
    for _ in range(n):
        t0 = time.perf_counter()
        fn()
        ts.append((time.perf_counter() - t0) * 1000)
    return statistics.median(ts)


def main():
    scenes = sorted(glob.glob(f"{DIR}/scene_*.tif"))
    raqs = sorted(glob.glob(f"{DIR}/scene_*.parquet"))
    dates = [re.search(r"_(\d{8})", f).group(1) for f in scenes]
    R = {}

    # ── cog: RS_ZonalStats 시계열 (SQL 한 방) ──
    c = duckdb.connect(config={"allow_unsigned_extensions": True})
    c.execute(f"LOAD '{EXT_COG}'")
    c.execute("CREATE TABLE scenes(date VARCHAR, path VARCHAR)")
    c.executemany("INSERT INTO scenes VALUES (?, ?)", list(zip(dates, scenes)))
    q = f"SELECT date, RS_ZonalStats(path, [{W[0]}, {W[1]}, {W[2]}, {W[3]}], 1, 'mean') FROM scenes ORDER BY date"
    c.execute(q).fetchall()  # warm
    R["cog"] = med(lambda: c.execute(q).fetchall())

    # ── duckdb-raster: RT_* (참고치 — fill 반환, 정합 실패) ──
    cr = duckdb.connect()
    try:
        cr.execute("INSTALL raster; LOAD raster; INSTALL spatial; LOAD spatial;")
        wkt = f"POLYGON(({W[0]} {W[1]},{W[2]} {W[1]},{W[2]} {W[3]},{W[0]} {W[3]},{W[0]} {W[1]}))"

        def rt_series():
            for f in scenes:
                cr.execute(
                    f"""SELECT RT_CubeStats_Agg(RT_CubeClip(databand_1, 0, 0, metadata,
                          ST_GeomFromText('{wkt}'), 0.0), 0)
                        FROM RT_Read('{f}')
                        WHERE st_intersects_extent(geometry, ST_GeomFromText('{wkt}'))"""
                ).fetchall()

        rt_series()
        R["duckdb-raster (정합실패·참고치)"] = med(rt_series, n=3)
    except Exception as e:  # noqa: BLE001 — 미가용 자체가 결과
        R["duckdb-raster"] = f"FAIL: {str(e)[:70]}"

    # ── raquet: quadbin 블록 정렬 창 (재인코딩 데이터) ──
    if raqs:
        cq = duckdb.connect()
        cq.execute("INSTALL raquet; LOAD raquet; INSTALL spatial; LOAD spatial;")

        def raquet_series():
            for f in raqs:
                cq.execute(
                    f"""SELECT avg(v) FROM (
                          SELECT unnest(raquet_decode_band(band_1, 'uint16', 256, 256, 'gzip')) AS v
                          FROM read_raquet('{f}')
                          WHERE block IN (SELECT unnest(QUADBIN_POLYFILL(
                                ST_MakeEnvelope({LL[0]}, {LL[1]}, {LL[2]}, {LL[3]}), 14))))"""
                ).fetchone()

        raquet_series()
        R["raquet (재인코딩·블록정렬)"] = med(raquet_series)

    # ── PostGIS: 서버측 \timing (docker exec 왕복 제외) ──
    pgq = f"""SELECT filename, (ST_SummaryStatsAgg(ST_Clip(rast,
        ST_MakeEnvelope({W[0]}, {W[1]}, {W[2]}, {W[3]}, 32652)), 1, true)).mean
        FROM ts_rast WHERE ST_Intersects(ST_ConvexHull(rast),
        ST_MakeEnvelope({W[0]}, {W[1]}, {W[2]}, {W[3]}, 32652))
        GROUP BY filename ORDER BY filename;"""

    def pg_ms():
        out = subprocess.run(
            ["docker", "exec", "pg-ts", "psql", "-U", "postgres", "-c", "\\timing", "-c", pgq],
            capture_output=True, text=True,
        )
        m = re.search(r"Time: ([\d.]+) ms", out.stdout)
        if not m:
            raise RuntimeError(out.stderr[:100])
        return float(m.group(1))

    try:
        pg_ms()
        R["postgis (서버측)"] = statistics.median([pg_ms() for _ in range(5)])
    except Exception as e:  # noqa: BLE001
        R["postgis"] = f"SKIP: {str(e)[:60]}"

    print("\n=== 12씬 zonal-mean 시계열 추출 (warm, median) ===")
    for k, v in R.items():
        print(f"{k:34s} {v if isinstance(v, str) else f'{v:8.1f} ms'}")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "gen":
        gen_fixtures()
    else:
        main()
