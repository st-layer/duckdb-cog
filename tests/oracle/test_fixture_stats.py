"""T2 픽스처 계약 — GDAL_METADATA STATISTICS_* 태그 (§6.7 decode 없는 집계 재료)."""
import hashlib
import json
from pathlib import Path

import rasterio

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "test" / "data" / "generated" / "stats_64x64_u16.tif"
LOCK = ROOT / "tests" / "oracle" / "fixtures.lock"


def test_fixture_locked():
    lock = json.loads(LOCK.read_text())
    assert lock[FIXTURE.name] == hashlib.sha256(FIXTURE.read_bytes()).hexdigest()


def test_statistics_tags_planted():
    with rasterio.open(FIXTURE) as ds:
        t = ds.tags(1)
        assert t["STATISTICS_MINIMUM"] == "33.000000"
        assert t["STATISTICS_MAXIMUM"] == "65477.000000"
        assert t["STATISTICS_MEAN"] == "32939.121338"
        assert t["STATISTICS_STDDEV"] == "18924.488017"
