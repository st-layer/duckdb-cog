"""비교 벤치마크 재현 스크립트 — docs/benchmarks/2026-07-12-comparative.md 의 수치 산출.

대상: cog(우리, release 빌드) vs duckdb-raster(community, GDAL) [+ PostGIS 는 docker,
보고서의 psql 스크립트 참조]. 데이터: /tmp/cogbench/bench_4096.tif (4096², u16,
DEFLATE, 256px 타일, seed 42 — 아래 gen 섹션 참조).

주의: RT_* 포인트/zonal 은 값 정합 검증에 실패해 (문서 부재로 호출 형태 확정 불가)
보고서에서 참고치로만 취급한다. 재현 전 `make release` (rustup PATH) 필요.
"""
import statistics, time
import duckdb

F = "/tmp/cogbench/bench_4096.tif"
N = 5
EXT_COG = "build/release/cog.duckdb_extension"

def timed(fn, n=N):
    ts = []
    for _ in range(n):
        t0 = time.perf_counter(); fn(); ts.append(time.perf_counter() - t0)
    return statistics.median(ts)

def con_cog():
    c = duckdb.connect(config={"allow_unsigned_extensions": True})
    c.execute(f"LOAD '{EXT_COG}'"); return c

def con_rt():
    c = duckdb.connect(); c.execute("LOAD raster"); return c

# 동일 시드 1000 포인트 (extent 안)
import random
rng = random.Random(42)
PTS = [(300000.0 + rng.uniform(1, 40959), 4000000.0 - rng.uniform(1, 40959)) for _ in range(1000)]
XS = [p[0] for p in PTS]; YS = [p[1] for p in PTS]

R = {}

# W4 cold metadata (fresh connection per run)
R["W4_cold_meta_cog"] = timed(lambda: con_cog().execute(f"SELECT RS_Width('{F}'), RS_SRID('{F}')").fetchall())
R["W4_cold_meta_rt"]  = timed(lambda: con_rt().execute(f"SELECT metadata FROM RT_Read('{F}') LIMIT 1").fetchall())

# W1 tile inventory (warm connection)
cc, cr = con_cog(), con_rt()
R["W1_tiles_cog"] = timed(lambda: cc.execute(f"SELECT level, count(*) FROM read_cog('{F}') GROUP BY level").fetchall())
R["W1_tiles_rt"]  = timed(lambda: cr.execute(f"SELECT level, count(*) FROM RT_Read('{F}') GROUP BY level").fetchall())

# W2 1000-point sampling
cc.execute("CREATE OR REPLACE TABLE pts(i INT, x DOUBLE, y DOUBLE)")
cc.executemany("INSERT INTO pts VALUES (?, ?, ?)", [(i, x, y) for i, (x, y) in enumerate(PTS)])
cr.execute("CREATE OR REPLACE TABLE pts(i INT, x DOUBLE, y DOUBLE)")
cr.executemany("INSERT INTO pts VALUES (?, ?, ?)", [(i, x, y) for i, (x, y) in enumerate(PTS)])
R["W2_1kpts_cog_values"] = timed(lambda: cc.execute(f"SELECT RS_Values('{F}', ?, ?)", [XS, YS]).fetchall())
R["W2_1kpts_cog_scalar"] = timed(lambda: cc.execute(f"SELECT RS_Value('{F}', x, y) FROM pts").fetchall())
def rt_pts():
    cr.execute(f"""
        SELECT p.i, RT_CoordValue_Agg(r.databand_1, 1, p.x, p.y, r.cols, r.rows, r.metadata, 0.0)
        FROM RT_Read('{F}') r, pts p GROUP BY p.i
    """).fetchall()
R["W2_1kpts_rt_agg"] = timed(rt_pts, n=3)

# W3 zonal mean (중앙 1024x1024 픽셀 창)
ENV = (310000.0, 3970000.0, 320240.0, 3980240.0)  # xmin,ymin,xmax,ymax
R["W3_zonal_cog"] = timed(lambda: cc.execute(
    f"SELECT RS_ZonalStats('{F}', [?, ?, ?, ?], 1, 'mean')", list(ENV)).fetchall())
wkt = f"POLYGON(({ENV[0]} {ENV[1]},{ENV[2]} {ENV[1]},{ENV[2]} {ENV[3]},{ENV[0]} {ENV[3]},{ENV[0]} {ENV[1]}))"
def rt_zonal():
    cr.execute(f"""
        SELECT RT_CubeStats_Agg(RT_CubeClip(databand_1, 1, 0, metadata, ST_GeomFromText('{wkt}'), 0.0), 1)
        FROM RT_Read('{F}') WHERE st_intersects_extent(geometry, ST_GeomFromText('{wkt}'))
    """).fetchall()
try:
    rt_zonal()
    R["W3_zonal_rt"] = timed(rt_zonal, n=3)
except Exception as e:
    R["W3_zonal_rt"] = f"EXPR_FAIL: {str(e)[:90]}"

for k, v in R.items():
    print(f"{k:28s} {v if isinstance(v, str) else f'{v*1000:9.1f} ms'}")
