# duckdb-cog 개발 하네스 가이드 — Claude Code 에이전트 주도 개발

|  |  |
| -- | -- |
| **상태** | Draft v1 |
| **작성일** | 2026-07-05 |
| **대상** | RFC-001 rev.3 (`duckdb-cog`)의 구현 단계 |
| **전제** | Claude Code를 주 개발 도구로 사용, 개발자 1인(손호영) 감독 |

---

## 1. 핵심 원리: 검증 루프가 곧 개발 속도다

에이전트 코딩의 최대 실패 모드는 **Verification Gap** — 에이전트가 검증 전에 "완료"를 선언하는 것이다. 증상은 셋: 테스트 실행 전에 성공 메시지를 출력, 테스트는 돌지만 stderr를 안 읽음, 인수 조건은 E2E인데 유닛만 통과. Anthropic의 하네스 연구에서도 하네스 없는 에이전트는 20분 만에 "완료"를 선언하고 아무것도 동작하지 않았지만, 독립 평가자를 붙이자 같은 모델이 6시간 동안 작동하는 결과물을 냈다.

교훈: **모델을 신뢰하는 게 아니라 루프를 신뢰한다.** 에이전트에게 필요한 것은 (a) 스스로 돌릴 수 있는 명확한 판정 신호, (b) 그 신호를 조작할 수 없다는 보장, (c) 판정 전 종료를 막는 게이트. RFC §6.9의 테스트 전략(오라클 대조, 픽스처 매트릭스, sqllogictest)이 (a)를 제공하고, 이 문서의 하네스가 (b)와 (c)를 제공한다.

duckdb-cog는 에이전트 개발에 유리한 조건을 이미 갖췄다: 정답이 객관적이고(픽셀값은 rasterio와 일치하거나 아니거나), 피드백이 기계적이며(cargo test / sqllogictest exit code), 도메인 지식이 문서화 가능하다(RFC가 존재). 하네스는 이 조건을 실제 루프로 배선하는 일이다.

## 2. 단일 판정 명령: `just check`

에이전트 루프의 피드백 신호는 **명령 하나**로 일원화한다. 여러 명령의 조합은 에이전트가 일부를 빼먹는 원인이 된다.

```makefile
check: lint test-unit test-sql test-oracle        # 로컬 표준 게이트
lint:        ; cargo fmt --check && cargo clippy -- -D warnings
test-unit:   ; cargo test --workspace
test-sql:    ; make -C duckdb-ext test_sqllogic   # DuckDB sqllogictest
test-oracle: ; uv run pytest tests/oracle -x -q   # rasterio 대조 (T1)
fixtures:    ; uv run python scripts/gen_fixtures.py   # T2 결정적 생성
check-full: check test-wasm bench-smoke           # PR 게이트 (CI)
```

원칙:
- **fail fast, 출력은 짧게.** 에이전트 컨텍스트는 유한 자원 — 실패 시 첫 에러가 바로 보이게 `-x`, `head` 활용.
- **빠른 것부터.** lint(초) → unit(수십 초) → sql/oracle(분). 에이전트가 싼 신호로 먼저 교정하게.
- **`check`는 어떤 상태에서도 실행 가능해야 한다.** 픽스처가 없으면 자동 생성하도록 의존성 배선.

## 3. CLAUDE.md — 짧고, 불변식 중심으로

CLAUDE.md는 모든 세션이 읽는 상주 메모리다. 커뮤니티 컨센서스는 **~60줄 수준으로 짧게** 유지하고, 세부 규칙은 `.claude/rules/*.md`로 분리해 path glob으로 관련 파일 작업 시에만 로드시키는 것. 뻔한 내용("좋은 코드를 짜라")은 넣지 않는다 — 모델의 기본 행동을 **바꿔야 하는 것**만 넣는다.

duckdb-cog의 CLAUDE.md 골자:

```markdown
# duckdb-cog

GDAL-free COG reader for DuckDB. 설계 준거: docs/RFC-001-rev3.md (필독).

## 판정
- 완료 판정은 `just check` 통과가 유일한 기준. 통과 전 완료 선언 금지.
- 테스트 실패 시 tests/ 를 고쳐서 통과시키지 말 것 — 테스트가 계약이다.
  테스트 자체가 틀렸다고 판단되면 수정 대신 이유를 보고하고 승인을 받아라.

## 아키텍처 불변식 (위반 = 즉시 실패)
- 읽기 경로(엔진/익스텐션 크레이트)에 GDAL/PROJ/GEOS 링크 금지 (RFC N4).
  GDAL은 tests/, scripts/ 의 dev-dependency로만 허용.
- decode/fetch는 async-tiff에 위임 — TIFF 파싱 재구현 금지 (RFC N7).
  async-tiff 커버리지 공백 발견 시: 우회 구현 대신 이슈로 보고.
- 읽기 시점 reproject 금지 (RFC N2). 픽셀값 변형 없는 경로만.
- 엔진 크레이트는 wasm32-unknown-unknown 컴파일 가능 상태 유지 (RFC G8).
- async-tiff 직접 호출은 src/reader_boundary.rs 의 trait 뒤에서만 (RFC R8).

## SQL 표면
- 스칼라 함수는 Sedona RS_* 규약 (RFC §6.8): 1-based 밴드, nodata→NULL,
  GDAL 순서 geotransform. 새 함수 전 docs/sedona-semantics.md 확인.

## 명령
- 빌드: make build / 테스트: just check / 픽스처 재생성: make fixtures
- DuckDB 로컬 로드 확인: make smoke
```

포인트: RFC의 비목표(N-시리즈)가 그대로 에이전트의 불변식이 된다 — **RFC를 잘 쓴 프로젝트는 CLAUDE.md가 거의 공짜다.**

## 4. 결정적 게이트: hooks

CLAUDE.md 지시는 대략 70% 수준으로 준수되지만 hook은 100% 강제된다는 것이 커뮤니티의 실측 결론이다. 지시로 하는 통제와 코드로 하는 통제를 구분하라: "테스트를 수정하지 마라"는 지시이고, tests/ 쓰기를 차단하는 PreToolUse hook은 물리 법칙이다.

duckdb-cog에 배선할 최소 hook 세트 (`.claude/settings.json` + `.claude/hooks/`):

| Hook | 이벤트 | 동작 |
| -- | -- | -- |
| `freeze-tests` | PreToolUse (Edit/Write) | 구현 태스크 중 `tests/oracle/**`, `test/sql/**` 쓰기 차단 (exit 2). 테스트 작성 태스크에서만 환경변수로 해제 |
| `guard-deps` | PreToolUse (Edit/Write on Cargo.toml) | `gdal`, `proj-sys`, `geos` 문자열이 dev-dependencies 밖에 추가되면 차단 — N4를 물리적으로 강제 |
| `fmt-on-write` | PostToolUse (Edit/Write on *.rs) | `cargo fmt` 자동 적용 — 스타일 지적에 컨텍스트 낭비 방지 |
| `test-on-stop` | Stop | `just check` 실패 시 턴 종료 차단 → 에이전트가 스스로 수정 루프 계속 (8회 연속 차단 시 시스템이 해제하므로 무한루프 없음) |
| `block-danger` | PreToolUse (Bash) | `git push --force`, `rm -rf`, main 직접 commit/push/merge 차단 (GitHub Flow 가드레일 — 완전 봉쇄는 GitHub 브랜치 보호) |

`test-on-stop`이 이 하네스의 심장이다 — "완료 선언은 just check 통과 후"라는 규칙이 지시가 아니라 게이트가 된다.

## 5. 워크플로: Research → Plan → Execute → Review → Ship

현재 커뮤니티 표준 패턴이자 Claude Code 기능이 각 단계에 정확히 대응하는 루프:

1. **Research.** 새 영역(예: DuckDB C-API의 pushdown 인터페이스, async-tiff의 planar 처리) 착수 시 read-only 탐색을 먼저. 필요하면 Explore 서브에이전트로 — 본 컨텍스트를 어지럽히지 않는다.
2. **Plan.** 다중 파일 작업은 반드시 plan mode로 계획을 먼저 받고 승인 후 실행. 계획 없는 실행은 프로토타입 브랜치에서만.
3. **Execute.** 아래 §6의 태스크 단위 규칙대로. 사이드퀘스트(예: "픽스처 생성기가 zstd를 지원 안 하네")는 서브에이전트나 별도 이슈로 격리 — 본 태스크의 WIP=1 유지.
4. **Review.** **만든 에이전트가 채점하지 않는다.** 별도 컨텍스트의 리뷰어 서브에이전트(또는 새 세션)가 diff를 검토 — 같은 모델이라도 컨텍스트가 분리되면 자기 작업에 관대해지는 편향이 사라진다. duckdb-cog 전용 리뷰어 프롬프트: "RFC 불변식(N2/N4/N7/G8) 위반, 오라클 테스트 우회, unsafe 블록, 에러 삼킴(silent fallback)을 찾아라."
5. **Ship.** PR → CI에서 `check-full`(WASM 스모크 + 벤치 회귀 포함) → 머지.

## 6. 태스크 설계: vertical slice + 테스트 선행

**수직 슬라이스(tracer bullet)로 자른다.** AI는 수평 페이징(스키마 전부 → 함수 전부 → 테스트 전부)으로 기본 설정되는데, 이러면 end-to-end 피드백이 마지막까지 지연된다. 대신 "픽스처 1개 → read_cog가 그 파일의 메타데이터 행 반환 → sqllogictest 1개 통과"처럼 모든 층을 관통하는 최소 슬라이스로.

**태스크당 TDD 시퀀스 (Anthropic 권장 순서):**
1. 테스트 먼저 작성 (오라클 대조 + sqllogictest) — 필요 시 테스트 작성 전용 세션/태스크로 분리
2. 실패 확인 ("전부 실패해야 정상")
3. **실패하는 테스트를 커밋** — 체크포인트이자 계약 고정
4. 구현 (이 시점부터 freeze-tests hook 활성)
5. `just check` 그린 → 리뷰 → 커밋

테스트가 없으면 에이전트의 유일한 검증 수단은 자기 판단인데, 이는 컨텍스트가 차오를수록 열화된다. 테스트는 세션이 얼마나 길어지든 정확도가 유지되는 **외부 오라클**이다 — 에이전트 코딩에서 TDD가 단일 최강 패턴으로 꼽히는 이유.

## 7. duckdb-cog 특화 장치

**오라클의 오라클 문제.** T1의 전제는 "rasterio가 옳다"인데, GDAL도 버그가 있다. 판정 불일치 발견 시 에이전트가 임의로 어느 쪽에 맞추지 않도록: 불일치는 `docs/oracle-disputes.md`에 기록하고 사람 판정을 기다린다는 규칙을 CLAUDE.md에 명시.

**픽스처는 결정적으로.** `make fixtures`는 seed 고정으로 항상 동일 바이트를 생성 — 에이전트가 "픽스처를 다시 만들었더니 통과했다"는 우회로를 갖지 못하게. 픽스처 해시를 lock 파일로 커밋.

**skill로 도메인 지식 캡슐화.** 반복 참조되는 전문 지식은 `.claude/skills/`로: `cog-anatomy`(IFD/오버뷰/sparse 구조 요약 + 헥스 덤프 판독법), `duckdb-ext-dev`(table function 등록, pushdown 인터페이스, sqllogictest 작성법), `sedona-semantics`(RS_ 함수별 시그니처·NULL 규약 표). 스킬은 파일이 아니라 폴더로 — 참조 자료는 `references/`에, 에이전트의 실수가 발견될 때마다 Gotchas 섹션에 축적.

**병렬화는 worktree로.** RS_ 함수들은 상호 독립이라 병렬 개발에 적합 — git worktree로 세션당 격리된 체크아웃을 주면 편집 충돌 없이 동시 진행 가능. 단 공유 파일(register.rs 등) 충돌은 사람이 머지에서 조정.

**CI에서의 headless 활용(선택).** `claude -p` 비대화 모드로 "실패한 CI 로그 요약 + 원인 후보 3개" 같은 진단 태스크를 파이프라인에 넣을 수 있다 — 단 CI에서의 자율 수정은 v1에서는 하지 않는다(검토 부하 대비 이득 불명).

## 8. 안티패턴 (하지 말 것)

- **마이크로매니징.** 구현 방법을 한 줄씩 지시하지 말 것 — 목표·제약·판정 기준을 주고 방법은 위임. "버그를 붙여넣고 fix라고 말하라"가 커뮤니티의 실측 권고.
- **CLAUDE.md 비대화.** 모든 팁을 다 넣으면 신호가 노이즈에 묻힌다. 60줄 목표, 초과분은 rules/skills로.
- **hook으로 할 일을 지시로 하기.** 결정적으로 강제 가능한 것(포맷, 금지 의존성, 테스트 게이트)은 전부 hook/settings로 내리고, CLAUDE.md에는 판단이 필요한 것만.
- **긴 세션 방치.** 태스크 완료 시 요약을 문서로 남기고(`docs/worklog/`) 새 세션으로 — 컨텍스트 부패(context rot)의 비용이 재시작 비용보다 크다.
- **에이전트가 만든 것을 에이전트 본인이 승인.** Review 단계 생략 금지.

## 9. 부트스트랩 순서 (Phase 0~1 착수 시)

1. 리포 뼈대: workspace(engine 크레이트 + duckdb-ext) + Makefile + CI
2. `make fixtures` — 합성 픽스처 생성기 (T2)
3. `make test-oracle` — rasterio 하네스 뼈대 + 첫 대조 테스트 (T1)
4. sqllogictest 배선 (T3)
5. `.claude/` — CLAUDE.md, hooks 5종, skills 3종
6. 여기까지가 "하네스 완성" — **이후에야 첫 기능(read_cog 메타데이터 슬라이스) 착수**

하네스가 기능보다 먼저다. RFC Phase 1의 "테스트 인프라 선행" 조항과 동일한 원칙의 도구 버전이다.
