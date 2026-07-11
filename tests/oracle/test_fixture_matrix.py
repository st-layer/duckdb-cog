"""T2 픽스처 계약 — 압축 변종 + nodata 구멍 매트릭스 (RFC §6.9 T2).

decode 경로(RS_Value)가 생겼으므로 압축 변종이 실물 가치를 가진다.
- deflate_128x128_u16: COMPRESS=DEFLATE (COG 최다 사용)
- zstd_128x128_u16:    COMPRESS=ZSTD (async-tiff zstd 경로)
- nodatahole_64x64_u16: nodata=0 픽셀을 (0,0)에 실제로 심음 — RS_Value NULL E2E 재료
"""
import hashlib
import json
from pathlib import Path

import pytest
import rasterio

ROOT = Path(__file__).resolve().parents[2]
GEN = ROOT / "test" / "data" / "generated"
LOCK = ROOT / "tests" / "oracle" / "fixtures.lock"

CASES = {
    "deflate_128x128_u16.tif": dict(size=(128, 128), compression="deflate", nodata=0),
    "zstd_128x128_u16.tif": dict(size=(128, 128), compression="zstd", nodata=0),
    "nodatahole_64x64_u16.tif": dict(size=(64, 64), compression=None, nodata=0),
}


@pytest.mark.parametrize("name", CASES)
def test_fixture_exists_and_locked(name):
    path = GEN / name
    assert path.is_file(), "픽스처가 없다 — `just fixtures` 가 생성해야 한다"
    lock = json.loads(LOCK.read_text())
    assert lock[name] == hashlib.sha256(path.read_bytes()).hexdigest()


@pytest.mark.parametrize("name", CASES)
def test_fixture_properties(name):
    case = CASES[name]
    with rasterio.open(GEN / name) as ds:
        assert (ds.width, ds.height) == case["size"]
        assert ds.dtypes[0] == "uint16"
        assert ds.nodata == case["nodata"]
        assert ds.crs.to_epsg() == 32652
        comp = ds.profile.get("compress")
        expected = case["compression"]
        assert (comp.lower() if comp else None) == expected, f"압축 {comp} != {expected}"


def test_nodata_hole_is_actually_planted():
    """(0,0) 픽셀이 정말 nodata(0) — RS_Value NULL 경로의 오라클."""
    with rasterio.open(GEN / "nodatahole_64x64_u16.tif") as ds:
        band = ds.read(1)
        assert band[0, 0] == 0, "구멍이 안 심겼다"
        assert (band != 0).any(), "구멍만 있으면 대조 재료가 없다"


def test_compressed_variants_decode_to_same_values_as_uncompressed_rng():
    """같은 seed → 압축과 무관하게 동일 픽셀값 (decode 정확성의 오라클 기준선)."""
    with rasterio.open(GEN / "deflate_128x128_u16.tif") as d, rasterio.open(
        GEN / "zstd_128x128_u16.tif"
    ) as z:
        assert (d.read(1) == z.read(1)).all(), "동일 seed 인데 압축별 픽셀 불일치"
