"""5년 NDVI 시계열 조널 평균 벤치 — GeoJSON AOI 1개 × Sentinel-2 L2A (earth-search).

워크로드: AOI(GeoJSON, EPSG:4326) + 조회 기간(기본 2021-01-01/2025-12-31, 5년)
→ read_stac_search 로 씬 목록 확보 → 씬별 NDVI 조널 평균 시계열.
AOI 는 항적/타일 중첩에 따라 같은 날짜에 씬이 여럿일 수 있다 — dedupe 없이
검색 결과 전체를 그대로 계산한다 (구름 필터 없음: read_stac_search 는
eo:cloud_cover 를 노출하지 않는다 — 성능 벤치에는 무관).

경로 2종 (NDVI 는 raw DN 기준 (nir-red)/(nir+red) — L2A BOA offset 미보정):
  A (band-mean): RS_ZonalStats(red,'mean') · RS_ZonalStats(nir,'mean') →
     (nir̄-red̄)/(nir̄+red̄). "평균의 NDVI". 청크 행 동시 실행(#74) 적용 경로.
     +fast: 5-인자 max_pixels(#71) 로 오버뷰 근사.
  B (per-pixel): RS_BandAsArray(band, 폴리곤존) 픽셀 배열 → 픽셀별 NDVI → avg.
     "NDVI 의 평균" (의미상 정답). 행 순차 — 동시화 미적용 경로(#74 범위 밖).

측정 프로토콜: 측정마다 콜드 프로세스(서브프로세스) — 타일 캐시·커넥션 풀
공유 배제. STAC 검색은 별도 1회 측정 후 씬 목록을 parquet 으로 고정, A/B 는
동일 목록을 읽는다. 씬의 UTM zone 은 href 경로에서 파싱해 AOI 를 zone 별로
투영(rasterio.warp — 벤치 스크립트 한정, RFC N4 위반 아님).

사전 조건: `make release` (build/release/cog.duckdb_extension), 인터넷.

사용 (repo 루트에서):
  uv run python scripts/bench_ndvi_timeseries.py all            # 전체 (search→A×2→Afast→B)
  uv run python scripts/bench_ndvi_timeseries.py all --smoke    # 1개월 스모크
  uv run python scripts/bench_ndvi_timeseries.py all --cap 64   # A 동시성 캡 변경
개별 단계(search 선행 필요 — /tmp/cogbench-ndvi/ 에 씬 목록 캐시): search | a | b
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time

import duckdb

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXT = os.path.join(ROOT, "build/release/cog.duckdb_extension")
DIR = "/tmp/cogbench-ndvi"
STAC = "https://earth-search.aws.element84.com/v1/search"
DEFAULT_AOI = os.path.join(ROOT, "scripts/data/bench_aoi_gimje.geojson")
DT_5Y = "2021-01-01T00:00:00Z/2025-12-31T23:59:59Z"
DT_SMOKE = "2025-04-01T00:00:00Z/2025-04-30T23:59:59Z"


def connect():
    con = duckdb.connect(config={"allow_unsigned_extensions": True})
    con.execute(f"LOAD '{EXT}'")
    return con


def load_polygon(path):
    with open(path) as f:
        g = json.load(f)
    if g.get("type") == "FeatureCollection":
        g = g["features"][0]["geometry"]
    elif g.get("type") == "Feature":
        g = g["geometry"]
    if g["type"] != "Polygon":
        sys.exit(f"Polygon 만 지원: {g['type']}")
    return g


def epsg_of(href):
    """S2 COG href 의 MGRS 경로(<zone>/<latband>/<sq>)에서 UTM EPSG."""
    m = re.search(r"/(\d{1,2})/([C-X])/[A-Z]{2}/", href)
    if not m:
        sys.exit(f"href 에서 UTM zone 파싱 실패: {href}")
    zone, band = int(m.group(1)), m.group(2)
    return (32600 if band >= "N" else 32700) + zone


def utm_wkt(geom, epsg):
    from rasterio.warp import transform_geom

    t = transform_geom("EPSG:4326", f"EPSG:{epsg}", geom)
    rings = ", ".join(
        "(" + ", ".join(f"{x:.3f} {y:.3f}" for x, y in ring) + ")"
        for ring in t["coordinates"]
    )
    return f"POLYGON ({rings})"


def ring_area_px(wkt):
    """외곽 링 shoelace 면적(m²) → 10m 픽셀 수 추정 (규모 감각용)."""
    xy = [tuple(map(float, p.split())) for p in wkt.split("((")[1].split(")")[0].split(", ")]
    s = sum(xy[i][0] * xy[i + 1][1] - xy[i + 1][0] * xy[i][1] for i in range(len(xy) - 1))
    return abs(s) / 2 / 100


def search(aoi_path, dtrange):
    """STAC 검색(측정 1) → 씬 목록·zone 별 WKT 를 /tmp 에 고정."""
    os.makedirs(DIR, exist_ok=True)
    geom = load_polygon(aoi_path)
    xs = [x for ring in geom["coordinates"] for x, _ in ring]
    ys = [y for ring in geom["coordinates"] for _, y in ring]
    bbox = [min(xs), min(ys), max(xs), max(ys)]

    con = connect()
    t0 = time.perf_counter()
    con.execute(
        f"""CREATE TABLE raw AS
            SELECT item_id, datetime::VARCHAR AS datetime, asset_key, href
            FROM read_stac_search('{STAC}', collections := ['sentinel-2-l2a'],
                 bbox := {bbox}, datetime := '{dtrange}',
                 page_size := 200, max_rows := 200000)
            WHERE asset_key IN ('red', 'nir') AND href LIKE '%.tif'"""
    )
    stac_s = time.perf_counter() - t0
    # COG 아닌 자산(예: 2022-01-07 두 아이템은 JP2 원본을 가리킴)을 거른 뒤
    # red·nir 이 모두 남은 아이템만 — 제외분은 개수로 보고
    all_rows = con.execute(
        """SELECT item_id, any_value(datetime),
                  max(CASE WHEN asset_key = 'red' THEN href END),
                  max(CASE WHEN asset_key = 'nir' THEN href END)
           FROM raw GROUP BY item_id ORDER BY 2"""
    ).fetchall()
    rows = [r for r in all_rows if r[2] is not None and r[3] is not None]
    if not rows:
        sys.exit("red/nir COG 자산을 가진 아이템이 없다")
    dropped = len(all_rows) - len(rows)

    con.execute("CREATE TABLE scenes(item_id VARCHAR, datetime VARCHAR, epsg INT, red VARCHAR, nir VARCHAR)")
    con.executemany(
        "INSERT INTO scenes VALUES (?, ?, ?, ?, ?)",
        [(i, d, epsg_of(red), red, nir) for i, d, red, nir in rows],
    )
    epsgs = [e for (e,) in con.execute("SELECT DISTINCT epsg FROM scenes ORDER BY 1").fetchall()]
    con.execute("CREATE TABLE zones(epsg INT, wkt VARCHAR)")
    con.executemany("INSERT INTO zones VALUES (?, ?)", [(e, utm_wkt(geom, e)) for e in epsgs])
    con.execute(f"COPY scenes TO '{DIR}/scenes.parquet'")
    con.execute(f"COPY zones TO '{DIR}/zones.parquet'")

    n_dates = con.execute("SELECT count(DISTINCT datetime[:10]) FROM scenes").fetchone()[0]
    px = ring_area_px(con.execute("SELECT wkt FROM zones LIMIT 1").fetchone()[0])
    print(
        f"PHASE search wall={stac_s:.1f}s items={len(rows)} (비COG 제외 {dropped}) "
        f"dates={n_dates} epsg={epsgs} aoi_px≈{px:,.0f} range={dtrange}",
        flush=True,
    )


def run_phase(kind, max_pixels):
    """A/B 쿼리 1회 (콜드 프로세스 전제 — all 러너가 서브프로세스로 부른다)."""
    con = connect()
    con.execute(f"CREATE TABLE scenes AS FROM '{DIR}/scenes.parquet'")
    con.execute(f"CREATE TABLE zones AS FROM '{DIR}/zones.parquet'")
    mp = f", {max_pixels}" if max_pixels else ""
    if kind == "a":
        q = f"""SELECT item_id, datetime,
                       (nir_m - red_m) / nullif(nir_m + red_m, 0.0) AS ndvi
                FROM (SELECT s.item_id, s.datetime,
                             RS_ZonalStats(s.red, z.wkt, 1, 'mean'{mp}) AS red_m,
                             RS_ZonalStats(s.nir, z.wkt, 1, 'mean'{mp}) AS nir_m
                      FROM scenes s JOIN zones z USING (epsg))
                ORDER BY datetime"""
    else:
        q = """WITH px AS (
                 SELECT s.item_id, s.datetime,
                        unnest(RS_BandAsArray(s.red, 1, z.wkt)) AS r,
                        unnest(RS_BandAsArray(s.nir, 1, z.wkt)) AS n
                 FROM scenes s JOIN zones z USING (epsg))
               SELECT item_id, datetime, avg((n - r) / nullif(n + r, 0.0)) AS ndvi
               FROM px GROUP BY item_id, datetime ORDER BY datetime"""
    t0 = time.perf_counter()
    rows = con.execute(q).fetchall()
    wall = time.perf_counter() - t0
    nulls = sum(1 for r in rows if r[2] is None)
    cap = os.environ.get("COG_FETCH_CONCURRENCY", "16(기본)")
    label = f"{kind}{'-fast' + str(max_pixels) if max_pixels else ''}"
    print(
        f"PHASE {label} cap={cap} wall={wall:.1f}s rows={len(rows)} null={nulls} "
        f"({wall / max(len(rows), 1) * 1000:.0f} ms/scene)",
        flush=True,
    )
    for r in rows[:2] + rows[-2:]:
        print(f"  {r[1][:10]}  {r[0]}  ndvi={r[2] if r[2] is None else round(r[2], 4)}", flush=True)


def run_all(args):
    dtrange = DT_SMOKE if args.smoke else args.datetime
    search(args.aoi, dtrange)
    phases = [("a", 0), ("a", 0), ("a", 4096)] + ([] if args.skip_b else [("b", 0)])
    for kind, mp in phases:
        env = {**os.environ, "COG_FETCH_CONCURRENCY": str(args.cap)}
        subprocess.run(
            [sys.executable, __file__, kind, "--max-pixels", str(mp)],
            env=env, check=False,
        )


def main():
    p = argparse.ArgumentParser()
    p.add_argument("mode", choices=["all", "search", "a", "b"])
    p.add_argument("--aoi", default=DEFAULT_AOI)
    p.add_argument("--datetime", default=DT_5Y)
    p.add_argument("--smoke", action="store_true")
    p.add_argument("--cap", type=int, default=16, help="A 의 COG_FETCH_CONCURRENCY")
    p.add_argument("--max-pixels", type=int, default=0)
    p.add_argument("--skip-b", action="store_true")
    args = p.parse_args()
    # sentinel-cogs 는 공개 버킷 — 서명 생략 안 하면 object_store 가 IMDS 를 찾다 죽는다
    os.environ.setdefault("AWS_SKIP_SIGNATURE", "true")
    if args.mode == "all":
        run_all(args)
    elif args.mode == "search":
        search(args.aoi, DT_SMOKE if args.smoke else args.datetime)
    else:
        run_phase(args.mode, args.max_pixels)


if __name__ == "__main__":
    main()
