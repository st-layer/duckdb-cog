"""T2 픽스처 계약 — tiny 16x16 uint8 COG: readahead 32KiB 초과 요청(EOF 클램프) 재료.

전체 파일이 readahead 초기 요청(32KiB)보다 작아야 존재 의미가 있다 —
ByteSource 의 EOF 클램프 계약을 로컬·원격 양쪽에서 판정하는 유일한 픽스처.
"""
import hashlib
import json
from pathlib import Path

import rasterio

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "test" / "data" / "generated" / "tiny_16x16_u8.tif"
LOCK = ROOT / "tests" / "oracle" / "fixtures.lock"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_fixture_exists():
    assert FIXTURE.is_file(), "픽스처가 없다 — `just fixtures` 가 생성해야 한다"


def test_fixture_matches_lock():
    lock = json.loads(LOCK.read_text())
    assert lock[FIXTURE.name] == sha256(FIXTURE)


def test_fixture_smaller_than_readahead():
    assert FIXTURE.stat().st_size < 32 * 1024, "32KiB 이상이면 이 픽스처의 존재 이유가 사라진다"


def test_fixture_properties():
    with rasterio.open(FIXTURE) as ds:
        assert (ds.width, ds.height) == (16, 16)
        assert ds.count == 1
        assert ds.dtypes[0] == "uint8"
        assert ds.block_shapes == [(128, 128)]
        assert ds.overviews(1) == []
        assert ds.nodata is None
        assert ds.crs.to_epsg() == 32652
        t = ds.transform
        assert (t.a, t.c, t.e, t.f) == (10.0, 700000.0, -10.0, 3950000.0)
