# duckdb-cog 소개 — 위성영상을 SQL로 다루는 법

> 팀원 온보딩용 문서입니다. 영어 공식 문서는 [README](../README.md), 설계 배경은
> [RFC-001](RFC-001-rev3.md) 참조.

## 한 줄 요약

**클라우드에 있는 위성영상(COG)을 다운로드 없이, GDAL 없이, SQL로 바로 읽는 DuckDB 익스텐션.**

```sql
INSTALL cog FROM community;  LOAD cog;

SELECT RS_Value('https://…/S2B_52SCG_20260630_0_L2A/B04.tif', 325210.0, 4121100.0);
-- 0.8초 뒤: 1748.0  (120MB 파일에서 필요한 몇 KB만 읽어서)
```

## 왜 만들었나

위성영상으로 뭔가 하려면 보통 이런 여정을 거칩니다:

1. 씬을 통째로 다운로드 (수백 MB × 수십 씬)
2. GDAL 스택 설치 (무겁고, 버전 지옥)
3. DB에 쓰려면 임포트/재인코딩 (PostGIS raster2pgsql 등)
4. 그제서야 첫 질의

**duckdb-cog는 이 여정을 "SQL 한 줄"로 바꿉니다.** 비결은 COG(Cloud-Optimized
GeoTIFF)라는 포맷 자체에 있어요 — 내부가 타일로 조각나 있고 메타데이터가 앞쪽에
모여 있어서, HTTP Range 요청으로 **필요한 조각만** 집어올 수 있습니다. 우리는 그걸
DuckDB 테이블 함수/스칼라 함수로 노출한 겁니다.

세 가지 설계 원칙(전부 테스트로 강제됨):

| 원칙 | 의미 |
| -- | -- |
| **in-place** | 다운로드·임포트·재인코딩 없음. 파일이 있는 곳에서 읽는다 |
| **픽셀 무변형** | 재투영·리샘플링·보간 없음. 픽셀값은 과학적 측정값이므로 저장된 그대로 |
| **GDAL-free** | 읽기 경로에 GDAL/PROJ/GEOS 링크 없음 (가볍고, WASM까지 컴파일됨). GDAL은 테스트 오라클로만 |

그리고 신뢰의 근거: **모든 픽셀 관련 결과는 CI에서 rasterio와 교차 검증**됩니다.
zonal 평균은 PostGIS와 소수점까지 일치하는 걸 확인했어요.

## 뭘 할 수 있나 — 유즈케이스 4종

### 1. 원격 COG 탐색 (다운로드 0)

```sql
-- 타일 그리드, 오버뷰 레벨, extent, CRS — 메타데이터만 몇 번의 range-read 로
SELECT level, tile_x, tile_y, bbox, crs FROM read_cog('https://…/B04.tif');

-- Sedona 스타일 접근자
SELECT RS_Width(f), RS_Height(f), RS_SRID(f), RS_MetaData(f)
FROM (SELECT 'https://…/B04.tif' AS f);
```

### 2. 픽셀값·영역 통계

```sql
-- 한 점의 값 (좌표는 래스터의 네이티브 CRS — 예: UTM)
SELECT RS_Value('https://…/B04.tif', 325210.0, 4121100.0);

-- bbox 영역 평균/합/최소/최대 — 걸치는 타일만 fetch+decode
SELECT RS_ZonalStats('https://…/B04.tif', [322000, 4119000, 326400, 4123200], 1, 'mean');

-- 배치 포인트 샘플링 (같은 타일은 한 번만 읽음 — 1,000점 산개도 7ms)
SELECT RS_Values('https://…/B04.tif', [x1, x2, …], [y1, y2, …]);
```

### 3. 라이브 STAC 카탈로그 검색

```sql
-- Earth Search 같은 STAC API 를 POST /search + 페이지네이션까지 알아서
SELECT item_id, datetime, href
FROM read_stac_search('https://earth-search.aws.element84.com/v1/search',
                      collections := ['sentinel-2-l2a'],
                      bbox := [127.01, 37.20, 127.06, 37.24],      -- 검색은 위경도 (STAC 표준)
                      datetime := '2026-06-01T00:00:00Z/2026-07-13T00:00:00Z')
WHERE asset_key = 'red';
```

### 4. 밭 경계 시계열 — SQL 한 방 (킬러 유즈케이스)

"위경도 GeoJSON 폴리곤을 주면 그 영역의 반사율 시계열을 달라" — DuckDB `spatial`
익스텐션과 조합하면 검색→재투영→집계가 쿼리 하나로 끝납니다:

```sql
LOAD cog; LOAD spatial;

WITH scenes AS (
  SELECT datetime, href FROM read_stac_search('https://…/v1/search',
    collections := ['sentinel-2-l2a'], bbox := [127.01, 37.20, 127.06, 37.24],
    datetime := '2026-06-01T00:00:00Z/2026-07-13T00:00:00Z')
  WHERE asset_key = 'red'
),
zoned AS (  -- 씬별 네이티브 CRS 로 GeoJSON 재투영 (RS_SRID 가 CRS 를 행별 공급)
  SELECT datetime, href,
         ST_Transform(ST_GeomFromGeoJSON('…'), 'EPSG:4326',
                      'EPSG:' || RS_SRID(href), always_xy := true) AS g
  FROM scenes
)
SELECT CAST(datetime[:10] AS DATE) AS date,
       round(RS_ZonalStats(href, [ST_XMin(g), ST_YMin(g), ST_XMax(g), ST_YMax(g)], 1, 'mean'), 1) AS mean_red
FROM zoned ORDER BY date;
```

6주치 9개 씬 → 씬당 1~2초, 다운로드 0. 결과에서 장마 구름까지 그대로 보입니다
(맑은 날 ~1300–1900, 구름 낀 날 ~7500–10300). 실행 결과가 담긴 노트북:
[`examples/use-cases.ipynb`](../examples/use-cases.ipynb).

## 시작하기

```sh
# 등록 완료 후 (어디서나):
duckdb -c "INSTALL cog FROM community; LOAD cog; SELECT * FROM cog_version();"

# 지금 (개발 체크아웃):
just setup && just ext          # 최초 1회 + 빌드
duckdb -unsigned -c "LOAD 'build/debug/cog.duckdb_extension'; …"

# 노트북으로 체험 (제일 추천):
cd ~/personal/projects/duckdb-cog
uv run --with jupyterlab --with matplotlib --with pandas jupyter lab examples/use-cases.ipynb
```

## 성능 감각 (Apple Silicon, release 빌드 실측)

| 워크로드 | 시간 |
| -- | -- |
| 콜드 메타 → 첫 답 (로컬 4096² COG) | 3.8 ms |
| 1,000점 샘플링 (배치) | 8.0 ms |
| zonal mean (1024² 창) | 2.9 ms |
| **원격** 콜드 메타 (실 Sentinel-2) | 0.84 s |
| **원격** 반복 접근 (프로세스 캐시) | **0.4 ms** |

비교 맥락: PostGIS는 같은 파일에 raster2pgsql 임포트 1.4초+α가 선행돼야 하고,
zonal은 우리가 ~18배 빠릅니다. 상세: [benchmarks/](benchmarks/).

## ⚠️ 함정 목록 (전부 실측으로 확인된 것)

| 함정 | 증상 | 처방 |
| -- | -- | -- |
| 공개 S3 버킷 익명 접근 | 질의가 하염없이 멈춤 (EC2 메타데이터 폴링) | `AWS_SKIP_SIGNATURE=true` 환경변수를 **첫 질의 전에** |
| RS_* 에 위경도 좌표 | **에러 없이 빈 결과** (count=0, NULL) | 좌표는 네이티브 CRS — `RS_SRID()`로 확인 후 재투영 |
| `ST_Transform` 축 순서 | `POINT (inf inf)` → 빈 결과 | EPSG:4326 입력엔 `always_xy := true` 필수 |
| STAC datetime | HTTP 400 | RFC3339 전체 형식 (`2026-06-01T00:00:00Z/…`) — 날짜만은 거부됨 |
| 일반(스트립) GeoTIFF | `IFD 0 is not tiled` 에러 | 타일드만 지원 — `gdal_translate -of COG` 로 변환 |
| 원격 캐시 신선도 | TTL 60초 내 서버측 변경 안 보임 | `COG_REMOTE_CACHE_TTL_S` 로 조정 (0=끔) |

## 한계 — 정직하게

- **reader입니다.** 픽셀 쓰기·재투영·모자이크·리샘플링이 필요하면 GDAL/PostGIS가 맞습니다 (설계상 범위 밖).
- **폴리곤 zonal은 외접 bbox 근사**입니다. 픽셀 단위 폴리곤 클리핑은 GEOS가 필요해서 안 합니다.
- 스트립 기반 GeoTIFF는 못 읽습니다 (타일드/COG 전용 — 명시적 에러).

## 언제 뭘 쓰나 (팀 가이드)

| 상황 | 도구 |
| -- | -- |
| "수천 씬 중 어떤 걸 읽을지" 고르기, 탐색적 분석, 시계열 추출 | **duckdb-cog** — 경쟁자 없음 |
| 파이프라인에서 픽셀 가공 (재투영·밴드 연산·저장) | GDAL/rasterio |
| 이미 임포트된 래스터에 반복 서빙 | PostGIS |
| 노트북에서 빠르게 "이 밭 NDVI 어때?" | **duckdb-cog** + spatial |

## 더 보기

- [README](../README.md) — 공식 문서 (영어)
- [examples/use-cases.ipynb](../examples/use-cases.ipynb) — 실행된 노트북
- [RFC-001-rev3.md](RFC-001-rev3.md) — 설계 결정 전체
- [benchmarks/](benchmarks/) — 성능·비교·I/O 경로 실측 보고서
- 등록 현황: [duckdb/community-extensions#2274](https://github.com/duckdb/community-extensions/pull/2274)
