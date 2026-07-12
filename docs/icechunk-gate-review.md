# Icechunk 재검토 게이트 (RFC §6.3 / Phase 2) — 2026-07-12 검토 기록

|  |  |
| -- | -- |
| **상태** | 검토 완료 — **유예 유지 권고** (채택 결정은 사람 승인 대상) |
| **트리거** | RFC Phase 2 명시 게이트: "Icechunk 재검토" |

## 유예 근거의 현재 유효성 (§6.3 N8 대비 점검)

RFC 가 유예 조건으로 둔 "재도입 검토 시점 = 시계열 스냅샷/버저닝 수요 실증" 관점에서:

1. **lazy fetch 는 이미 충분히 확보됨** — async-tiff + ByteSource 경계 위에서
   메타 나열 1~2 fetch(T5 계약), 타일 병합 요청(fetch_tiles), EOF 클램프까지
   계약 테스트로 고정. virtual chunk reference 계층이 얹을 IO 가치가 v1 대비
   더 줄었다.
2. **시계열 접근은 STAC 레벨에서 해소 중** — read_stac(datetime 컬럼) +
   raster:bands 통계로 "어느 시점의 어느 자산"의 카탈로그 질의가 SQL 로 가능.
   스냅샷/버저닝의 실수요 신호는 아직 없다.
3. **표면 비용은 그대로 유효** — Icechunk 도입 시 빌드·WASM(G8)·감사 표면 증가.
   특히 engine 의 wasm32 유지 계약과의 상호작용을 재검증해야 한다.

## 권고

**유예 유지.** 다음 신호가 실증되면 재검토를 재개한다:

- 사용자가 "같은 지역의 시점 간 diff/롤백" 류 질의를 STAC 시계열로 풀 수 없다는
  구체 사례 (STAC 는 카탈로그 버전이지 픽셀 버전이 아님)
- Icechunk virtual dataset 이 가리키는 COG 집합을 read_cog 로 직접 여는 수요
  (= Icechunk 를 "카탈로그"로 취급하는 read_icechunk 유스케이스)

재개 시 진입 슬라이스: read_stac 과 동형의 `read_icechunk(store)` 스파이크
(store 메타만, 픽셀 미접촉) 로 비용/가치를 실측.

— 본 문서는 게이트 통과 기록이며, 채택/기각의 최종 결정은 리포 소유자 승인 사항.
