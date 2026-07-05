# duckdb-cog

GDAL-free COG reader for DuckDB. Cloud-Optimized GeoTIFF를 재인코딩·재투영 없이
제자리에서(in-place) 읽어 SQL 테이블로 노출하는 익스텐션.

설계 준거: `docs/RFC-001-rev3.md` (필독). 개발 프로세스: `docs/HARNESS.md`.

## 판정 (verification)

- **완료 판정은 `just check` 통과가 유일한 기준 (익스텐션 E2E는 `just ext-test`).** 통과 전 완료 선언 금지.
- 테스트 실패 시 `tests/` 를 고쳐서 통과시키지 말 것 — 테스트가 계약이다.
  테스트 자체가 틀렸다고 판단되면 수정하지 말고 이유를 보고하고 승인을 받아라.
- 오라클(rasterio)과 우리 결과가 불일치하면 임의로 판정하지 말고
  `docs/oracle-disputes.md` 에 기록 후 사람 판정을 기다린다.

## 아키텍처 불변식 (위반 = 즉시 실패)

- 읽기 경로(`crates/engine`, `crates/duckdb-ext`)에 GDAL/PROJ/GEOS 링크 금지 (RFC N4).
  GDAL·rasterio는 `tests/`, `scripts/` 의 오라클/픽스처 용도로만 허용.
- decode/fetch는 async-tiff에 위임 — TIFF/IFD 파싱 재구현 금지 (RFC N7).
  async-tiff 커버리지 공백 발견 시: 우회 구현 대신 이슈로 보고.
- 읽기 시점 reproject 금지 (RFC N2). 픽셀값을 변형하는 경로를 만들지 않는다.
- `crates/engine` 은 wasm32-unknown-unknown 컴파일 가능 상태를 유지한다 (RFC G8).
- async-tiff 직접 호출은 engine 내부의 reader 경계(trait) 뒤에서만 (RFC R8).

## SQL 표면

- 테이블 함수는 우리 것: `read_cog()`, `read_stac()`.
- 스칼라/집계 함수는 Sedona `RS_*` 카탈로그를 따라 만든다 (RFC §6.8):
  1-based 밴드 인덱스, 범위 밖/nodata → NULL, GDAL 순서 geotransform.
  준거는 착수 시점 스냅샷 — Sedona 드리프트는 추적하지 않는다.

## 구조 (duckdb/extension-template-rs 표준 + workspace)

- 리포 루트 = 익스텐션 크레이트 `cog` (`src/lib.rs`, cdylib). `src/wasm_lib.rs` 는 WASM 빌드 우회용 — 내용 변경 금지.
- `crates/engine` = 엔진 크레이트 (도메인 로직은 전부 여기, lib.rs 는 SQL 배선만).
- `Makefile` + `extension-ci-tools`(서브모듈) = 표준 익스텐션 빌드/테스트. 수정 금지.
- `test/sql/*.test` = sqllogictest (E2E 계약). `tests/oracle/` = rasterio 오라클 대조.

## 명령

- 판정(전체): `just check`   ·  빌드: `just build`  ·  테스트만: `just test`
- 익스텐션 빌드: `just ext`  ·  익스텐션 E2E(sqllogictest): `just ext-test`
- 최초 1회 셋업(서브모듈+venv): `just setup`
- 픽스처 재생성: `just fixtures` (결정적 — seed 고정, Phase 1에서 실체화)

## 작업 방식

- **브랜치 전략 (GitHub Flow): 세션 = 슬라이스 = 브랜치 = PR.** main은 항상 그린이며
  직접 commit/push/merge 금지(`block_danger.py` 훅이 강제). 네이밍: `feat/*`, `fix/*`,
  `chore/*`, `docs/*`. PR은 슬라이스 하나만 담고 squash merge 후 브랜치 삭제.
  머지는 사람이 GitHub에서 한다. **PR 제목·본문은 영어로** (squash 시 main 히스토리가 된다).
  커밋 메시지에 co-author 트레일러를 넣지 않는다.
- 다중 파일 변경은 plan mode로 계획 → 승인 → 실행.
- 태스크는 수직 슬라이스로: 픽스처 1개 → 기능 최소 경로 → 테스트 1개 통과.
- TDD 순서: 테스트 작성 → 실패 확인 → 실패 테스트 커밋 → 구현 → `just check` 그린.
- 세션이 길어지면 요약을 `docs/worklog/` 에 남기고 새 세션으로.
