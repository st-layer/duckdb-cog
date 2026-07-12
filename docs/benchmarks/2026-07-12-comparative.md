# 비교 벤치마크: duckdb-cog vs PostGIS Raster vs duckdb-raster vs raquet

|  |  |
| -- | -- |
| **일자/환경** | 2026-07-12 · macOS arm64 (Apple Silicon) 단일 머신 |
| **우리 쪽** | cog **release 빌드**, duckdb-python 1.5.4 (ABI 일치) |
| **duckdb-raster** | community 익스텐션 `raster` (GDAL 기반, `RT_*`) — ahuarte47 계열 설계 |
| **PostGIS** | 16 + postgis_raster 3.4, **docker 컨테이너** (VM 오버헤드 존재) |
| **raquet** | community 익스텐션 `raquet` (읽기) + raquet-io(변환) |
| **데이터** | bench_4096.tif — 4096², uint16, DEFLATE, 256px 타일(=256타일), EPSG:32652, seed 42 (`scripts/bench_compare.py` 참조) |
| **방법** | median of 3~5, cold = 신규 연결/프로세스. 재현: `scripts/bench_compare.py` |

## 결과

| 워크로드 | **cog (우리)** | duckdb-raster (GDAL) | PostGIS | raquet |
| -- | -- | -- | -- | -- |
| **ingest (파일 → 질의 가능)** | **0** (in-place) | 0 (in-place) | 1.39 s (raster2pgsql 256px 타일+GiST) + 클라이언트 도구 설치 필요 | 측정 불가† |
| 콜드 메타데이터 (로컬) | **9.2 ms** | 22.1 ms | n/a (임포트 후에만) | n/a† |
| 타일 인벤토리 (웜, 전 레벨) | **0.4 ms** | 0.9 ms | (임포트된 테이블 count) | n/a† |
| 1,000점 포인트 샘플링 | **20.7 ms** (RS_Value) / 73.6 ms (RS_Values‡) | 31.3 ms *(정합 실패 — 참고치)*§ | 18.3 ms | n/a† |
| zonal mean (1024² 창) | **3.0 ms** | 174.5 ms *(정합 실패 — 참고치)*§ | 53.9 ms | n/a† |
| 원격 콜드 메타 (실 Sentinel-2 B04, https) | 1.6–2.2 s | 2.0 s (진짜 콜드) / **0.03 s (GDAL 프로세스 캐시 히트)**¶ | 임포트 선행 필요 | n/a† |
| **값 정합 (교차 검증)** | zonal mean **PostGIS 와 소수점까지 정확 일치** (32758.54307460785) · RS_Value == rasterio (실데이터 포함) | 포인트/zonal 정합 검증 실패§ | 기준 축 중 하나 | — |

† **raquet 은 비교 축 자체가 다르다**: quadbin(WebMercator) 고정 포맷이라 UTM 래스터는
**재투영+리샘플링(픽셀값 변형) 후 재인코딩**해야 표현 가능 — "in-place 판독" 비교가
성립하지 않는다. 변환 도구(raquet-io)는 GDAL 파이썬 바인딩 요구로 본 환경에서 미가용.
RFC §7 의 기각 사유(3857 락, 재인코딩 모델)가 실측 준비 과정에서 그대로 재확인됨.

‡ **자체 발견**: 이미지 전역에 산개한 1,000점에서 RS_Values(배치)가 RS_Value(스칼라 루프)보다
느리다 (73.6 vs 20.7 ms). 배치는 유일 타일 256개를 병합 fetch 후 전부 디코드해 동시에 들고
가는 반면, 스칼라는 점당 1타일만 디코드한다. 밀집 포인트(동일 타일 다수)에선 배치가 유리
(4,096점 동일 타일 = fetch 1회 계약). → 백로그: 산개-포인트 휴리스틱.

§ RT_* 포인트/zonal 은 문서 부재 속에서 여러 호출 형태를 시도했으나 fill 값 반환·밴드 인덱스
규약 불일치(함수별 0/1-based 혼재)로 **값 정합을 확정하지 못했다**. 수치는 "그 모델의 연산
비용" 참고치로만 싣는다. RT_Read(타일 나열)·메타데이터는 정상 동작해 정식 비교.

¶ GDAL vsicurl 은 프로세스 전역 캐시를 가져 두 번째 접근부터 0.03s — 우리는 연결/청크 단위
dedupe 만 있고 전역 캐시가 없다 (설계 선택이자 백로그). **진짜 콜드는 동급(≈2s)**.

## PostGIS 측정 상세 (재현)

```sh
docker run -d --name pg-bench -e POSTGRES_PASSWORD=bench postgis/postgis:16-3.4
docker exec pg-bench bash -c "apt-get update -qq && apt-get install -y -qq postgis"  # raster2pgsql
docker exec pg-bench psql -U postgres -c "CREATE EXTENSION postgis; CREATE EXTENSION postgis_raster;"
docker cp bench_4096.tif pg-bench:/tmp/bench.tif
docker exec pg-bench bash -c "time (raster2pgsql -s 32652 -t 256x256 -I /tmp/bench.tif public.bench | psql -q -U postgres)"
# 쿼리: ST_Value 조인(1k점) / ST_SummaryStatsAgg(ST_Clip(...)) — \timing, 5회 median
```

## 활용성 판단 (요약)

**"쓸만한가" — 예, 다음 프로파일에서 특히 강하다:**

1. **카탈로그-스케일 탐색/선별**: ingest 0 + 콜드 메타 9ms + STAC 조인 — "수천 파일 중
   어떤 것을 읽을지"를 SQL 로 정하는 단계에서 경쟁 대상이 없다 (PostGIS 는 임포트 선행,
   raquet 은 재인코딩 선행).
2. **영역 통계**: zonal 3.0ms 는 PostGIS(53.9ms, docker 감안해도)와 GDAL ext(174.5ms 참고치)
   대비 큰 차이 — 타일 병합 fetch + 1회 디코드 + 부분 순회 설계의 효과.
3. **정확성 신뢰**: PostGIS·rasterio 와 값이 소수점까지 일치 — 오라클 하네스(T1)의 배당.

**한계 (정직한 표기):**

- 포인트 샘플링 절대치는 PostGIS(18.3ms)가 근소 우위 — 서버가 이미 떠 있고 데이터가
  임포트돼 있다면. "파일 도착 → 첫 답"의 총비용은 우리가 압도 (0 vs 1.4s+α).
- 반복 원격 접근은 GDAL 의 전역 캐시가 유리 (우리 백로그: 전역/연결 캐시).
- 산개-포인트 배치 회귀(‡) 존재.
- 우리는 reader 다: 픽셀 쓰기·재투영·모자이크가 필요하면 PostGIS/GDAL 이 맞다 (N2/N3).
