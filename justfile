# duckdb-cog 개발/에이전트 판정 명령. 익스텐션 표준 빌드는 Makefile(템플릿) 담당.
# duckdb-loadable-macros 가 컴파일 시 요구하는 env:
export DUCKDB_EXTENSION_NAME := "cog"
export DUCKDB_EXTENSION_MIN_DUCKDB_VERSION := "v1.5.5"

# rustup 툴체인 우선 (homebrew cargo 1.86 은 async-tiff MSRV ≥1.87 미달).
# rust-toolchain.toml 은 배포 CI의 잡별 툴체인 관리와 충돌해 쓰지 않는다 (PR #3) —
# 로컬 버전 고정은 `rustup default`, CI 고정은 Lint.yml 의 dtolnay 액션이 담당.
export PATH := env_var("HOME") / ".cargo/bin:" + env_var("PATH")

default: check

# 전체 판정 게이트 — 완료 판정의 유일한 기준 (빠른 것부터: HARNESS §2)
# fixtures 가 test 앞: engine 통합테스트(T5 fetch_contract)가 픽스처 파일을 읽는다.
check: fmt clippy fixtures test oracle

fmt:
    cargo fmt --all -- --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

build:
    cargo build --workspace

# ---- 익스텐션 표준 파이프라인 (extension-ci-tools 경유, 최초 1회 setup 필요) ----

# 최초 1회: 서브모듈 + venv/platform 구성
setup:
    git submodule update --init extension-ci-tools
    make configure

# 익스텐션 바이너리 빌드 (debug)
ext:
    make debug

# sqllogictest 실행 (test/sql/*.test) — LOAD 포함 E2E
# COG_TEST_FIXTURES 가 픽스처 의존 테스트를, COG_TEST_HTTP 가 원격(http) 테스트를,
# COG_TEST_STAC_API 가 STAC API 검색 테스트(mock /search)를 켠다.
# 픽스처를 Range 지원 서버(rangehttpserver)로 서빙 — object_store 는 Range GET 필수.
ext-test: ext fixtures
    #!/usr/bin/env bash
    set -euo pipefail
    port=18923
    stac_port=18924
    (cd test/data/generated && exec uv run --project ../../.. python -m RangeHTTPServer "$port") >/tmp/cog-range-server.log 2>&1 &
    srv=$!
    (exec uv run python scripts/mock_stac_api.py "$stac_port" test/data/stac) >/tmp/cog-stac-mock.log 2>&1 &
    stac=$!
    # uv 의 python 자식까지 정리 — 살아남은 자식이 포트를 점유하면 다음 실행이 깨진다
    trap 'pkill -P "$srv" 2>/dev/null || true; kill "$srv" 2>/dev/null || true; pkill -P "$stac" 2>/dev/null || true; kill "$stac" 2>/dev/null || true' EXIT
    ready=0
    for _ in $(seq 50); do
        curl -sf -o /dev/null "http://127.0.0.1:$port/" \
            && curl -sf -o /dev/null -X POST -d '{}' "http://127.0.0.1:$stac_port/search" \
            && ready=1 && break
        sleep 0.1
    done
    if [ "$ready" != 1 ]; then
        echo "FAIL: 테스트 서버가 안 뜸 (:$port range / :$stac_port stac-mock — 포트 점유? /tmp/cog-*.log 확인)" >&2
        exit 1
    fi
    COG_TEST_FIXTURES=test/data/generated COG_TEST_HTTP="http://127.0.0.1:$port" COG_TEST_STAC_API="http://127.0.0.1:$stac_port/search" make test_debug
    # T1 조밀 오라클 (RS_Value ↔ rasterio, ABI 일치 duckdb-python) —
    # 빌드 산출물이 필요해 check 의 oracle 과 분리 (COG_EXT_BINARY 로 활성화)
    COG_EXT_BINARY=build/debug/cog.duckdb_extension uv run pytest tests/oracle/test_rs_value_oracle.py -x -q

# 엔진+사이드카 wasm32-unknown-unknown 컴파일 판정 (RFC G8, #66) — rustup 환경
# 필요, CI 상시 실행.
# macOS: Apple clang 은 wasm 타깃 미지원(zstd-sys C 빌드) — homebrew llvm 이 있으면 사용.
wasm-check:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -x /opt/homebrew/opt/llvm/bin/clang ]; then
        export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
        export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
    fi
    cargo check -p engine -p engine-wasm --target wasm32-unknown-unknown

# 사이드카 패리티 판정 (#66) — engine-wasm 을 헤드리스 Chrome 에서 실행해
# 네이티브 테스트와 동일 골든 상수로 비교. 픽스처는 include_bytes! 라 fixtures 선행.
# 함정: wasm-pack 은 CHROMEDRIVER env 를 무시하고 자체 캐시 드라이버를 받는다 —
# 로컬 Chrome 과 major 가 어긋나면 ~/Library/Caches/.wasm-pack/chromedriver-*/ 의
# 바이너리를 맞는 버전으로 교체 (rm 후 cp — 제자리 덮어쓰기는 macOS 가 SIGKILL).
wasm-test: fixtures
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -x /opt/homebrew/opt/llvm/bin/clang ]; then
        export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
        export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
    fi
    command -v wasm-pack >/dev/null || { echo "wasm-pack 없음 — brew install wasm-pack" >&2; exit 1; }
    # fetch 경로 테스트용 Range+CORS 서버 — tests/web_fetch.rs 의 포트 계약(18925)
    port=18925
    (exec uv run python scripts/range_cors_server.py "$port" test/data/generated) >/tmp/cog-wasm-range.log 2>&1 &
    srv=$!
    trap 'pkill -P "$srv" 2>/dev/null || true; kill "$srv" 2>/dev/null || true' EXIT
    ready=0
    for _ in $(seq 50); do
        curl -sf -o /dev/null "http://127.0.0.1:$port/" && ready=1 && break
        sleep 0.1
    done
    if [ "$ready" != 1 ]; then
        echo "FAIL: range+CORS 서버가 안 뜸 (:$port — 포트 점유? /tmp/cog-wasm-range.log 확인)" >&2
        exit 1
    fi
    wasm-pack test --headless --chrome crates/engine-wasm

# 사이드카 배포 산출물 (#66) — wasm-pack build+pack 으로 npm 설치 가능한 .tgz
# 를 만든다. 배포 채널은 GitHub Release 자산 (WasmArtifact.yml 이 릴리스 발행
# 시 자동 첨부) — npm 레지스트리 배포는 하지 않는다 (#66 스코프 결정, R6 미결).
wasm-artifact:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -x /opt/homebrew/opt/llvm/bin/clang ]; then
        export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
        export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
    fi
    command -v wasm-pack >/dev/null || { echo "wasm-pack 없음 — brew install wasm-pack" >&2; exit 1; }
    command -v npm >/dev/null || { echo "npm 없음 (wasm-pack pack 이 요구) — node 설치 필요" >&2; exit 1; }
    wasm-pack build crates/engine-wasm --target web --release
    wasm-pack pack crates/engine-wasm
    # 스모크: 산출 tgz 에 핵심 파일이 실려 있는지 (빈/불완전 패키지 방지)
    tgz=$(ls crates/engine-wasm/pkg/*.tgz)
    tar -tzf "$tgz" | grep -q 'engine_wasm_bg.wasm'
    tar -tzf "$tgz" | grep -q 'package.json'
    echo "OK: $tgz"

# 결정적 픽스처 생성 (seed 고정 — 해시가 tests/oracle/fixtures.lock 과 일치해야 함)
fixtures:
    uv run python scripts/gen_fixtures.py

# T7 벤치마크 (criterion) — 로컬 성능 관측. CI 회귀 게이트는 후속.
bench: fixtures
    cargo bench -p engine

# 벤치 스모크: "벤치가 실행 가능하다" 판정 (측정 1회 축소 실행)
bench-smoke: fixtures
    cargo bench -p engine -- --test

# rasterio 오라클 대조 테스트 (T1) — 픽스처 없으면 자동 생성
oracle: fixtures
    uv run pytest tests/oracle -x -q
