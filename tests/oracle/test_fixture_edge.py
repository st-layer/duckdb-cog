"""T2 픽스처 계약 — edge 400x300 uint16 COG: 엣지 클리핑 판정 재료.

이미지가 타일 크기로 나누어떨어지지 않아 우/하단 타일이 클립되는 케이스.
bounds 는 read_cog bbox 의 sqllogictest 기대값과 동일 수치 — rasterio 가
같은 값을 판정하는 것이 오라클 상호 검증이다 (RFC §6.9 T1).
"""
import hashlib
import json
from pathlib import Path

import rasterio

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "test" / "data" / "generated" / "edge_400x300_u16.tif"
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
        assert (ds.width, ds.height) == (400, 300)
        assert ds.count == 1
        assert ds.dtypes[0] == "uint16"
        assert ds.block_shapes == [(256, 256)], "내부 타일 256x256 (물리 크기)"
        assert ds.overviews(1) == [2], "오버뷰 1레벨 (400x300 → 200x150)"
        assert ds.nodata == 0
        assert ds.crs.to_epsg() == 32652
        t = ds.transform
        assert (t.a, t.b, t.c, t.d, t.e, t.f) == (
            10.0, 0.0, 500000.0, 0.0, -10.0, 3800000.0,
        )
        # 데이터 범위 — read_cog bbox 클리핑 기대값의 오라클
        assert ds.bounds == (500000.0, 3797000.0, 504000.0, 3800000.0)
