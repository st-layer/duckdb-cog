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

## Phase 2 이후 (미구현)

`RS_Value`/`RS_Values`, `RS_NormalizedDifference`, `RS_BandAsArray`, `RS_ZonalStats`,
`RS_WorldToRasterCoord`/`RS_RasterToWorldCoord` — RFC §6.8 표 참조. 래스터
생성/변형 함수군(`RS_MakeRaster`, `RS_Resample` 등)은 명시적 범위 밖 (N3).

## 판정

계약 테스트: `test/sql/rs_metadata.test` (E2E) · `crates/engine/src/meta.rs` 단위
테스트 · `tests/oracle/test_fixture_*.py` (rasterio 오라클) — 세 곳이 같은 수치를
판정한다 (RFC §6.9 T1/T3).
