"""RFC §6.5 I/O 경로 실측: (a) object_store 직행 vs (b) DuckDB FileSystem 브릿지 (이슈 #30).

cog_io_bench(path, io := ...) 가 소스별로 같은 엔진 워크로드(open_cog 메타 →
산개 64점 read_pixels → 중앙 절반 창 zonal_stats)를 돌려 (metric, value) 행을
내놓는다 — ms 는 물론 fetch 횟수/바이트(T5 counting)와 픽셀 체크섬(정합)까지.
이 스크립트는 전송 축별로 콜드(신규 커넥션) median 을 모아 표로 찍는다.

축 (인자로 선택, 기본 local+http):
  local   /tmp/cogbench/bench_4096.tif — file · object_store(file://) · duckdb_fs
  http    RangeHTTPServer(127.0.0.1) — object_store vs duckdb_fs(httpfs)
  remote  실 Sentinel-2 B04 (https) — object_store(AWS_SKIP_SIGNATURE) vs duckdb_fs(httpfs)
  secret  (b) 자격증명 UX 검증 — s3:// 익명 접근이 CREATE SECRET 으로 되는지 (#21 footgun)

사전 조건: `just setup` 후 release 빌드(아래 EXT 경로), /tmp/cogbench/bench_4096.tif
(scripts/bench_compare.py 와 동일 픽스처 — gdal_translate 재현 절차는
docs/benchmarks/2026-07-12-comparative.md). remote/secret 은 네트워크 필요.

사용: uv run python scripts/bench_io_compare.py [local] [http] [remote] [secret] [--url=...]
"""

import os
import statistics
import subprocess
import sys
import time

import duckdb

EXT = "build/release/cog.duckdb_extension"
LOCAL = "/tmp/cogbench/bench_4096.tif"
HTTP_PORT = 18931
# 2026-07-12 유즈케이스 검증(워크로그)과 같은 씬 — 공개 버킷, 안정 경로.
REMOTE_HTTPS = (
    "https://sentinel-cogs.s3.us-west-2.amazonaws.com/"
    "sentinel-s2-l2a-cogs/8/X/MR/2026/7/S2C_8XMR_20260712_0_L2A/B04.tif"
)

COLS = ["wall_ms", "open_ms", "meta_ms", "meta_fetches", "meta_bytes",
        "points_ms", "points_fetches", "points_bytes", "points_checksum",
        "window_ms", "window_fetches", "window_bytes", "window_checksum"]


def con(httpfs=False):
    c = duckdb.connect(config={"allow_unsigned_extensions": True})
    c.execute(f"LOAD '{EXT}'")
    if httpfs:
        c.execute("INSTALL httpfs")
        c.execute("LOAD httpfs")
    return c


def bench(path, io, n=5, httpfs=False, setup=()):
    """콜드 실행 n회(커넥션 신규 생성 포함) → 메트릭별 median."""
    acc = {}
    for _ in range(n):
        c = con(httpfs)
        for q in setup:
            c.execute(q)
        t0 = time.perf_counter()
        rows = c.execute(
            f"SELECT metric, value FROM cog_io_bench('{path}', io := '{io}')"
        ).fetchall()
        acc.setdefault("wall_ms", []).append((time.perf_counter() - t0) * 1000)
        for m, v in rows:
            acc.setdefault(m, []).append(v)
        c.close()
    return {m: statistics.median(vs) for m, vs in acc.items()}


def show(axis, results):
    print(f"\n== {axis} ==")
    print(f"{'':14s}" + "".join(f"{c:>16s}" for c in COLS))
    for label, r in results:
        print(f"{label:14s}" + "".join(
            f"{r.get(c, float('nan')):16.1f}" if c.endswith(("_ms", "fetches"))
            else f"{r.get(c, float('nan')):16.0f}" for c in COLS))


def axis_local():
    results = [
        ("file", bench(LOCAL, "file")),
        ("object_store", bench(f"file://{LOCAL}", "object_store")),
        ("duckdb_fs", bench(LOCAL, "duckdb_fs")),
    ]
    show("local file", results)


def axis_http():
    srv = subprocess.Popen(
        [sys.executable, "-m", "RangeHTTPServer", str(HTTP_PORT)],
        cwd=os.path.dirname(LOCAL),
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        time.sleep(1.0)
        url = f"http://127.0.0.1:{HTTP_PORT}/{os.path.basename(LOCAL)}"
        results = [
            ("object_store", bench(url, "object_store")),
            ("duckdb_fs", bench(url, "duckdb_fs", httpfs=True)),
        ]
        show("local http (RangeHTTPServer)", results)
    finally:
        srv.terminate()
        srv.wait()


def axis_remote(url):
    # (a) object_store: virtual-host S3 는 익명 접근에 AWS_SKIP_SIGNATURE 필요 (#21)
    os.environ["AWS_SKIP_SIGNATURE"] = "true"
    results = [
        ("object_store", bench(url, "object_store", n=3)),
        ("duckdb_fs", bench(url, "duckdb_fs", n=3, httpfs=True)),
    ]
    show(f"remote ({url.split('/')[2]})", results)


def axis_secret(url):
    """(b) 의 자격증명 UX: AWS_SKIP_SIGNATURE env 없이 s3:// 익명 접근이 되는가."""
    os.environ.pop("AWS_SKIP_SIGNATURE", None)
    s3 = url.replace("https://sentinel-cogs.s3.us-west-2.amazonaws.com/", "s3://sentinel-cogs/")
    print("\n== secret UX ((b) duckdb_fs, env 없음) ==")
    cases = [
        ("https, no secret", url, ()),
        ("s3, no secret", s3, ()),
        ("s3, CREATE SECRET config", s3,
         ("CREATE SECRET s2 (TYPE s3, PROVIDER config, REGION 'us-west-2')",)),
    ]
    for label, target, setup in cases:
        try:
            r = bench(target, "duckdb_fs", n=1, httpfs=True, setup=setup)
            print(f"{label:28s} OK  meta {r['meta_ms']:8.1f} ms  checksum {r['window_checksum']:.0f}")
        except Exception as e:  # noqa: BLE001 — 실패 자체가 측정 결과
            print(f"{label:28s} FAIL {str(e).splitlines()[0][:110]}")


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    url = next((a.split("=", 1)[1] for a in sys.argv[1:] if a.startswith("--url=")), REMOTE_HTTPS)
    for axis in args or ["local", "http"]:
        {"local": axis_local, "http": axis_http,
         "remote": lambda: axis_remote(url), "secret": lambda: axis_secret(url)}[axis]()
