# RFC-001 (rev.3): DuckDB를 위한 GDAL-free COG-native 래스터 접근 계층

|  |  |
| -- | -- |
| **상태(Status)** | Draft — **rev.3** |
| **작성자(Author)** | 손호영 |
| **작성일(Created)** | 2026-06-27 |
| **개정일(Revised)** | 2026-07-05 |
| **작업명(Working name)** | `duckdb-cog` *(임시 — 네이밍은 미해결 항목)* |
| **대체(Supersedes)** | RFC-001 rev.2 (2026-07-03) |

---

## 0. rev.2 변경 요약 — 무엇이 달라지고, 왜 더 좋아지는가

rev.1 작성 직후의 생태계 조사에서 리스크 지도가 바뀌었음을 확인했다. 이 개정판은 세 가지 전략적 전환을 반영한다.

### 0.1 세 가지 전환

| # | 항목 | rev.1 | rev.2 | 무엇이 좋아지는가 |
| -- | -- | -- | -- | -- |
| **A** | **COG decode 엔진** | 순수 Rust decode 계층을 직접 구축·검증 (`async-tiff`/`cog3pio`/`tiff`는 미검증 후보; Icechunk를 코어 의존성으로 상정) | **`async-tiff`(Development Seed)를 엔진 기반으로 채택.** decode 자작 포기. Icechunk는 v1에서 제외, Phase 2+로 유예 | 최대 미지수였던 R2가 **소멸**. IFD 파싱·오버뷰·압축(DEFLATE/LZW/ZSTD)·요청 병합·object_store I/O가 이미 프로덕션급(Lonboard가 실전 검증). Phase 0~1 기간 단축, 유지보수 표면적 축소, WASM 스토리 단순화 |
| **B** | **브라우저 전달체** | DuckDB-WASM "익스텐션"을 1순위, 사이드카는 fallback | **사이드카를 브라우저의 정식 경로로 확정.** WASM 익스텐션은 v1 비목표(N6)로 강등 | 최고 리스크 R1을 정면돌파 대신 **우회로 해소**. DuckDB-WASM 익스텐션은 Emscripten side-module 전용 툴체인 + 런타임 심볼 해석 실패(컴파일 성공≠동작)라는 이중 함정이 확인됨. 사이드카(wasm-bindgen + Arrow 교환)는 Lonboard가 사실상 동일 구조로 이미 증명한 검증된 경로 |
| **C** | **출시 전략** | 마일스톤 완성도 중심, 배포 채널 미정 | **Phase 1을 최소 범위로 압축해 DuckDB community-extensions 저장소에 조기 등록** | ahuarte47 `duckdb-raster`가 커뮤니티 익스텐션으로 승격되어 뉴스레터에 소개되는 등 마인드셰어를 선점 중. "최초의 GDAL-free 래스터 익스텐션" 포지션은 시간이 지날수록 좁아지는 창(window) — 조기 출시가 곧 해자 |

### 0.2 전환이 만드는 구체적 차이

**개발 속도.** rev.1의 Phase 0은 "decode 크레이트가 실제 위성 COG를 처리하는가"라는 열린 연구 질문을 포함했다. rev.2에서는 이 질문이 "async-tiff API 위에 타일-테이블 스키마를 얹는다"라는 닫힌 엔지니어링 작업으로 바뀐다. 만들어야 할 코드가 "COG reader + DuckDB 어댑터"에서 **"DuckDB 어댑터"로 줄어든다.**

**리스크 프로파일.** rev.1의 2대 리스크(R1 WASM 익스텐션, R2 decode 커버리지)가 rev.2에서는 하나는 우회, 하나는 소멸한다. 대신 새 리스크(업스트림 의존, 경쟁 창)가 들어오지만, 이들은 "성공 여부"가 아니라 "속도와 포지셔닝"의 리스크다 — 프로젝트가 죽는 시나리오의 수가 줄었다.

**포지셔닝.** rev.1은 "엔진을 만드는 프로젝트"였다. rev.2는 **"async-tiff 생태계와 DuckDB SQL 사이의 빠진 어댑터를 최초로 꽂는 프로젝트"**다. Development Seed 스택(async-tiff → async-geotiff → Lonboard)은 렌더링·Python 방향으로만 뻗고 있고 SQL 계층이 비어 있다. 이 어댑터는 그들과 경쟁하는 게 아니라 그 커뮤니티의 수요에 올라탄다. 단, 조합이 자명해진 만큼 선점 속도가 전부다.

**비전 정합성.** "Zed of QGIS" 장기 비전 관점에서도 사이드카 확정이 유리하다: Tauri 앱은 어차피 엔진을 네이티브/WASM으로 직접 임베딩하는 구조가 자연스럽고, DuckDB-WASM 익스텐션 로딩이라는 가장 불안정한 경로에 앱의 운명을 걸지 않게 된다. Lonboard 사례는 "reproject를 렌더 계층에 위임"(N2) 테제의 외부 실증이기도 하다 — 브라우저 측 재투영으로 래스터 warping 없이 COG를 직접 시각화하는 접근이 실전에서 동작함을 보여줬다.

**대가(정직하게).** (a) decode 계층의 통제권을 업스트림에 넘긴다 — async-tiff가 못 하는 압축/레이아웃이 나오면 기여(PR)로 풀어야 한다. (b) Icechunk 기반 시계열/버저닝 능력은 v1에서 빠진다. (c) "우리가 바닥부터 만든 엔진"이라는 서사는 약해지고 "생태계 통합을 가장 잘 하는 프로젝트"라는 서사로 대체된다. 셋 다 수용 가능한 트레이드오프로 판단한다.

### 0.3 rev.3 추가 변경 (2026-07-05)

| # | 항목 | 내용 | 왜 좋아지는가 |
| -- | -- | -- | -- |
| **D** | **Sedona `RS_*` 카탈로그를 따라 함수 구현** (§6.8) | SedonaDB가 구현 중인 RS_* 함수 목록을 "무엇을 만들 것인가"의 준거 카탈로그로 삼아, 같은 이름·시그니처의 함수를 duckdb-cog에 구현한다. 상호운용을 보장하는 호환 계약이 아니라 설계 참조 | 함수 목록·이름을 스스로 발명할 필요가 없고, 사용자·AI 도구의 기존 RS_ 지식이 그대로 통하며, Sedona 문서가 사실상 우리 스펙 초안 역할을 함 |
| **E** | **테스트-확실성(test-certain) 개발 원칙 명문화** (§6.9) | 오라클(rasterio/GDAL) 대조 테스트, 픽스처 매트릭스, sqllogictest, property test, 함수별 완료 정의(DoD)를 설계 문서 수준으로 격상 | 에이전트(Claude Code) 주도 개발의 전제 조건 — 기계가 스스로 검증 가능한 루프가 있어야 자율 개발이 성립. 회귀·픽셀값 정확성이 프로젝트 신뢰의 근간 |

**rev.3의 배경 신호:** SedonaDB(Apache Sedona의 Rust/DataFusion 엔진)가 래스터 지원 epic을 본격 가동 중이다 — 0.2.0에서 래스터 타입 + 기본 RS_ 함수 출시, N-D 래스터 스키마로 33개 RS_ 함수 재구현, Zarr I/O 계획. 단 그들의 계획은 compute-heavy 연산(clip, zonal stats, map algebra)에 **GDAL-backed 구현**을 쓰고, COG in-place가 아닌 in-DB/out-DB 모델이다. 본 프로젝트의 wedge(GDAL-free + COG in-place + DuckDB)는 유효하되, RS_ 함수 표면이라는 **관습의 표준화**는 그들을 따르는 것이 유리하다 — 포맷은 경쟁하고 관습은 편승한다.

---

## 1. 요약 (Summary)

이 RFC는 Cloud-Optimized GeoTIFF(COG)를 **제자리에서(in-place)** 읽는 DuckDB 래스터 접근 계층을 제안한다 — GDAL 없이, 새 포맷으로 재인코딩하지 않고, 읽는 시점에 reproject하지 않고.

핵심은 **`async-tiff`(순수 Rust, Development Seed) 위에 얹은 얇은 엔진 크레이트**다. async-tiff가 COG 바이트의 lazy range-read와 타일 decode를 담당하고, 엔진은 그 위에 관계형 타일-테이블 모델, 오버뷰 선택, 공간 pruning 키, 통계 매핑을 더한다. DuckDB 익스텐션은 그 엔진의 *한 얼굴*로서 SQL table function을 노출한다. 브라우저에서는 익스텐션이 아니라 **사이드카**(같은 엔진의 wasm-bindgen 빌드 + DuckDB-WASM, Arrow로 데이터 교환)로 동작한다.

빈칸(wedge)은 기존 어떤 도구도 동시에 채우지 못한 교집합이다: **GDAL-free + COG in-place(재인코딩 없음) + native CRS 보존 + STAC 인지 + 브라우저 동작 가능.**

---

## 2. 동기 (Motivation)

### 2.1 문제

SQL/분석 엔진에서 래스터를 쿼리하는 기존 방식은 각자 받아들이기 힘든 trade-off를 강요한다:

* **GDAL**에 의존한다 — `wasm32`로 컴파일 불가, 버전에 핀이 박히고, 빌드가 무거운 거대한 네이티브 C/C++ 스택.
* 래스터를 전용 저장 포맷으로 **재인코딩**해야 한다 — 스토리지 2배 + 원본 픽셀값 변형.
* **OLTP 서버(PostgreSQL)**에 묶인다 — 분석 스캔, 서버리스/임베디드/브라우저 용도에 안 맞는 형태.

특히 농업 원격탐사(UTM 또는 한국 EPSG:5179의 Sentinel/CAS500 영상에서 NDVI·반사율·작물분류)에서는 **픽셀값 그 자체가 과학적 측정값**이다. ingest 시점에 리샘플/reproject하는 파이프라인은 측정값을 훼손한다.

### 2.2 빈칸 — 그리고 좁아지는 창

GDAL-free 동작, COG in-place 읽기, 임의 CRS 보존, STAC-native 카탈로깅, 브라우저 호환을 **동시에** 제공하는 도구는 여전히 없다. 그러나 rev.1 이후 두 가지 시장 신호가 확인됐다:

1. **`ahuarte47/duckdb-raster`가 DuckDB community extension으로 승격**되어 공식 커뮤니티 익스텐션 페이지에 등재되고 생태계 뉴스레터(2026-06)에 소개됐다. GDAL 의존이라는 구조적 약점은 그대로지만, "DuckDB에서 래스터 = 이것"이라는 인식이 굳어가고 있다.
2. **Development Seed가 GDAL-free COG 스택을 빠르게 수직 통합** 중이다(async-tiff → async-geotiff → Lonboard 브라우저 COG 렌더링, deck.gl-raster/GeoZarr 로드맵). SQL 계층만 비어 있다.

결론: wedge는 유효하나, 시한부다. 이 RFC의 실행 속도가 곧 전략이다.

---

## 3. 목표 (Goals)

* **G1.** 오브젝트 스토리지(S3/HTTP) 또는 로컬 스토리지에 저장된 COG를 in-place로, lazy range-read로 읽는다 — 쿼리가 건드리는 타일만 fetch.
* **G2.** 읽기 경로에서 GDAL·PROJ·GEOS 등 무거운 네이티브 지오 C/C++ 의존성을 **전혀** 쓰지 않는다.
* **G3.** 소스 CRS와 원본 픽셀값을 보존한다. 읽기 시점에 리샘플하지 않는다.
* **G4.** 래스터를 1급 관계형 데이터로 노출해, 벡터/표 데이터와 **단일 SQL 플랜 안에서** 조인되게 한다.
* **G5.** 공간 predicate pushdown으로 필터가 관련 타일만 건드리게 한다.
* **G6.** COG 오버뷰 레벨을 노출해, 줌아웃 쿼리가 저해상도 데이터를 읽게 한다.
* **G7.** (COG/STAC 메타데이터의) per-tile 통계를 노출해, 픽셀 decode 없이 집계가 돌게 한다.
* **G8.** *(rev.2 개정)* 엔진 크레이트가 `wasm32-unknown-unknown` 타깃에서 브라우저 fetch 기반으로 동작 가능하게 유지한다 — 전달체는 사이드카(§6.5).
* **G9.** 카탈로그 기반 접근을 위해 STAC를 인지한다.
* **G10.** *(rev.2 신설)* Phase 1 산출물을 DuckDB community-extensions 저장소에 등록해 공식 `INSTALL … FROM community;` 경로로 배포한다.
* **G11.** *(rev.3 신설)* 스칼라/집계 함수는 Apache Sedona `RS_*` 카탈로그를 준거로 삼아 같은 이름·시그니처로 구현한다 — 선별 부분집합이며, 상호운용 호환 계약이 아닌 설계 참조(§6.8).
* **G12.** *(rev.3 신설)* 모든 공개 함수는 오라클 대조 테스트와 sqllogictest를 통과해야 완료로 간주한다 — "테스트가 확실한 개발"(§6.9).

## 4. 비목표 (Non-Goals)

* **N1. 새 on-disk 저장 포맷을 만들지 않는다.** COG가 저장소, STAC가 카탈로그, DuckDB가 쿼리 계층. 출력/중간결과만 Arrow/Parquet로 쓴다.
* **N2. 읽기 시점 reproject를 하지 않는다.** 좌표 변환은 렌더 계층(MapLibre/deck.gl) 또는 명시적 opt-in 단계에 위임. *(rev.2 보강: Lonboard가 브라우저 측 재투영으로 warping 없는 COG 시각화를 실증 — 이 위임 전략의 외부 증거.)*
* **N3. PostGIS/Sedona 함수 *패리티*를 노리지 않는다.** *(rev.3 보강: 단, 구현하는 함수의 **이름과 의미론은** Sedona RS_* 규약을 따른다(G11). "적게 만들되, 만드는 것은 표준 관습대로.")*
* **N4. 읽기 경로 어디에도 GDAL/PROJ를 링크하지 않는다.** *(rev.3 명확화: 테스트 스위트의 **오라클**로서 GDAL/rasterio를 dev-dependency로 쓰는 것은 허용 — 제품 바이너리에 링크되지 않는 한 원칙 위반이 아니다. SedonaDB도 RS_Value를 rasterio 대조로 검증한다.)*
* **N5. "익스텐션"을 유일한 전달체로 고정하지 않는다.** 엔진은 라이브러리고, 익스텐션은 그 한 얼굴이다.
* **N6.** *(rev.2 신설)* **DuckDB-WASM "익스텐션" 빌드는 v1 범위가 아니다.** Emscripten side-module 툴체인 요건과 런타임 심볼 해석 실패 양상(§9 R1′)이 해소되기 전까지 브라우저는 사이드카로만 지원한다. 업스트림에서 Rust 툴체인 지원이 성숙하면 재평가한다.
* **N7.** *(rev.2 신설)* **TIFF decode 계층을 직접 구현하지 않는다.** IFD 파싱·압축 해제·타일 fetch는 async-tiff에 위임한다. 커버리지 공백은 자체 fork가 아니라 업스트림 기여로 해결하는 것을 원칙으로 한다.
* **N8.** *(rev.2 신설)* **Icechunk는 v1 의존성이 아니다.** virtual chunk reference가 주는 가치(스냅샷/버저닝/시계열)는 Phase 2+에서 필요가 실증될 때 도입한다.

---

## 5. 배경 / 선행기술 (Prior Art)

### 5.1 `ahuarte47/duckdb-raster` (GDAL 기반) — *상태 갱신*

GDAL을 감싼 DuckDB 익스텐션. **rev.2 시점: community extension으로 정식 배포, 생태계 뉴스레터 노출.** `RT_Read`가 래스터를 one-row-per-tile 테이블로 반환하고, 픽셀 읽기 전에 타일을 스킵하는 filter pushdown, datacube 대수 연산, COG 드라이버로의 `COPY TO`까지 제공한다.

* **훔칠 만한 구조(유지):** one-row-per-tile 테이블 모델(`id, x, y, bbox, geometry(crs), level, tile_x, tile_y, cols, rows, metadata` + 밴드별 BLOB); 픽셀 BLOB 로드 전 가벼운 컬럼으로 prefilter하는 pushdown; sparse-tile skip.
* **치명적 결함(유지):** 모든 능력이 GDAL 호출. WASM 영구 불가, 버전핀, 무거운 빌드, 오버뷰 미구현(`level` 항상 0).
* **rev.2 함의:** 결함은 그대로이나 **마인드셰어 경쟁자로 격상**. 기술이 아니라 속도로 대응해야 한다.

### 5.2 `raquet` / `duckdb-raquet` (CARTO) — *상태 갱신*

**rev.2 시점: 역시 community extension으로 등재.** GDAL이 필요한 것은 `read_raster()`(ingest)뿐이고 나머지 함수는 GDAL 없이 동작하는 형태로 정리됐다 — rev.1의 분석(쿼리 경로 GDAL-free, ingest GDAL 종속)이 배포 형태로 확인된 셈.

* **유지되는 결함:** Web Mercator(3857) 락 + on-ingest 리샘플이 과학 픽셀값 훼손; 복사/재인코딩 모델(스토리지 2배); 깨지기 쉬운 `block=0` 메타데이터 행.
* **차용(유지):** per-tile 통계 사전계산으로 decode 없는 집계; 컬럼나-분석 자세.

### 5.3 PostGIS Raster — *변경 없음*

OLTP row-store 종속, GDAL 의존, in-db 1GB 한도/out-db 파일핸들 한도. 함수 폭으로 경쟁하지 말고 클라우드-네이티브 분석 형태로 경쟁한다는 교훈 유지.

### 5.4 *(rev.2 신설)* Development Seed 스택: `async-tiff` / `async-geotiff` / Lonboard

rev.1이 "검증 필요 후보"로 분류했던 `async-tiff`가 이 개정의 최대 변수다.

* **`async-tiff` (Rust):** async 전용 저수준 TIFF reader. 타일드 TIFF, object_store 크레이트를 통한 S3/GCS/Azure/HTTP 직접 읽기, I/O-bound(fetch)와 CPU-bound(decode)의 분리 스케줄링, 사용자 정의 압축 알고리즘 플러그인, **타일 요청 병합·동시성**, GeoTIFF 태그 메타데이터, ndarray 통합. COG 내부 타일 byte range 노출.
* **`async-geotiff` (Python, Rust 코어):** 오버뷰(축소 해상도) 접근, nodata 마스크, CRS 해석(PyProj), Affine 지오트랜스폼, COG 타일 그리드의 TileMatrixSet 표현. 동시 메타데이터 파싱에서 Rasterio 대비 25배 성능 보고.
* **Lonboard COG 통합 (2026-04):** async-geotiff 기반으로 **GDAL 의존 없이** 야생의 대다수 COG를 온디맨드 타일 스트리밍으로 브라우저 시각화. 별도 타일 서버 불필요, 재투영은 브라우저에서 자동 처리.

**함의 셋:**
1. **R2(decode 커버리지) 리스크 소멸** — 프로덕션 실증까지 끝난 크레이트가 존재한다.
2. **사이드카 아키텍처의 실증** — "Rust COG 엔진 + 브라우저 렌더링, GDAL 없음"이 실제로 돌아간다.
3. **경쟁이자 기회** — 이 스택에는 SQL 계층이 없다. `async-tiff × DuckDB` 어댑터는 그들 생태계의 빈칸이며, 본 프로젝트가 그 빈칸을 채우는 최초가 될 수 있다. 단 조합이 자명하므로 창은 짧다(추정 6개월 내외).

### 5.5 *(rev.3 신설)* SedonaDB 래스터 로드맵

Apache Sedona의 단일노드 Rust 엔진 SedonaDB(DataFusion 기반)가 래스터 지원을 본격화하고 있다.

* **현황:** 0.2.0에서 래스터 Arrow 타입 + 기본 RS_ 함수 출시. N-D 래스터 확장 계획(#746)에 따르면 33개 RS_ 함수를 trait 기반으로 재구현, ZarrBand lazy 로딩, RS_DimToBand 등 차원 대수까지 설계됨. RS_Value는 rasterio 대조 테스트로 검증하고, 전용 테스트 하네스 크레이트(sedona-testing)를 운용.
* **우리와의 차이:** compute-heavy 연산(clip/zonal stats/map algebra)은 **GDAL-backed 구현**을 계획 — GDAL-free가 아니다. 저장 모델은 in-DB/out-DB + Zarr 우선으로, **COG in-place**가 1급이 아니다. DuckDB가 아닌 DataFusion 생태계다.
* **함의:** (a) wedge는 침범되지 않았으나 "Rust SQL 엔진 + 래스터"라는 공간 자체가 붐빈다 — R7(경쟁 창)의 근거 보강. (b) RS_* 함수 규약이 Spark Sedona → SedonaSnow → SedonaDB로 이식되며 **Spatial SQL의 사실상 표준**이 되고 있다 — 우리가 독자 함수명을 만드는 것은 순손실. (c) 그들의 테스트 방법론(오라클 대조, table-driven, 전용 하네스)은 §6.9가 차용할 검증된 모범.

### 5.6 QUADBIN과 "Parquet 위의 공간 필터" — *변경 없음*

pruning의 원천은 공간충진곡선 정렬 + zone-map 통계이지 Web Mercator가 아니다. native 그리드 위 Hilbert/Morton 키 또는 `bbox` min/max 컬럼으로 임의 CRS에서 동일 효과.

### 5.7 빈칸 (요약, rev.2 갱신)

| 능력 | ahuarte47 | raquet | PostGIS | DevSeed 스택 | **이 RFC** |
| -- | -- | -- | -- | -- | -- |
| GDAL-free **end-to-end** | ✗ | read만 | ✗ | ✅ | ✅ |
| COG **in-place**(재인코딩 없음) | △ | ✗ (복사) | △ | ✅ | ✅ |
| **임의 CRS** 보존 | △ | ✗ (3857만) | ○ | ✅ | ✅ |
| **SQL 조인/pushdown** | ✅ | ✅ | ✅ | **✗** | ✅ |
| **STAC**-native | ✗ | ✗ | ✗ | △ | ✅ |
| **브라우저** 동작 | ✗ | △ | ✗ | ✅ (렌더링) | ✅ (사이드카) |
| 함수 성숙도 | △ | △ | ◎ | — | (목표 아님) |

교집합에서 유일하게 비는 칸이 **"GDAL-free COG in-place × SQL"** — 이것이 rev.2의 wedge다.

---

## 6. 제안 설계 (Proposed Design)

### 6.1 핵심 테제 (rev.2 개정)

**새 포맷도, 새 reader도 만들지 마라. 이미 존재하는 GDAL-free COG reader(async-tiff)를 DuckDB SQL에 꽂는 어댑터를 최초로 만들어라 — 브라우저는 사이드카로. reproject는 렌더 계층에 위임.**

### 6.2 아키텍처: 라이브러리 우선, 익스텐션은 하나의 얼굴

durable asset은 **엔진 크레이트**다 — 단, rev.2의 엔진은 decode를 품지 않고 async-tiff 위의 얇은 도메인 계층이다.

```
                ┌────────────────────────────────────────────┐
                │        엔진 크레이트 (순수 Rust, 얇음)        │
                │  • 타일-테이블 모델 (관계형 스키마 §6.4)      │
                │  • 오버뷰 레벨 선택 로직                     │
                │  • native-grid SFC 키 / bbox pruning (§6.6) │
                │  • COG/STAC 통계 → 컬럼 매핑 (§6.7)          │
                │  • STAC 카탈로그 워킹                        │
                ├────────────────────────────────────────────┤
                │        async-tiff  (업스트림 의존)           │
                │  • IFD/BigTIFF 파싱, GeoTIFF 태그            │
                │  • 타일 lazy range-read, 요청 병합           │
                │  • DEFLATE/LZW/ZSTD + 커스텀 decode 플러그인 │
                │  • object_store I/O (S3/GCS/Azure/HTTP)     │
                └────────────────────────────────────────────┘
                   ▲                    ▲                  ▲
        네이티브    │      브라우저      │       서버       │
   ┌───────────────┴──┐  ┌─────────────┴────┐  ┌──────────┴─────┐
   │ DuckDB 익스텐션   │  │ 사이드카 (확정)    │  │ 사이드카 / 앱   │
   │ (Rust, table fn) │  │ wasm-bindgen 빌드 │  │ 임베딩          │
   │ community-ext 등록│  │ + DuckDB-WASM     │  │                │
   │                  │  │ Arrow IPC 교환    │  │                │
   └──────────────────┘  └──────────────────┘  └────────────────┘
```

* **네이티브 DuckDB** → Rust 익스텐션(공식 extension-template-rs 기반; 현재 커뮤니티 등록은 C++ 글루 + CMake 템플릿 경유 빌드가 요구됨을 감안). **이걸 먼저, 최소 범위로 낸다.**
* **브라우저** → 같은 엔진 크레이트를 `wasm32-unknown-unknown` + wasm-bindgen으로 빌드, object_store의 I/O를 브라우저 fetch로 대체, 결과를 Arrow IPC로 DuckDB-WASM에 등록. WASM "익스텐션"이 아니다(N6).

### 6.3 엔진 구성: async-tiff 채택, Icechunk 유예 (rev.2 개정)

* **PixelQuery에서 가져오는 것:** 오케스트레이션 코드가 아니라 **알고리즘과 설계** — 오버뷰 선택 휴리스틱, 윈도잉 전략, 통계 매핑, "COG in-place" 개념 검증 자체.
* **async-tiff에 위임하는 것:** IFD/태그 파싱, 타일 fetch·병합, 압축 해제. 커버리지 공백(특정 압축 변종, sparse 처리 등)이 발견되면 fork가 아닌 **업스트림 PR**로 해결(N7) — 부수 효과로 Development Seed 커뮤니티 내 가시성 확보.
* **Icechunk를 빼는 이유(N8):** async-tiff가 COG byte-range를 직접 lazy fetch + 요청 병합까지 수행하므로, v1 유스케이스에서 virtual chunk reference 계층이 얹는 추가 가치가 불분명하다. 계층 하나가 빠지면 빌드·WASM·감사 표면이 모두 준다. 시계열 스냅샷/버저닝 수요가 실증되는 시점(Phase 2+)에 재도입을 검토한다.

### 6.4 관계형 스키마 — *변경 없음*

ahuarte47의 one-row-per-tile 모델을 차용하되 `level` 컬럼을 살려 오버뷰를 노출:

* 가벼운 컬럼: `id, x, y, bbox, geometry(crs), level, tile_x, tile_y, cols, rows, metadata` — 픽셀을 건드리지 않고 pushdown.
* 무거운 컬럼: 밴드별 BLOB(self-describing 헤더) 또는 멀티밴드 packing BLOB.
* `level`은 COG 오버뷰 IFD에 연결 — async-geotiff가 이미 오버뷰를 1급으로 노출하므로 구현 부담이 rev.1 대비 낮다.

### 6.5 I/O 경로 결정 — rev.2에서 질문이 바뀜

rev.1 §6.5의 문제의식("WASM 익스텐션의 sync/async 충돌을 DuckDB FileSystem 위임으로 해소")은 사이드카 확정으로 **동기 자체가 소멸**했다. 남는 것은 네이티브 익스텐션에서의 순수한 트레이드오프다:

| 경로 | 장점 | 단점 |
| -- | -- | -- |
| **(a) object_store 직행** (async-tiff 기본) | async-tiff를 무개조로 사용; 요청 병합·동시성 그대로; 코드 최소 | DuckDB Secrets(`CREATE SECRET`)·httpfs 설정과 이원화 — 사용자가 자격증명을 두 곳에 관리 |
| **(b) DuckDB FileSystem 브릿지** | DuckDB 자격증명/프록시/캐시 생태계와 통합; 사용자 경험 일관 | async-tiff의 reader trait을 DuckDB FS 위에 구현해야 함; 요청 병합 로직과의 상호작용 검증 필요 |

**결정 방침:** Phase 0에서 (a)로 스파이크(최속 경로), Phase 1 출시 전 (b)의 비용을 실측해 결정. table function의 pull-기반 동기 모델과 async-tiff의 async 모델 사이 브릿지는 네이티브에서는 단순 `block_on`으로 충분하다(전용 tokio 런타임 1개를 익스텐션 수명에 묶음).

### 6.6 QUADBIN 없는 공간 pruning — *변경 없음*

native 타일 그리드 위 Hilbert(선호)/Morton 키, 그리고/또는 `bbox` min/max 컬럼 노출. Web Mercator 의존 없이 zone-map pruning 동등 확보.

### 6.7 메타데이터 통계로 싼 집계 — *소폭 보강*

COG `GDAL_METADATA` / STAC `raster:bands` 통계를 per-tile 통계 컬럼으로 매핑해 decode 없는 집계를 지원. **주의(rev.2):** 야생의 COG 상당수는 타일 단위 통계 메타데이터가 없다 — STAC `raster:bands`가 더 신뢰할 만한 소스이며, 둘 다 없을 때의 graceful degradation(통계 컬럼 NULL + decode 경로 fallback)을 스키마 계약에 명시한다.

### 6.8 *(rev.3 신설)* SQL 함수 표면 — Sedona `RS_*` 카탈로그를 따라 구현

**원칙: 테이블 함수는 우리 것, 스칼라/집계 함수는 남의 카탈로그를 따라 만든다.** `read_cog()`/`read_stac()`은 DuckDB의 `read_parquet` 관습을 따르는 우리 고유 진입점이지만, 그 위에서 픽셀·메타데이터를 다루는 함수는 **이름과 시그니처를 새로 발명하지 않는다** — Apache Sedona의 `RS_*` 카탈로그(SedonaDB가 구현 중인 함수 목록)를 준거로 삼아 같은 꼴로 구현한다.

**이것은 참조이지 계약이 아니다.** SedonaDB와의 상호운용을 보장하지 않으며, Sedona 측 의미론 변경을 추적할 의무도 지지 않는다. 착수 시점의 Sedona 문서를 스냅샷해 준거로 고정하고, 이후 드리프트는 무시한다. 우리 실행 모델(COG in-place, native CRS)에 안 맞는 세부 의미론은 함수 문서에 차이를 명시하고 우리 방식으로 간다.

**따라 만들 때 그대로 가져오는 관습:**
* 밴드 인덱스는 **1-based** (Sedona 관습).
* 범위 밖 좌표·nodata 픽셀·빈 지오메트리는 에러가 아니라 **NULL** 반환.
* geotransform 순서는 GDAL 순서(scaleX, skewY, skewX, scaleY, upperLeftX, upperLeftY).

**v1~v2 구현 대상 (선별 부분집합):**

| 묶음 | 함수 | Phase |
| -- | -- | -- |
| 메타데이터 접근자 | `RS_Width`, `RS_Height`, `RS_NumBands`, `RS_ScaleX/Y`, `RS_SkewX/Y`, `RS_UpperLeftX/Y`, `RS_SRID`, `RS_BandNoDataValue`, `RS_MetaData`, `RS_GeoReference` | **1** (타일 테이블 컬럼과 동일 정보의 함수형 노출 — 구현 비용 낮음) |
| 픽셀 접근 | `RS_Value(raster, point[, band])`, `RS_Values` | **2** |
| 밴드 연산 | `RS_NormalizedDifference` (NDVI 등), `RS_BandAsArray` | **2** |
| 집계 | `RS_ZonalStats` (native CRS 픽셀 위에서) | **2** |
| 좌표 변환 | `RS_WorldToRasterCoord`, `RS_RasterToWorldCoord` | **2** |

**명시적 제외 (N3와 일관):** 래스터를 *생성/변형*하는 함수군(`RS_MakeRaster`, `RS_Resample`, `RS_AddBand`, `RS_MapAlgebra`의 raster-out 형태 등)은 범위 밖 — 우리는 reader이지 raster processor가 아니다. 필요해지면 별도 RFC.

**전략적 효과:** Sedona 사용자·문서·LLM 학습 데이터가 그대로 우리 온보딩 자산이 된다. 부수 효과로, 동일 쿼리를 SedonaDB에 던져 결과를 대조하는 교차 엔진 테스트를 *선택적* 검증 수단으로 쓸 수 있다(의무 아님 — 1차 오라클은 어디까지나 rasterio, §6.9 T1).

### 6.9 *(rev.3 신설)* 테스트 전략 — "테스트가 확실한 개발"

이 프로젝트의 산출물은 **과학 측정값을 만지는 인프라**다. 픽셀값 하나의 오차가 NDVI·수확량 예측의 오차로 전파되므로, 정확성 검증이 기능 구현과 동급의 1급 산출물이다. 또한 개발이 에이전트(Claude Code) 주도로 진행되므로(별도 문서: 개발 하네스 가이드), **기계가 스스로 판정할 수 있는 검증 루프**의 존재가 개발 속도의 전제 조건이다.

**T1. 오라클 대조 테스트 (정확성의 근간).** 모든 픽셀 접근·통계 함수는 GDAL/rasterio를 오라클로 삼아 동일 입력→동일 출력을 검증한다. 알려진 geotransform의 난수 래스터를 만들어 모든 픽셀 중심 + 픽셀 내 오프셋 + 무작위 내부 점을 조밀 샘플링해 `RS_Value` 결과가 rasterio 판독과 정확히 일치해야 한다(SedonaDB의 `test_rs_value_matches_rasterio` 패턴 차용). GDAL은 test/dev dependency로만 존재한다(N4 명확화).

**T2. 픽스처 매트릭스 (커버리지의 근간).** 두 층위:
* *합성 픽스처*: GDAL 스크립트로 생성하는 조합 매트릭스 — {압축: none/DEFLATE/LZW/ZSTD} × {predictor: none/horizontal/float} × {dtype: u8/u16/i16/f32} × {레이아웃: chunky/planar} × {BigTIFF 여부} × {sparse 타일 포함 여부} × {오버뷰 0~4단}. 생성 스크립트는 리포에 커밋, 산출물은 결정적(seed 고정).
* *실전 픽스처*: Sentinel-2 L2A 타일(UTM), 새팜 CAS500/드론 COG(EPSG:5179), Vantor 오픈데이터 등 — 소형 crop으로 리포 동봉, 원본 대형 파일은 CI 캐시.

**T3. sqllogictest (SQL 계약의 근간).** DuckDB 익스텐션 템플릿이 제공하는 sqllogictest 프레임워크로 모든 SQL-visible 동작을 선언적 테스트로 고정: 스키마, pushdown 후 결과 동일성(필터 on/off 결과 비교), 오버뷰 레벨 선택, NULL 의미론, 에러 메시지까지.

**T4. Property-based 테스트 (경계의 근간).** proptest로 타일 인덱스 산술·윈도잉·bbox 교차 로직에 불변식 검증: "임의 bbox에 대해 (pushdown 결과) ⊆ (전체 스캔 후 필터 결과)이며 두 집합은 동일", "타일 좌표 왕복 변환은 항등" 등.

**T5. 경계 계층 테스트.** async-tiff와의 접점은 reader trait 뒤에 있으므로(R8 완화 장치), mock reader로 fetch 횟수·range 병합을 단언한다 — "이 쿼리는 정확히 N개의 range GET을 유발해야 한다"는 **I/O 효율도 테스트 대상**이다(lazy read가 이 프로젝트의 존재 이유이므로).

**T6. WASM 스모크 테스트.** 엔진 크레이트는 `wasm-pack test --headless`로 브라우저 환경에서 대표 픽스처 decode를 검증 — R1′의 회귀 방지.

**T7. 벤치마크 회귀.** criterion 벤치를 CI에서 추적(콜드 첫-타일 지연, 웜 zonal stats 처리량). PixelQuery의 80ms 기준선을 성능 계약으로 승격.

**함수별 완료 정의(DoD):** 어떤 RS_ 함수도 다음 네 가지 없이 "완료"로 선언되지 않는다 — ① T1 오라클 대조 통과, ② T3 sqllogictest 스위트(정상+NULL+에러 경로), ③ T2 매트릭스의 관련 축 통과, ④ 함수 문서(시그니처, Sedona와의 의미론 차이가 있다면 명시). 이 DoD는 CI가 기계적으로 게이트한다.

---

## 7. 검토한 대안 (Alternatives Considered)

* `ahuarte47/duckdb-raster` **fork 후 GDAL 제거.** 기각(유지): 능력의 100%가 GDAL 호출. 참조 설계로만 사용.
* `raquet` **채택.** 기각(유지): 3857 락, 재인코딩 모델. per-tile 통계 아이디어만 차용.
* **PostGIS Raster.** 기각(유지).
* **새 클라우드-네이티브 래스터 포맷 발명.** 기각(유지, N1).
* *(rev.2 신설)* **자체 COG decode 계층 구축(rev.1의 기본 계획).** 기각: async-tiff가 동일 범위를 프로덕션급으로 커버하며 Lonboard로 실전 검증됨. 자작은 시간·유지보수·정합성 전부에서 열위. 통제권 상실은 업스트림 기여 전략(N7)으로 완화.
* *(rev.2 신설)* **Icechunk를 v1 코어로 유지.** 기각: async-tiff와 역할 중복, 계층 추가 비용 대비 v1 가치 불명(§6.3). Phase 2+ 재검토.
* *(rev.2 신설)* **DuckDB-WASM 익스텐션 정면돌파.** 기각(v1): Emscripten 전용 툴체인, Rust 호환성 미해결, "컴파일 성공 ≠ 로드/실행 성공" 함정(§9 R1′). 사이드카가 동일 사용자 가치를 검증된 경로로 제공.

---

## 8. 기존 자산과의 관계 (rev.2 개정)

세 갈래는 여전히 한 줄이지만, 어댑터의 반대쪽 끝이 바뀌었다:

> **PixelQuery**(개념 실증 + 알고리즘 공여) → **async-tiff**(decode/I/O 엔진, 업스트림) → **이 익스텐션**(async-tiff를 DuckDB SQL로 노출하는 빠진 어댑터) → **"Zed of QGIS"**(Tauri + MapLibre + DuckDB + deck.gl 앱, 엔진 직접 임베딩).

PixelQuery는 더 이상 재사용할 "엔진"이 아니라, 이 방향이 성립함을 증명한 **선행 실험이자 알고리즘 저장소**다. 이 재정의는 프로젝트를 from-scratch 문샷에서 **생태계 통합 프로젝트**로 바꾸며, 그만큼 실패 표면이 줄어든다.

---

## 9. 리스크 & 미해결 질문 (rev.2 전면 개정)

| ID | rev.1 | rev.2 상태 |
| -- | -- | -- |
| R1 | WASM 익스텐션 로드 + async I/O | **우회 해소** — 사이드카 확정(N6)으로 v1 경로에서 제거. 잔여분은 R1′로 축소 |
| R2 | 순수 Rust COG decode 커버리지 | **소멸** — async-tiff 채택 + Lonboard 실전 검증 |
| R3 | Icechunk-on-WASM 런타임 | **소멸** — Icechunk 자체가 v1에서 제외(N8) |
| R4 | 형태(form factor) | **해소** — 사이드카로 결정 |
| R5 | CRS 위임 지속성 | 유지, 근거 보강 |
| R6 | 네이밍 | 유지 |

**현행 리스크:**

* **R1′ (축소). 사이드카 WASM 빌드의 실측 미완.** async-tiff의 wasm32 타깃 + 브라우저 fetch 조합은 유사 사례(Lonboard는 Python 호스트 경유)가 있으나 본 프로젝트 구성 그대로의 실측은 아직이다. *Phase 0에서 검증.* 실패 시 fallback: 브라우저에서는 range-fetch를 JS로 하고 decode만 WASM으로.
* **R7 (신규, 최고). 경쟁 창.** `async-tiff × DuckDB` 어댑터는 자명한 조합이며, Development Seed 본인 또는 제3자가 착수할 수 있다. ahuarte47은 이미 커뮤니티 마인드셰어를 쌓는 중. **완화: Phase 1 범위를 최소로 압축해 community-extensions 등록을 최우선 이정표로.** 뉴스레터/커뮤니티 노출이 곧 방어.
* **R8 (신규). 업스트림 의존.** async-tiff의 API 안정성, 유지보수 속도, 커버리지 공백이 우리 로드맵의 외생 변수가 된다. 완화: (a) 엔진 크레이트에 reader trait 경계를 둬 교체 가능성 유지, (b) 공백은 업스트림 PR로 — 기여 실적이 곧 리스크 헤지이자 커뮤니티 자산.
* **R9 (신규, 소). 커뮤니티 등록 마찰.** Rust 익스텐션도 현재 C++ 글루 + DuckDB CMake 템플릿 경유가 요구되고, WASM 아티팩트는 "컴파일 통과 ≠ 동작"의 전례가 다수 — 커뮤니티 저장소가 WASM 빌드를 자동 생성하더라도 v1에서는 네이티브 플랫폼만 공식 지원 범위로 선언한다.
* **R5 (유지). CRS 위임의 지속성.** 완화책 동일(closed-form 화이트리스트: UTM/5179/4326/3857; PROJ-as-data fallback). Lonboard의 브라우저 재투영 실증으로 신뢰도 상승.
* **R6 (유지). 네이밍.** `duckdb-cog`는 임시. community-extensions는 고유 이름을 요구하므로 등록 전 확정 필요.
* **R10 (rev.3 신설, 소). 준거 카탈로그의 모호성.** RS_* 함수를 따라 만들 때 준거 자체가 모호할 수 있다(SedonaDB와 Spark Sedona 간 미묘한 의미론 차이, 미문서화 엣지케이스). 완화: 착수 시점 Sedona 문서를 스냅샷해 함수별 준거로 고정하고 이후 드리프트는 추적하지 않는다(§6.8 — 참조이지 계약이 아님). 준거와 다르게 구현하는 지점은 함수 문서에 명시.

---

## 10. 마일스톤 (rev.2 개정)

fail-fast 순서 유지 — 단, 죽일 미지수의 목록이 바뀌었다.

* **Phase 0 — Spike (며칠).**
  (a) extension-template-rs 기반 최소 Rust 익스텐션이 네이티브 빌드·로드;
  (b) async-tiff로 실제 Sentinel-2 COG(UTM) + 새팜 CAS500/드론 COG(EPSG:5179)의 타일을 S3 range-read → decode — *rev.1의 "크레이트 후보 검증"이 "우리 데이터 통과 확인"으로 축소*;
  (c) 엔진 크레이트 스켈레톤이 `wasm32-unknown-unknown`에서 브라우저 fetch로 동일 타일 decode (사이드카 실측, R1′);
  (d) `block_on` 브릿지로 table function에서 async-tiff 호출.
* **Phase 1 — 핵심 wedge + 조기 출시.** *(rev.3)* **테스트 인프라 선행:** T1 오라클 하네스, T2 합성 픽스처 생성기, T3 sqllogictest 배선을 첫 기능보다 먼저 세운다 — 이후 모든 기능이 이 레일 위에서 개발된다. 그 위에 `read_cog('s3://…')` table function: 타일-테이블 스키마, lazy windowed range-read, native CRS, bbox pushdown, **오버뷰 레벨 노출**; RS_ 메타데이터 접근자 묶음(§6.8 Phase 1 행). reproject 없음. **community-extensions 등록 + 이름 확정 + README/블로그로 "GDAL-free" 포지션 공표가 이 Phase의 완료 조건.**
* **Phase 2 — 분석.** `read_stac(catalog)` + STAC/COG 통계 매핑(decode 없는 집계, graceful degradation 포함); `RS_Value`/`RS_NormalizedDifference`/`RS_ZonalStats`/좌표 변환(§6.8 Phase 2 행) — 각 함수는 §6.9 DoD 게이트를 통과해야 완료. Icechunk 재검토 게이트.
* **Phase 3 — CRS (필요할 때만).** closed-form 변환 화이트리스트; PROJ-as-data fallback.
* **Phase 4 — 앱 통합.** Tauri 앱에 엔진 직접 임베딩 + DuckDB-WASM 사이드카 연결.

**권고: Phase 0의 (b)(c)를 이번 주 안에.** rev.1 대비 Phase 0의 성격이 "연구"에서 "확인"으로 바뀌었으므로 며칠이면 충분하고, R7(경쟁 창) 때문에 Phase 1 도달 속도가 프로젝트의 제1 변수다.

---

## 11. 참조 (References)

* `ahuarte47/duckdb-raster` — GDAL 기반 DuckDB 래스터 익스텐션 (community extension 등재; DuckDB 생태계 뉴스레터 2026-06 소개).
* CARTO `raquet` 포맷 스펙; `duckdb-raquet` community extension (read_raster만 GDAL 요구).
* PostGIS Raster 문서.
* QUADBIN 스펙.
* **Development Seed `async-tiff`** — async 저수준 TIFF reader (Rust); object_store I/O, 요청 병합, 커스텀 decode.
* **Development Seed `async-geotiff`** — COG 고수준 reader; 오버뷰·CRS·TileMatrixSet (2026-02 공개).
* **Lonboard COG 지원 (2026-04)** — async-geotiff 기반 GDAL-free 브라우저 COG 렌더링; 브라우저 측 재투영.
* DuckDB-WASM 익스텐션 로딩 트래킹 이슈 (#1202) — Emscripten 툴체인 요건, Rust 호환성 미해결.
* "Compiling Isn't Running" (Rusty Conover, 2026-06) — WASM 익스텐션의 런타임 심볼 해석 실패 전수 분석.
* `duckdb-rs` / extension-template-rs — Rust 익스텐션 빌드 경로.
* Icechunk (Earthmover) — Phase 2+ 재검토 대상.
* PixelQuery — 작성자의 GDAL-free COG-native 픽셀 쿼리 라이브러리 (개념 실증·알고리즘 공여).
* Cloud-Optimized GeoTIFF; STAC; COG 오버뷰/IFD 구조.
* **Apache SedonaDB** — 래스터 지원 epic(#246), N-D 래스터 확장(#746), 0.2.0 릴리스 노트(래스터 타입 + RS_ 함수); `sedona-raster-functions` 크레이트; rasterio 오라클 대조 테스트(`test_rs_value_matches_rasterio`), sedona-testing 하네스.
* Apache Sedona RS_* 함수 카탈로그 문서 (SedonaSQL API) — §6.8 준거 카탈로그 (착수 시점 스냅샷 고정).

---

## 부록 A. rev.1 → rev.2 조항별 대응표

| rev.1 조항 | rev.2 처리 |
| -- | -- |
| §6.3 Icechunk 기반 엔진 | §6.3 async-tiff 기반으로 교체, Icechunk는 N8로 유예 |
| §6.5 WASM async 해법 (FileSystem 위임) | §6.5 I/O 경로 트레이드오프로 재구성 (동기 소멸) |
| G8 "구조적으로 WASM 가능" | G8 "엔진의 wasm32 동작 유지, 전달체는 사이드카" |
| R1 (WASM 익스텐션) | R1′로 축소 (사이드카 실측만 잔존) |
| R2 (decode 커버리지) | 소멸 |
| R3 (Icechunk-on-WASM) | 소멸 |
| R4 (form factor) | 해소 (사이드카 확정) |
| — | R7 경쟁 창 / R8 업스트림 의존 / R9 등록 마찰 신설 |
| Phase 0 (d) decode 크레이트 검증 | Phase 0 (b) 자사 데이터 통과 확인으로 축소 |
| 마일스톤에 배포 채널 없음 | G10 + Phase 1 완료 조건에 community-extensions 등록 명시 |
