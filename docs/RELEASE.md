# 릴리스·배포 절차 (community-extensions)

v0.1.0 등록(#2274)과 첫 배포 사고(1.5.4/1.5.5 스탬프 불일치 → CE #2313)에서
실측으로 배운 것의 정본. 세션 현황 스냅샷은 `docs/worklog/` 최신 파일 참조.

## 버전 체계

- **SemVer** (`MAJOR.MINOR.PATCH`). 0.x 동안은 `0.MINOR` 가 breaking 신호.
- 버전은 머지가 아니라 **릴리스 행위**에 붙는다 — PR 은 main 에 쌓이고,
  의미 있는 묶음이 되면 한 번에 릴리스한다 (raster 참조 리듬: 3개월간 11릴리스).

## 레지스트리(CE) 모델 — 실측 확인된 규칙

- CE 의 `extensions/cog/description.yml` 은 **커밋 SHA 를 고정(pin)** 한다.
  우리 main 이 아무리 움직여도 배포본은 불변 — 새 버전 배포는 ref-bump PR 로만.
- 배포는 **CE PR 머지 시 1회**, 그 시점 CE 파이프라인의 DuckDB 버전으로만 빌드된다.
  **부분 배포 없음** (플랫폼 1개라도 실패하면 전체 미배포 — deploy 가 매트릭스
  전체 성공의 하류 단계). **과거 DuckDB 버전 소급 배포 없음**.
- DuckDB 신버전이 나오면 CE 는 전 익스텐션을 새 버전으로 일괄 재빌드한다 —
  소스가 호환되는 한 자동으로 실려간다 (C-API 라 유리).

## 릴리스 체크리스트 (vX.Y.Z)

같이 움직여야 하는 **5총사**: Cargo.toml `version` · `test/sql/cog_version.test`
기대값 · git 태그 · CE `version` 필드 · CE `ref`.

1. **`CHANGELOG.md` 에 릴리스 항목 추가** (Keep a Changelog 형식 — PR 본문 요약,
   Deployed 줄에 CE PR 링크) + `gh release create vX.Y.Z` 로 GitHub Release 발행
2. `Cargo.toml` version bump + `test/sql/cog_version.test` 핀 동기 (버전 문자열이
   테스트 계약이다)
3. 낡은 문구 점검: README 한계 절·`examples/use-cases.ipynb`·docs — 이번 릴리스가
   없앤 한계를 문서가 계속 주장하지 않게
4. `just check` + `just ext-test` 그린 → 릴리스 PR 머지
5. 태그 `vX.Y.Z` 를 머지 커밋에 — **원격 태그 push 는 사람이 직접**
   (`block_danger.py` 가 태그 조작 push 를 막는다)
6. CE 에 "cog: update to vX.Y.Z" PR — `version` + `ref`(태그 커밋 SHA) 두 줄.
   **외부 리포 PR 은 오너 건별 승인 후 제출.**
7. 배포 검증 (머지 후): CDN 200 확인
   (`http://community-extensions.duckdb.org/<duckdb_ver>/<platform>/cog.duckdb_extension.gz`)
   → 실로드 `INSTALL cog FROM community; LOAD cog; SELECT * FROM cog_version();`
   — **검증 전에는 배포 완료를 선언하지 않는다.**

## DuckDB 신버전 대응 (예: 1.5.4 → 1.5.5, PR #46)

DuckDB 릴리스는 우리 CI 도 깨뜨린다 (venv 가 최신 duckdb-python 을 받아
스탬프 불일치). **5핀 일괄 범프**가 처방:

| 핀 | 파일 |
| -- | -- |
| `TARGET_DUCKDB_VERSION` | `Makefile` (템플릿의 공식 버전 노브 — "수정 금지" 대상 아님) |
| `DUCKDB_EXTENSION_MIN_DUCKDB_VERSION` | `justfile` |
| `duckdb_version` | `.github/workflows/MainDistributionPipeline.yml` |
| `duckdb==X.Y.Z` | `pyproject.toml` (오라클 ABI 일치) |
| (재잠금) | `uv.lock` |

범프 후 CE ref 갱신 PR 도 필요하다 (기능 릴리스와 겸해도 됨).

## 함정 기록 (재발 방지)

- 리뷰 중 DuckDB 가 릴리스되면 pinned ref 가 구버전 스탬프로 남아 머지 후 배포가
  깨질 수 있다 — 머지 임박 시점에 CE 파이프라인의 `duckdb_version` 과 우리 ref 의
  타깃이 일치하는지 확인.
- 게이트 명령을 파이프(`| tail`)에 물리면 exit code 가 가려진다.
- `cargo fmt --all` 후 경로 한정 `git add` 는 커밋 누락을 만든다 (add 는 전체로).
