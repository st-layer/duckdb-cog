# 2026-08-26 — 5년 NDVI 시계열 조널 평균 (GeoJSON AOI 1개, 실계열 규모)

## 결론

**GeoJSON AOI 1개(김제평야 ~1.3 km², 14,253px@10m) × 5년(2021–2025) Sentinel-2
L2A 실카탈로그(earth-search) 931씬**의 NDVI 조널 평균 시계열, 콜드 프로세스,
한국 → us-west-2:

| 경로 | wall | ms/씬 | 유효 ms/read | 의미 규약 |
|---|---|---|---|---|
| STAC 검색 (`read_stac_search`, 5페이지) | 8.4 s | — | — | 931 items / 372 날짜 |
| **A: 밴드평균 NDVI** — `RS_ZonalStats` mean ×2밴드, cap 16 | **798.5 / 819.5 s** | 858–880 | 429–440 | 평균의 NDVI (근사) |
| A + cap 64 (`COG_FETCH_CONCURRENCY=64`) | 560.7 s | 602 | 301 | 값은 cap 16 과 완전 동일 |
| **A-fast** — `max_pixels := 4096` (#71 오버뷰) | **510.3 s** | 548 | 274 | 평균의 NDVI, 오버뷰 근사 |
| **B: 픽셀단위 NDVI** — `RS_BandAsArray` 폴리곤존 ×2밴드 | **1766.7 s** | 1898 | 949 | **NDVI 의 평균 (정답 규약)** |

엔드투엔드(검색 포함): 정답 규약(B) **≈ 29.6분**, 밴드평균 근사(A) ≈ 13.5분
(cap 64 면 9.5분), 오버뷰 근사(A-fast) ≈ 8.6분. 931씬 전 행 무결(에러·NULL 0), 값 정상
(겨울 0.12 → 벼 생육기 0.7+ 시즌 사이클).

이전 벤치와의 연속성: A 의 유효 429 ms/read 는 2026-08-06 청크 동시화
벤치(22 read, 436–590 ms/read)와 일치 — **85× 규모에서 열화 없음.**

## 워크로드 정의

- AOI: `scripts/data/bench_aoi_gimje.geojson` (EPSG:4326 Polygon 6꼭짓점).
  AOI 가 S2 타일 중첩부(52SBE/52SCE)에 걸쳐 날짜당 2씬 — dedupe 없이 전량 계산.
- 기간: `2021-01-01T00:00:00Z/2025-12-31T23:59:59Z` (5년).
- 씬: earth-search `sentinel-2-l2a`, bbox+datetime 검색. 구름 필터 없음
  (`read_stac_search` 는 eo:cloud_cover 미노출).
- NDVI: raw DN `(B08-B04)/(B08+B04)` (BOA offset 미보정 — 성능에 무관).
- 측정: 경로마다 콜드 프로세스(타일캐시·커넥션 풀 공유 배제), 씬 목록은
  검색 1회 후 parquet 고정. 재현: `uv run python scripts/bench_ndvi_timeseries.py all`.

## 관찰

1. **픽셀단위(정답) 경로가 밴드평균의 2.2× 느리다 — 동시화 공백.**
   `RS_ZonalStats` 는 청크 행 동시(#74, cap 16)인데 `RS_BandAsArray` 는 행
   순차라 949 ms/read 를 그대로 지불한다. B 를 A 수준으로 당기려면
   BandAsArray 에 같은 `run_rows` 패턴을 적용하면 된다 (후속 이슈 후보).
2. **cap 16 의 동시화 이득이 순차 대비 2.2× 에 그침** — 16× 캡을 못 채운다.
   cap 64 로 올리면 798.5 → 560.7 s (1.42×, 값 동일): 캡 4× 에 1.4× 라
   **부분적 대역폭 바운드**. 기본 16 은 보수적 — 대형 시계열은
   `COG_FETCH_CONCURRENCY` 상향 가치가 실측으로 확인됐다 (2026-08-06 벤치의
   "cap 상향 여지" 후속 확증).
3. **fast mode(#71)는 -36%**: 오버뷰 1레벨(20m)로 14,253px → ~3.6k px.
   NDVI 절대 편차 0.0002–0.0006 (아래 값 규약 참고).
4. **의미 규약 차이가 계절 의존**: 밴드평균 NDVI vs 픽셀단위 NDVI 가
   여름(0.269 vs 0.273, ~1.5%)엔 근접하지만 겨울 저 NDVI 에선
   0.1231 vs 0.1489 (**상대 17%**)로 벌어진다. 시계열의 절대값이 중요한
   분석이면 B 규약을 써야 한다.
5. **실카탈로그의 오염**: 5년 범위에서 2022-01-07 두 아이템의 red/nir 이
   COG 가 아니라 JP2 원본(`s3://sentinel-s2-l2a/...jp2`)을 가리킴 —
   href/media-type 로 COG 만 걸러야 한다 (스크립트는 `.tif` 필터).
   또한 `s3://` 가 아닌 `https://sentinel-cogs...` 도 object_store 가 S3 로
   인식하므로 공개 버킷은 `AWS_SKIP_SIGNATURE=true` 없이는 IMDS 를 찾다 죽는다.

## 측정 환경

- 한국 주거망 → us-west-2, v0.4.0 릴리스 빌드(`make release`,
  chore/release-v0.4.0 = main+버전범프), duckdb Python 1.5.5.
- 씬 목록·존 WKT 는 `/tmp/cogbench-ndvi/{scenes,zones}.parquet` 에 고정.
  AOI 의 UTM 투영은 href 의 MGRS zone 파싱 → rasterio.warp (스크립트 한정,
  RFC N4 위반 아님).
