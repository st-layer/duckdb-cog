"""T1 오라클 대조 (RFC §6.9): RS_Value == rasterio 판독, 조밀 샘플링.

SedonaDB 의 test_rs_value_matches_rasterio 패턴 — 실제 빌드된 익스텐션을
duckdb(pip, ABI 일치 1.5.4)로 로드해 픽셀값을 전수/무작위 대조한다.
빌드 산출물이 필요하므로 COG_EXT_BINARY 없으면 스킵 (just check 의 oracle 은
빌드 비의존 유지 — ext-test 체인이 이 파일을 켠다).
"""
import os
import random
from pathlib import Path

import duckdb
import pytest
import rasterio

ROOT = Path(__file__).resolve().parents[2]
GEN = ROOT / "test" / "data" / "generated"
EXT = os.environ.get("COG_EXT_BINARY")

pytestmark = pytest.mark.skipif(
    not EXT, reason="COG_EXT_BINARY 미설정 — ext 빌드 의존 오라클은 ext-test 체인에서"
)


@pytest.fixture(scope="module")
def con():
    c = duckdb.connect(config={"allow_unsigned_extensions": True})
    c.execute(f"LOAD '{Path(EXT).resolve()}'")
    return c


def sql_values(con, path, pts, band=None):
    """포인트 배치를 테이블로 넣어 벡터화 경로(청크 dedupe 포함)를 그대로 태운다."""
    con.execute("CREATE OR REPLACE TABLE pts(x DOUBLE, y DOUBLE)")
    con.executemany("INSERT INTO pts VALUES (?, ?)", pts)
    band_arg = f", {band}" if band is not None else ""
    rows = con.execute(
        f"SELECT RS_Value('{path}', x, y{band_arg}) FROM pts ORDER BY rowid"
    ).fetchall()
    return [r[0] for r in rows]


def rio_values(ds, pts, band_idx0):
    return [vals[band_idx0] for vals in ds.sample(pts)]


def test_multiband_every_pixel_center_all_bands(con):
    """64x64 전 픽셀 중심 × 3밴드 전수 대조 — 조밀 샘플링의 핵심."""
    path = GEN / "multiband_64x64_u8.tif"
    with rasterio.open(path) as ds:
        t = ds.transform
        pts = [
            (t.c + (c + 0.5) * t.a, t.f + (r + 0.5) * t.e)
            for r in range(ds.height)
            for c in range(ds.width)
        ]
        for band in (1, 2, 3):
            expected = rio_values(ds, pts, band - 1)
            actual = sql_values(con, path, pts, band=band)
            assert actual == pytest.approx(expected), f"band {band} 불일치"


def test_random_interior_points_with_subpixel_offsets(con):
    """basic/edge: 무작위 내부점 + 픽셀 내 오프셋 (seed 고정) — 반올림 경계 검증."""
    rng = random.Random(20260711)
    for name in ("basic_512x512_u16.tif", "edge_400x300_u16.tif"):
        path = GEN / name
        with rasterio.open(path) as ds:
            b = ds.bounds
            pts = [
                (rng.uniform(b.left, b.right - 1e-6), rng.uniform(b.bottom + 1e-6, b.top))
                for _ in range(300)
            ]
            expected = rio_values(ds, pts, 0)
            actual = sql_values(con, path, pts)
            assert actual == pytest.approx(expected), f"{name} 불일치"


def test_outside_extent_is_null_not_zero(con):
    """extent 밖 → NULL (rasterio sample 은 0 을 주지만 우리 계약은 NULL — §6.8)."""
    path = GEN / "basic_512x512_u16.tif"
    pts = [(299999.0, 3999995.0), (305120.1, 3999995.0), (300005.0, 4000000.1), (0.0, 0.0)]
    assert sql_values(con, path, pts) == [None, None, None, None]


def test_band_out_of_range_is_null(con):
    path = GEN / "multiband_64x64_u8.tif"
    assert sql_values(con, path, [(600325.0, 3899675.0)], band=4) == [None]
    assert sql_values(con, path, [(600325.0, 3899675.0)], band=0) == [None]


def test_rs_values_batch_matches_rasterio(con):
    """RS_Values 한 호출로 전 픽셀 중심 배치 — 개별 RS_Value 와 동일 경로 검증."""
    path = GEN / "multiband_64x64_u8.tif"
    with rasterio.open(path) as ds:
        t = ds.transform
        pts = [
            (t.c + (c + 0.5) * t.a, t.f + (r + 0.5) * t.e)
            for r in range(ds.height)
            for c in range(ds.width)
        ]
        xs = [p[0] for p in pts]
        ys = [p[1] for p in pts]
        for band in (1, 2, 3):
            expected = rio_values(ds, pts, band - 1)
            (actual,) = con.execute(
                f"SELECT RS_Values('{path}', ?, ?, {band})", [xs, ys]
            ).fetchone()
            assert actual == pytest.approx(expected), f"band {band} 배치 불일치"


def test_normalized_difference_matches_rasterio(con):
    """전 픽셀 중심에서 ND(1,2) == rasterio 로 계산한 (b2-b1)/(b2+b1)."""
    path = GEN / "multiband_64x64_u8.tif"
    with rasterio.open(path) as ds:
        t = ds.transform
        pts = [
            (t.c + (c + 0.5) * t.a, t.f + (r + 0.5) * t.e)
            for r in range(ds.height)
            for c in range(ds.width)
        ]
        b1 = [float(v[0]) for v in ds.sample(pts)]
        b2 = [float(v[1]) for v in ds.sample(pts)]
        expected = [
            None if (x + y) == 0 else (y - x) / (y + x) for x, y in zip(b1, b2)
        ]
        con.execute("CREATE OR REPLACE TABLE ndpts(x DOUBLE, y DOUBLE)")
        con.executemany("INSERT INTO ndpts VALUES (?, ?)", pts)
        rows = con.execute(
            f"SELECT RS_NormalizedDifference('{path}', x, y, 1, 2) FROM ndpts ORDER BY rowid"
        ).fetchall()
        actual = [r[0] for r in rows]
        assert actual == pytest.approx(expected)


def test_zonal_stats_matches_numpy_windows(con):
    """무작위 윈도 10개 × basic/edge — count/sum/mean/min/max == numpy."""
    import numpy as np

    rng = random.Random(20260712)
    for name in ("basic_512x512_u16.tif", "edge_400x300_u16.tif"):
        path = GEN / name
        with rasterio.open(path) as ds:
            t = ds.transform
            for _ in range(10):
                c0 = rng.randrange(0, ds.width - 1)
                c1 = rng.randrange(c0, ds.width)
                r0 = rng.randrange(0, ds.height - 1)
                r1 = rng.randrange(r0, ds.height)
                a = ds.read(1, window=((r0, r1 + 1), (c0, c1 + 1))).astype(np.float64)
                # 픽셀 중심 포함 bbox (중심 좌표 그대로 경계로)
                bbox = [
                    t.c + (c0 + 0.5) * t.a,
                    t.f + (r1 + 0.5) * t.e,
                    t.c + (c1 + 0.5) * t.a,
                    t.f + (r0 + 0.5) * t.e,
                ]
                q = lambda stat: con.execute(
                    f"SELECT RS_ZonalStats('{path}', {bbox}, 1, '{stat}')"
                ).fetchone()[0]
                assert q("count") == a.size
                assert q("sum") == pytest.approx(a.sum())
                assert q("mean") == pytest.approx(a.mean())
                assert q("min") == a.min()
                assert q("max") == a.max()


def test_zonal_stats_polygon_matches_geometry_mask(con):
    """폴리곤 zone (WKT) == geometry_mask+numpy — 고정 P1 + 무작위 notch 6개 × basic/edge (#48)."""
    import numpy as np
    from rasterio.features import geometry_mask

    def snap(v):
        # 픽셀 중심(…5) 비정렬 좌표(…3.7)로 스냅 — on-edge 퇴화 방지
        return round(v / 10.0) * 10.0 + 3.7

    def ring_wkt(r):
        return "(" + ", ".join(f"{x} {y}" for x, y in r) + ")"

    def wkt_of(geom):
        if geom["type"] == "Polygon":
            return "POLYGON (" + ", ".join(ring_wkt(r) for r in geom["coordinates"]) + ")"
        return "MULTIPOLYGON (" + ", ".join(
            "(" + ", ".join(ring_wkt(r) for r in poly) + ")" for poly in geom["coordinates"]
        ) + ")"

    rng = random.Random(20260725)
    fixed_basic = {  # 4타일 걸침 오목+구멍 — sqllogictest/엔진 테스트와 동일 P1
        "type": "Polygon",
        "coordinates": [
            [(301203.7, 3995803.7), (304003.7, 3995803.7), (304003.7, 3998803.7),
             (303203.7, 3998803.7), (303203.7, 3996803.7), (302203.7, 3996803.7),
             (302203.7, 3998803.7), (301203.7, 3998803.7), (301203.7, 3995803.7)],
            [(303403.7, 3996003.7), (303803.7, 3996003.7), (303803.7, 3996403.7),
             (303403.7, 3996403.7), (303403.7, 3996003.7)],
        ],
    }
    for name in ("basic_512x512_u16.tif", "edge_400x300_u16.tif"):
        path = GEN / name
        with rasterio.open(path) as ds:
            b = ds.bounds
            geoms = [fixed_basic] if name.startswith("basic") else []
            for _ in range(6):
                # 무작위 사각형 + 상단 notch — 모든 꼭짓점이 …3.7 격자 (축정렬 변만)
                x0 = snap(rng.uniform(b.left, b.right - 700))
                y0 = snap(rng.uniform(b.bottom, b.top - 700))
                x1 = x0 + 10 * rng.randrange(40, 65)
                y1 = y0 + 10 * rng.randrange(40, 65)
                nx0 = x0 + 10 * rng.randrange(8, 16)
                nx1 = nx0 + 10 * rng.randrange(8, 16)
                ny = y0 + 10 * rng.randrange(8, 24)
                ring = [(x0, y0), (x1, y0), (x1, y1), (nx1, y1), (nx1, ny),
                        (nx0, ny), (nx0, y1), (x0, y1), (x0, y0)]
                geoms.append({"type": "Polygon", "coordinates": [ring]})
            a = ds.read(1).astype(np.float64)
            for geom in geoms:
                m = geometry_mask([geom], out_shape=(ds.height, ds.width),
                                  transform=ds.transform, invert=True)
                if ds.nodata is not None:
                    m &= a != ds.nodata
                v = a[m]
                wkt = wkt_of(geom)
                q = lambda stat: con.execute(
                    f"SELECT RS_ZonalStats('{path}', '{wkt}', 1, '{stat}')"
                ).fetchone()[0]
                assert q("count") == v.size, f"{name} {wkt[:60]}… count 불일치"
                if v.size:
                    assert q("sum") == pytest.approx(v.sum())
                    assert q("mean") == pytest.approx(v.mean())
                    assert q("min") == v.min()
                    assert q("max") == v.max()
                else:
                    assert q("mean") is None
