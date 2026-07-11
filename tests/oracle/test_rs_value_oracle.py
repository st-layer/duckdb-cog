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
