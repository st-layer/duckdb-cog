"""T2 픽스처 계약 — basic 512x512 uint16 COG의 결정성과 속성.

이 테스트는 계약이다: 구현(gen_fixtures.py)이 이 계약을 만족해야 하며,
불일치 시 테스트가 아니라 구현을 고친다 (AGENTS.md 판정 규칙).
"""
import hashlib
import json
from pathlib import Path

import rasterio

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "test" / "data" / "generated" / "basic_512x512_u16.tif"
LOCK = ROOT / "tests" / "oracle" / "fixtures.lock"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_fixture_exists():
    assert FIXTURE.is_file(), "픽스처가 없다 — `just fixtures` 가 생성해야 한다"


def test_fixture_matches_lock():
    """결정성 계약: 생성된 바이트가 커밋된 lock 해시와 정확히 일치."""
    assert LOCK.is_file(), "fixtures.lock 이 없다 — 생성기가 기록해야 한다"
    lock = json.loads(LOCK.read_text())
    assert lock[FIXTURE.name] == sha256(FIXTURE)


def test_fixture_properties():
    """rasterio(오라클) 판독 속성이 설계값과 일치."""
    with rasterio.open(FIXTURE) as ds:
        assert (ds.width, ds.height) == (512, 512)
        assert ds.count == 1
        assert ds.dtypes[0] == "uint16"
        assert ds.block_shapes == [(256, 256)], "내부 타일 256x256"
        assert ds.overviews(1) == [2], "오버뷰 1레벨 (512→256)"
        assert ds.nodata == 0
        assert ds.crs.to_epsg() == 32652
        t = ds.transform
        assert (t.a, t.b, t.c, t.d, t.e, t.f) == (
            10.0, 0.0, 300000.0, 0.0, -10.0, 4000000.0,
        ), "GDAL 순서 geotransform: 10m 해상도, 원점 (300000, 4000000)"
