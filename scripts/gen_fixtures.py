"""T2: 결정적 합성 COG 픽스처 생성기 (RFC §6.9, HARNESS §7).

seed 고정 + rasterio 버전 고정(uv.lock)으로 항상 동일 바이트를 생성한다.
tests/oracle/fixtures.lock 에 기록된 해시와 불일치하면 실패한다 —
"픽스처를 다시 만들었더니 통과했다" 우회 차단. 픽스처 설계를 의도적으로
바꿀 때만 REGEN_FIXTURES=1 로 lock 을 갱신하고, 그 diff 는 사람이 승인한다.
"""

import hashlib
import json
import os
import sys
from pathlib import Path

import numpy as np
import rasterio
from rasterio.transform import from_origin

ROOT = Path(__file__).resolve().parents[1]
OUT_DIR = ROOT / "test" / "data" / "generated"
LOCK = ROOT / "tests" / "oracle" / "fixtures.lock"


def gen_basic_512x512_u16(path: Path) -> None:
    """512x512 단일밴드 uint16, 256px 타일, 오버뷰 1레벨, EPSG:32652, 10m."""
    rng = np.random.default_rng(42)
    # 0 은 nodata 로 예약 — 데이터 값은 1..65535
    data = rng.integers(1, 65536, size=(512, 512), dtype=np.uint16)
    with rasterio.open(
        path,
        "w",
        driver="COG",
        width=512,
        height=512,
        count=1,
        dtype="uint16",
        crs="EPSG:32652",
        transform=from_origin(300000.0, 4000000.0, 10.0, 10.0),
        nodata=0,
        blocksize=256,
        compress="NONE",  # 압축 변종은 픽스처 매트릭스(다음)에서 — 여기선 결정성 우선
        overview_resampling="nearest",
    ) as dst:
        dst.write(data, 1)


FIXTURES = {
    "basic_512x512_u16.tif": gen_basic_512x512_u16,
}


def main() -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    lock = json.loads(LOCK.read_text()) if LOCK.is_file() else {}
    regen = os.environ.get("REGEN_FIXTURES") == "1"
    changed = False

    for name, gen in FIXTURES.items():
        path = OUT_DIR / name
        gen(path)
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        expected = lock.get(name)
        if expected is None or regen:
            lock[name] = digest
            changed = True
            print(f"[lock] {name} = {digest[:16]}… 기록")
        elif expected != digest:
            print(
                f"FAIL: {name} 해시 불일치 — 생성기는 결정적이어야 한다.\n"
                f"  lock:  {expected}\n  now:   {digest}\n"
                "픽스처 설계를 바꾼 게 의도라면 REGEN_FIXTURES=1 로 갱신 후 "
                "lock diff 를 사람이 승인한다.",
                file=sys.stderr,
            )
            return 1
        else:
            print(f"[ok]   {name} 해시 일치")

    if changed:
        LOCK.write_text(json.dumps(lock, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
