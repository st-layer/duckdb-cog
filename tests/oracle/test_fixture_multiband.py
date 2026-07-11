"""T2 픽스처 계약 — multiband 64x64 uint8 3밴드 COG: RS_NumBands·NoDataValue 판정 재료.

nodata 미설정 픽스처 — RS_BandNoDataValue 의 NULL 경로와 1-based 밴드 범위 계약의
오라클. 속성 수치는 test/sql/rs_metadata.test 기대값과 동일해야 한다 (RFC §6.9 T1).
"""
import hashlib
import json
from pathlib import Path

import rasterio

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "test" / "data" / "generated" / "multiband_64x64_u8.tif"
LOCK = ROOT / "tests" / "oracle" / "fixtures.lock"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_fixture_exists():
    assert FIXTURE.is_file(), "픽스처가 없다 — `just fixtures` 가 생성해야 한다"


def test_fixture_matches_lock():
    lock = json.loads(LOCK.read_text())
    assert lock[FIXTURE.name] == sha256(FIXTURE)


def test_fixture_properties():
    with rasterio.open(FIXTURE) as ds:
        assert (ds.width, ds.height) == (64, 64)
        assert ds.count == 3
        assert ds.dtypes == ("uint8", "uint8", "uint8")
        assert ds.block_shapes == [(256, 256)] * 3, "타일은 이미지보다 커도 256 유지"
        assert ds.overviews(1) == [], "블록보다 작은 이미지 — 오버뷰 없음"
        assert ds.nodata is None, "nodata 미설정 — RS_BandNoDataValue NULL 경로"
        assert ds.crs.to_epsg() == 32652
        t = ds.transform
        assert (t.a, t.b, t.c, t.d, t.e, t.f) == (
            10.0, 0.0, 600000.0, 0.0, -10.0, 3900000.0,
        )
