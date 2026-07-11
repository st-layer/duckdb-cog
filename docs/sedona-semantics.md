# Sedona RS_* 의미론 스냅샷 — duckdb-cog 구현 준거

RFC §6.8/R10: Apache Sedona `RS_*` 카탈로그는 **참조이지 계약이 아니다**.
착수 시점(2026-07) 의미론을 여기 고정하고 이후 Sedona 드리프트는 추적하지 않는다.
준거와 다르게 구현한 지점은 각 함수의 "차이" 열에 명시한다.

## 공통 규약 (Sedona 에서 그대로 가져옴)

- 밴드 인덱스는 **1-based**.
- 범위 밖 밴드·nodata 부재·NULL 인자 → 에러가 아니라 **NULL**.
- geotransform 은 GDAL 순서: scaleX, skewY, skewX, scaleY, upperLeftX, upperLeftY.

## duckdb-cog 공통 차이

- **raster 인자 = COG 경로 문자열(VARCHAR).** Sedona 는 raster 타입 객체를 받지만
  우리는 raster 타입이 없다 — read_cog() 와 동일한 경로 문자열이 파일 식별자다.
  함수 호출마다 IFD 메타데이터만 range-read 한다 (픽셀 미접촉, 청크 내 경로 dedupe).
- 없는 파일·비 COG 경로는 NULL 이 아니라 **에러** (경로 포함) — bind 실패와 동급.
- skew: ModelPixelScale+ModelTiepoint 경로만 지원하므로 georef 가 있으면 항상 0.0.
  (회전 ModelTransformation 만 있는 파일은 georef 자체가 없음 — georef 파생 함수 NULL.)
- INTEGER 반환 함수(Width/Height/NumBands/SRID)는 i32 상한(2³¹−1) 초과 값을
  NULL 로 강등한다 — 현실적 COG 에선 도달 불가하나 "항상 존재" 의 이론적 예외.

## Phase 1 함수별 계약

| 함수 (시그니처) | 반환 | georef/값 부재 시 | Sedona 와의 차이 |
| -- | -- | -- | -- |
| `RS_Width(path)` | INTEGER | 항상 존재 (level 0 폭) | 인자형 외 없음 |
| `RS_Height(path)` | INTEGER | 항상 존재 (level 0 높이) | 〃 |
| `RS_NumBands(path)` | INTEGER | 항상 존재 (IFD0 SamplesPerPixel) | 〃 |
| `RS_ScaleX(path)` | DOUBLE | NULL | 〃 |
| `RS_ScaleY(path)` | DOUBLE | NULL | 〃 (north-up 관례로 **음수**) |
| `RS_SkewX(path)` / `RS_SkewY(path)` | DOUBLE | NULL | georef 있으면 항상 0.0 (위 공통 차이) |
| `RS_UpperLeftX(path)` / `RS_UpperLeftY(path)` | DOUBLE | NULL | 인자형 외 없음 |
| `RS_SRID(path)` | INTEGER | **0** (NULL 아님) | Sedona/PostGIS 관례 준수. read_cog 의 crs 컬럼은 부재 시 NULL — 컬럼과 함수의 규약이 다름에 주의 |
| `RS_BandNoDataValue(path[, band])` | DOUBLE | NULL (nodata 부재·범위 밖 밴드) | GDAL_NODATA 태그는 파일 단위 — 전 밴드 동일값 반환 (Sedona 는 밴드별 가능) |
| `RS_MetaData(path)` | STRUCT | georef 파생 필드만 NULL, srid 0 | Sedona 1.5 는 DOUBLE 배열(10원소) — 우리는 DuckDB 관례의 named STRUCT: (upperleftx, upperlefty, width, height, scalex, scaley, skewx, skewy, srid, numbands) |
| `RS_GeoReference(path)` | VARCHAR | NULL | GDAL 포맷만 지원 (`format` 파라미터·ESRI 포맷 없음). 6줄, `%.6f`: scaleX\nskewY\nskewX\nscaleY\nupperLeftX\nupperLeftY |

## Phase 2 — 픽셀 접근 (구현분)

| 함수 (시그니처) | 반환 | NULL 규약 | Sedona 와의 차이 |
| -- | -- | -- | -- |
| `RS_Value(path, x DOUBLE, y DOUBLE[, band])` | DOUBLE | extent 밖·범위 밖 밴드·nodata·NULL 인자 → NULL | 인자형 외: level 0 고정 판독, 보간 없음(floor 격자 — 원점 코너는 픽셀 (0,0), 우/하단 경계 좌표는 밖). georef 없는 파일 → 에러 (좌표 해석 불가; bbox 필터와 동일 결정) |
| `RS_WorldToRasterCoord(path, x, y)` | STRUCT(col, row) INTEGER | NULL 인자 → NULL | 1-based, 순수 변환(경계 검사 없음 — extent 밖도 환산). i32 초과 좌표는 NULL. georef 없음 → 에러 |
| `RS_RasterToWorldCoord(path, col, row)` | STRUCT(x, y) DOUBLE | NULL 인자 → NULL | 1-based 픽셀의 **좌상단 코너** 좌표. georef 없음 → 에러 |
| `RS_Values(path, xs DOUBLE[], ys DOUBLE[][, band])` | DOUBLE[] | 리스트 인자 NULL → NULL; 원소 NULL·extent 밖·nodata → 그 원소만 NULL | Sedona 는 Point geometry 배열 — 우리는 좌표 배열 쌍(geometry 타입 부재). xs/ys 길이 불일치 → 에러. 같은 타일 점들은 1회 fetch+decode |
| `RS_BandAsArray(path, band[, bbox DOUBLE[]])` | DOUBLE[] | 범위 밖 밴드·NULL 인자 → NULL (빈 배열과 구분); nodata → NULL **원소** | bbox 없으면 전체 level 0 밴드(georef 불요), 있으면 픽셀 중심 포함 윈도 (zonal 과 동일 규약 — Sedona 에 없는 windowed 확장). row-major (좌상→우하) |
| `RS_ZonalStats(path, bbox DOUBLE[], band, stat)` | DOUBLE | NULL 인자·bbox 원소 NULL → NULL; 유효 픽셀 없음 → count 0, 나머지 NULL | **의도적 이탈**: zone 은 geometry 가 아니라 bbox (GEOS 비링크 N4). 픽셀 **중심** 포함 규약(닫힌 구간 — 공유 경계 위 중심은 이웃 zone 양쪽에 포함), nodata 제외. 범위 밖 밴드도 빈 집계와 동일 (count 0 / 나머지 NULL — RS_Value 의 NULL 과 비대칭, 의도). stat ∈ count/sum/mean/min/max (대소문자 무관); 모르는 stat·bbox 형식 오류 → 에러 |
| `RS_NormalizedDifference(path, x, y, b1, b2)` | DOUBLE | 결측(extent 밖·nodata·범위 밖 밴드)·**합 0**(0/0 정의 불가)·NULL 인자 → NULL | **의도적 이탈**: Sedona 는 raster 반환 — 우리는 reader(N3)라 포인트 값 연산. (v2−v1)/(v2+v1). Float 래스터의 NaN 픽셀은 nodata=NaN 이 아니면 NaN 으로 전파 |

판정: T1 조밀 오라클 (`tests/oracle/test_rs_value_oracle.py`) — multiband 전 픽셀
중심 ×3밴드 전수 + basic/edge 무작위 300점씩을 rasterio `ds.sample` 과 대조
(ABI 일치 duckdb-python 으로 실제 익스텐션 로드, `just ext-test` 가 실행).

## Phase 2 잔여 (미구현)

`read_stac()` (테이블 함수, §6.7) 이 마지막 표면. 래스터
생성/변형 함수군(`RS_MakeRaster`, `RS_Resample` 등)은 명시적 범위 밖 (N3).

## 판정

계약 테스트: `test/sql/rs_metadata.test` (E2E) · `crates/engine/src/meta.rs` 단위
테스트 · `tests/oracle/test_fixture_*.py` (rasterio 오라클) — 세 곳이 같은 수치를
판정한다 (RFC §6.9 T1/T3).
