"""필지 반복 접근 벤치 (#55) — 타일 캐시 전/후 대조.

리포트의 병리 재현: 타일 하나(256px = 2,560m)보다 훨씬 작은 필지(10×10px)
수백 개가 같은 타일을 공유하는데, 캐시가 없으면 필지마다 타일을 재fetch 한다.
docs/benchmarks/2026-07-29-tile-cache.md 의 수치 산출.

사전 조건: `make release` + `python scripts/bench_timeseries.py gen`
(/tmp/cogbench-ts 의 2048² 씬 재사용). 원격 경로는 RangeHTTPServer 로 모사:
  cd /tmp/cogbench-ts && uv run --project <repo> python -m RangeHTTPServer 18933

사용: 캐시는 프로세스 시작 시 env 로 고정되므로 구성별로 새 프로세스로 돈다.
  COG_TILE_CACHE_MB=0   uv run python scripts/bench_parcels.py   # 캐시 끔
  COG_TILE_CACHE_MB=256 uv run python scripts/bench_parcels.py   # 캐시 켬 (기본)
"""

import os
import statistics
import time

import duckdb

URL = "http://127.0.0.1:18933/scene_00_20260601.tif"
EXT = "build/release/cog.duckdb_extension"
N_PARCELS = 200

# 씬: origin (300000, 4000000), 10m 픽셀, 2048², 256px 타일 (=2,560m).
# 필지 200개를 타일 4개(2×2) 영역 안에 10×10px 로 격자 배치 — 리포트의
# "타일당 수십~수백 필지" 형상. 결정적 (i 기반 격자).
PARCELS = []
for i in range(N_PARCELS):
    col = i % 20
    row = i // 20
    x0 = 300100.0 + col * 250.0   # 20열 × 250m = 5,000m ≈ 타일 2개 폭
    y0 = 3998000.0 - row * 250.0  # 10행 × 250m = 2,500m ≈ 타일 1~2개 높이
    PARCELS.append((x0, y0 - 100.0, x0 + 100.0, y0))


def run_pass(con):
    t0 = time.perf_counter()
    for (xmin, ymin, xmax, ymax) in PARCELS:
        con.execute(
            f"SELECT RS_ZonalStats('{URL}', [{xmin}, {ymin}, {xmax}, {ymax}], 1, 'mean')"
        ).fetchone()
    return (time.perf_counter() - t0) * 1000


def main():
    mb = os.environ.get("COG_TILE_CACHE_MB", "256(기본)")
    con = duckdb.connect(config={"allow_unsigned_extensions": True})
    con.execute(f"LOAD '{EXT}'")
    cold = run_pass(con)
    warms = [run_pass(con) for _ in range(3)]
    warm = statistics.median(warms)
    print(f"COG_TILE_CACHE_MB={mb:>10s}  cold {cold:8.1f} ms   warm {warm:8.1f} ms"
          f"   ({cold / N_PARCELS:5.2f} / {warm / N_PARCELS:5.2f} ms/필지)")


if __name__ == "__main__":
    main()
