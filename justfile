# duckdb-cog 개발/에이전트 판정 명령. 익스텐션 표준 빌드는 Makefile(템플릿) 담당.
# duckdb-loadable-macros 가 컴파일 시 요구하는 env:
export DUCKDB_EXTENSION_NAME := "cog"
export DUCKDB_EXTENSION_MIN_DUCKDB_VERSION := "v1.5.4"

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
# COG_TEST_FIXTURES 가 픽스처 의존 테스트를, COG_TEST_HTTP 가 원격(http) 테스트를 켠다.
# 픽스처를 Range 지원 서버(rangehttpserver)로 서빙 — object_store 는 Range GET 필수.
ext-test: ext fixtures
    #!/usr/bin/env bash
    set -euo pipefail
    port=18923
    (cd test/data/generated && exec uv run --project ../../.. python -m RangeHTTPServer "$port") >/tmp/cog-range-server.log 2>&1 &
    srv=$!
    # uv 의 python 자식까지 정리 — 살아남은 자식이 포트를 점유하면 다음 실행이 깨진다
    trap 'pkill -P "$srv" 2>/dev/null || true; kill "$srv" 2>/dev/null || true' EXIT
    ready=0
    for _ in $(seq 50); do
        curl -sf -o /dev/null "http://127.0.0.1:$port/" && ready=1 && break
        sleep 0.1
    done
    if [ "$ready" != 1 ]; then
        echo "FAIL: range 서버가 :$port 에서 안 뜸 (포트 점유? /tmp/cog-range-server.log 확인)" >&2
        exit 1
    fi
    COG_TEST_FIXTURES=test/data/generated COG_TEST_HTTP="http://127.0.0.1:$port" make test_debug

# 엔진 wasm32-unknown-unknown 컴파일 판정 (RFC G8) — rustup 환경 필요, CI 상시 실행.
# macOS: Apple clang 은 wasm 타깃 미지원(zstd-sys C 빌드) — homebrew llvm 이 있으면 사용.
wasm-check:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -x /opt/homebrew/opt/llvm/bin/clang ]; then
        export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
        export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
    fi
    cargo check -p engine --target wasm32-unknown-unknown

# 결정적 픽스처 생성 (seed 고정 — 해시가 tests/oracle/fixtures.lock 과 일치해야 함)
fixtures:
    uv run python scripts/gen_fixtures.py

# rasterio 오라클 대조 테스트 (T1) — 픽스처 없으면 자동 생성
oracle: fixtures
    uv run pytest tests/oracle -x -q
