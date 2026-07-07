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
check: fmt clippy test oracle

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
# COG_TEST_FIXTURES 가 픽스처 의존 테스트를 켠다 (없으면 require-env 가 스킵)
ext-test: ext fixtures
    COG_TEST_FIXTURES=test/data/generated make test_debug

# 엔진 wasm32-unknown-unknown 컴파일 판정 (RFC G8) — rustup 환경 필요, CI 상시 실행
wasm-check:
    cargo check -p engine --target wasm32-unknown-unknown

# 결정적 픽스처 생성 (seed 고정 — 해시가 tests/oracle/fixtures.lock 과 일치해야 함)
fixtures:
    uv run python scripts/gen_fixtures.py

# rasterio 오라클 대조 테스트 (T1) — 픽스처 없으면 자동 생성
oracle: fixtures
    uv run pytest tests/oracle -x -q
