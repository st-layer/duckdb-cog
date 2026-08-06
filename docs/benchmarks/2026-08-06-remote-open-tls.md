# 2026-08-06 — 원격 open 지연: per-open TLS 제거 (#72)

## 결론

원격 COG open 이 느렸던 원인은 이슈 #72 의 추정(헤더를 ~5회 순차 read)이
아니라 **open 마다 새 reqwest Client(빈 커넥션 풀)를 만들어 DNS+TCP+TLS 를
새로 지불**한 것. origin 단위 store 공유로 수정 후, 11씬×2밴드(22 원격
read, Sentinel-2 L2A us-west-2, zonal mean 1폴리곤) 워크로드:

| | wall | per COG read |
|---|---|---|
| before (main 1be5b19) | 51.9 / 51.5 / 51.7 s | 2.35 s |
| **after (store 공유)** | **29.3 / 34.2 s** | **1.33–1.56 s** |
| rasterio 1.4.3 / GDAL 3.9.3 | 25.4 / 24.2 s | 1.10 s |

값은 수정 전후·rasterio 와 자릿수까지 동일 (2025-04-01 red mean
889.1461405270356, NDVI 0.40628343…) — 순수 지연 개선.

## 진단 근거 (재현 절차 포함)

1. **메타데이터 워크는 이미 최적**: 실제 S2 B04.tif 앞 2MB 를 받아
   (`curl -r 0-2097151`) 계측 ByteSource 로 `read_cog_meta` 실행 →
   **fetch 1회(0..32768), 5레벨 전부 파싱**. async-tiff
   `ReadaheadMetadataCache`(32KiB 초기, 배증) 뒤에서 원격 open 은 RTT 1회다.
   → 이 계약은 `crates/engine/tests/open_rtt.rs` 가 상시 가드.
2. **범인은 per-open Client**: `ObjectStoreSource::open` 이 URL 마다
   `parse_url_opts` 로 새 store 를 만들고, object_store 0.13 은 store 마다
   새 reqwest Client 를 만든다 (http/mod.rs:283). 이슈 실측과 정합 —
   fresh 연결 0.55–0.73s + GET 0.18s ≈ open 1.03s, 서로 다른 22개 객체에서
   균일(= 풀 재사용 없음), 타일 단계는 같은 store 재사용이라 예산 근접.

## 남은 갭 (후속 관찰)

after 1.33–1.56s/COG vs GDAL 1.10s — 이슈의 2차 관찰(타일 fetch+decode
1.32s vs 네트워크+decode 예산 ~0.72s)이 이제 마스킹 없이 드러난 상태.
별도 측정·이슈로 다룬다.

## 측정 환경

- 한국 → us-west-2, 단일 스트림 ~1.3MB/s, warm RTT ~0.18s
- release 빌드 (`make release`), duckdb CLI v1.5.5, `AWS_SKIP_SIGNATURE=true`
- 씬 목록·SQL 은 #72 본문 기준 (11씬 고정 리스트, `RS_ZonalStats(url, wkt,
  1, 'mean')` × red/nir) — 콜드 프로세스, 2회 반복
