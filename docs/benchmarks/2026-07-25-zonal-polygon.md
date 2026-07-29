# RS_ZonalStats polygon zone 성능 실측 — bbox 대비 PIP 오버헤드 (#48)

|  |  |
| -- | -- |
| **일자/환경** | 2026-07-25 · macOS arm64 (Apple M4 Pro) 단일 머신 |
| **대상** | 엔진: criterion release (`just bench`) · SQL: cog **release 빌드** + duckdb-python 1.5.5 |
| **데이터** | `basic_512x512_u16.tif` (10 m 픽셀, 256px 타일, 로컬) |
| **워크로드** | 필지형: seed 20260725 무작위 notch 폴리곤 200개 (40–64 px, 9꼭짓점) · 대형: P1 (오목+구멍, envelope 84k px 4타일, 유효 62.4k px, 15변) |
| **재현** | 엔진 `just bench` · SQL 스니펫은 본 문서 하단 워크로드 정의와 동일 (webm 없음 — 인라인 duckdb-python) |

## 결론

- **필지 규모 end-to-end: 폴리곤 = bbox 의 1.95×** (필지당 21 → 41 µs). 필지
  200개 집계가 8.2 ms — 단일 스레드 ~24k 필지/s. 실사용 목적(필지 경계 통계)에
  충분하고, envelope 오염 없는 정확한 값을 얻는 대가로 합리적.
- **spatial GEOMETRY 브리지 (`ST_AsText(geom)`) 비용은 무시 가능** (+0.4 ms/200필지,
  필지당 +2 µs). WKT 파싱 자체도 1.1 µs (P1, 15꼭짓점) + 청크-로컬 dedupe.
- **대형 zone 은 PIP 가 지배**: 인메모리 엔진 마이크로에서 84k px 윈도 기준
  bbox 178 µs vs 폴리곤 1.05 ms (**5.9×**) — 워크로그의 스캔라인 트리거(2×)를
  대형 zone 에서 초과. 필지 규모에선 1.95×로 경계선. → 스캔라인(행별 교차 구간)
  최적화는 **대형 zone(행정구역·유역 급) 수요가 실측되면** 착수 (후속 유지).

## 엔진 (criterion, release, 인메모리 MemorySource)

| 벤치 | median | 대비 |
| -- | -- | -- |
| `warm_zonal_100x100` (bbox, 10k px, 1타일) | 24.2 µs | 기준 |
| `warm_zonal_polygon_rect_100x100` (같은 윈도 사각 폴리곤, 5변) | 72.4 µs | **3.0×** — 순수 PIP 오버헤드 분리 (같은 픽셀 집합) |
| `warm_zonal_bbox_p1_envelope` (84k px, 4타일) | 177.9 µs | 기준 |
| `warm_zonal_polygon_p1` (62.4k px 유효, 15변+구멍) | 1.049 ms | **5.9×** |
| `parse_zone_wkt_p1` (WKT 15꼭짓점 파싱) | 1.08 µs | — |

픽셀당 PIP 비용 ≈ 0.7–1 ns × 변 수 (rect: 10k px×5변 → +48 µs; P1: 84k px×15변 → +0.87 ms). naive O(윈도 픽셀 × 변 수) 모델과 정합.

## SQL end-to-end (release 익스텐션, duckdb-python, 웜, median)

| 워크로드 (200 필지/쿼리) | median | 필지당 |
| -- | -- | -- |
| bbox envelope (`DOUBLE[4]`) | 4.2 ms | 21 µs |
| **polygon WKT (`VARCHAR`)** | 8.2 ms | 41 µs (**1.95×**) |
| spatial 브리지 `ST_AsText(ST_GeomFromText(wkt))` | 8.6 ms | +2 µs — 브리지 무시 가능 |
| 동일 폴리곤 ×200 (파싱 dedupe 경로) | 5.5 ms | 28 µs |

| 대형 zone 1건 (단건 쿼리 ×20) | median |
| -- | -- |
| bbox envelope (84k px) | 0.34 ms |
| polygon P1 (62.4k px 유효) | 1.23 ms (**3.6×**) |

주: debug 빌드로 재면 절대치가 ~60× 부풀려진다 (350 ms/200필지) — 비교는 반드시
release. 첫 측정에서 이걸로 오판할 뻔했음.

## 후속 판단 기준 (워크로그와 동기화)

- 필지(≲10³ px, ≲10² 변) 워크로드: **현행 naive PIP 유지** — 1.95×, 절대치 41 µs.
- 스캔라인 최적화 착수 트리거: 대형 zone(≳50k px 또는 ≳10³ 변) 워크로드가 실사용에
  등장하거나, CI 기준선(T7 후속)에서 `warm_zonal_polygon_p1` 회귀 시.
