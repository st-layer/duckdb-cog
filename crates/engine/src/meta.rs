//! 타일-테이블 도메인 모델 (RFC §6.4): COG 메타데이터 → 레벨/타일 그리드 + bbox.
//!
//! 픽셀은 건드리지 않는다 — IFD 메타데이터만 읽어 read_cog() 의 행을 만든다.

use async_tiff::metadata::cache::ReadaheadMetadataCache;
use async_tiff::metadata::TiffMetadataReader;

use crate::source::{ByteSource, FetchAdapter};
use crate::{pack_tile_key, MAX_TILE_INDEX};

/// COG 한 레벨(본체 IFD 또는 오버뷰 IFD)의 타일 그리드 메타데이터.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelMeta {
    pub level: u8,
    pub image_width: u32,
    pub image_height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
}

/// IFD0 의 GeoTIFF 태그에서 뽑은 georeference (level 0 기준).
///
/// 오버뷰 IFD 에는 geo 태그가 없는 것이 GDAL COG 관행 — 레벨 N 픽셀 크기는
/// 크기 비율(width0/widthN)로 유도하고 origin 은 공유한다.
#[derive(Debug, Clone, PartialEq)]
pub struct Georef {
    /// EPSG 코드 (projected 우선, 없으면 geographic).
    pub epsg: Option<u32>,
    pub origin_x: f64,
    pub origin_y: f64,
    /// level 0 픽셀 크기 (양수; y 는 북→남 진행으로 적용 시 감산).
    pub pixel_x: f64,
    pub pixel_y: f64,
}

/// COG 전체 메타데이터 — 레벨 순서는 IFD 순서(본체=0, 오버뷰=1..).
#[derive(Debug, Clone, PartialEq)]
pub struct CogMeta {
    pub levels: Vec<LevelMeta>,
    /// geo 태그 부재 시 None — bbox/crs 는 NULL 로 강등 (graceful degradation).
    pub georef: Option<Georef>,
    /// IFD0 SamplesPerPixel — 밴드 수.
    pub num_bands: u32,
    /// IFD0 GDAL_NODATA 태그 — 부재 시 None. GDAL 관행상 전 밴드 공통.
    pub nodata: Option<f64>,
}

impl CogMeta {
    /// CRS 문자열 표현 ("EPSG:32652" 꼴). georef 나 EPSG 부재 시 None.
    pub fn crs(&self) -> Option<String> {
        self.georef
            .as_ref()
            .and_then(|g| g.epsg)
            .map(|e| format!("EPSG:{e}"))
    }

    /// level 0 이미지 폭 (픽셀). 레벨 부재 시 None.
    pub fn width(&self) -> Option<u32> {
        self.levels.first().map(|l| l.image_width)
    }

    /// level 0 이미지 높이 (픽셀). 레벨 부재 시 None.
    pub fn height(&self) -> Option<u32> {
        self.levels.first().map(|l| l.image_height)
    }

    /// EPSG 코드 — 부재 시 0 (Sedona/PostGIS RS_SRID 관례; crs() 의 None 과 다름).
    pub fn srid(&self) -> u32 {
        self.georef.as_ref().and_then(|g| g.epsg).unwrap_or(0)
    }

    /// 1-based `band` 의 nodata. 범위 밖·nodata 부재 → None (RS_ NULL 규약).
    pub fn band_nodata(&self, band: u32) -> Option<f64> {
        if band == 0 || band > self.num_bands {
            return None;
        }
        self.nodata
    }

    /// GDAL 포맷 georeference 텍스트 (RFC §6.8 순서:
    /// scaleX, skewY, skewX, scaleY, upperLeftX, upperLeftY — %.6f, 줄바꿈 구분).
    pub fn georeference_gdal(&self) -> Option<String> {
        self.georef.as_ref().map(|g| {
            let (sx, sy) = g.scale_gdal();
            let (kx, ky) = g.skew();
            format!(
                "{sx:.6}\n{ky:.6}\n{kx:.6}\n{sy:.6}\n{:.6}\n{:.6}",
                g.origin_x, g.origin_y
            )
        })
    }
}

impl Georef {
    /// GDAL 순서 스케일 (scaleX, scaleY) — north-up 관례로 y 는 음수.
    pub fn scale_gdal(&self) -> (f64, f64) {
        (self.pixel_x, -self.pixel_y)
    }

    /// (skewX, skewY) — ModelPixelScale+Tiepoint 경로만 지원하므로 항상 0.
    pub fn skew(&self) -> (f64, f64) {
        (0.0, 0.0)
    }

    /// 월드 좌표 → **1-based** 그리드 (col, row) — Sedona 규약 (RFC §6.8).
    ///
    /// 순수 변환: 경계 검사 없음 (extent 밖은 0 이하/초과 좌표로 환산).
    /// floor 격자 — 픽셀 좌상단 코너는 그 픽셀에 속한다.
    pub fn world_to_raster(&self, x: f64, y: f64) -> (i64, i64) {
        let col = ((x - self.origin_x) / self.pixel_x).floor() as i64 + 1;
        let row = ((self.origin_y - y) / self.pixel_y).floor() as i64 + 1;
        (col, row)
    }

    /// **1-based** 그리드 (col, row) → 그 픽셀 좌상단 코너의 월드 좌표.
    pub fn raster_to_world(&self, col: i64, row: i64) -> (f64, f64) {
        (
            self.origin_x + (col - 1) as f64 * self.pixel_x,
            self.origin_y - (row - 1) as f64 * self.pixel_y,
        )
    }
}

/// GDAL_NODATA 태그 문자열 파싱 — 공백 trim, "nan"(대소문자 무관) 지원.
fn parse_gdal_nodata(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("nan") {
        return Some(f64::NAN);
    }
    s.parse().ok()
}

/// read_cog() 한 행 — RFC §6.4 가벼운 컬럼 부분집합.
///
/// cols/rows 는 TIFF 물리 타일 크기(엣지 클리핑 아님). bbox 는 반대로
/// **데이터 범위** — 엣지 타일은 이미지 경계로 클립된다 (§6.6 pruning 용도).
#[derive(Debug, Clone, PartialEq)]
pub struct TileRow {
    pub id: u64,
    pub level: u8,
    pub tile_x: u32,
    pub tile_y: u32,
    pub cols: u32,
    pub rows: u32,
    /// [xmin, ymin, xmax, ymax] — georef 부재 시 None.
    pub bbox: Option<[f64; 4]>,
}

#[derive(Debug)]
pub enum MetaError {
    /// 타일 구조가 없는 IFD — COG 가 아니다.
    NotTiled { level: usize },
    /// pack_tile_key 표현 범위 초과 (level 255 / 타일 인덱스 24bit).
    KeyOverflow(String),
    /// TIFF 파싱 실패 (async-tiff 에러 문자열).
    Tiff(String),
    /// bbox 필터가 유효하지 않다 (역전·비유한값).
    InvalidFilter(String),
    /// georef 없는 COG 에 bbox 필터 — 0행 침묵 대신 명시적 거부.
    FilterWithoutGeoref,
    /// georef 없는 COG 에 월드 좌표 픽셀 조회 — 좌표 해석 불가.
    NotGeoreferenced,
}

impl std::fmt::Display for MetaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetaError::NotTiled { level } => {
                write!(f, "IFD {level} is not tiled — not a valid COG")
            }
            MetaError::KeyOverflow(msg) => write!(f, "tile key overflow: {msg}"),
            MetaError::Tiff(msg) => write!(f, "TIFF read error: {msg}"),
            MetaError::InvalidFilter(msg) => write!(f, "invalid bbox filter: {msg}"),
            MetaError::FilterWithoutGeoref => {
                write!(
                    f,
                    "bbox filter requires a georeferenced COG (no geo tags found)"
                )
            }
            MetaError::NotGeoreferenced => {
                write!(
                    f,
                    "coordinate lookup requires a georeferenced COG (no geo tags found)"
                )
            }
        }
    }
}

impl std::error::Error for MetaError {}

/// COG 메타데이터를 읽어 레벨별 타일 그리드와 georeference 를 구성한다.
///
/// async-tiff 호출은 engine 내부(meta/source/pixel)에만 존재한다 (RFC R8).
/// 픽셀까지 읽을 거면 IFD 를 보관하는 [`crate::open_cog`] 를 쓴다.
pub async fn read_cog_meta<S: ByteSource>(source: S) -> Result<CogMeta, MetaError> {
    let fetch = new_metadata_fetch(FetchAdapter(std::sync::Arc::new(source)));
    let ifds = read_ifds(&fetch).await?;
    build_meta(&ifds)
}

/// 메타데이터 fetch 구성 — readahead 필수: async-tiff 리더는 태그 단위로 잘게
/// 읽어서, 캐시 없이는 나열 한 번에 수백 fetch (T5 fetch_contract 가 회귀 감시).
pub(crate) fn new_metadata_fetch<S: ByteSource>(
    adapter: FetchAdapter<S>,
) -> ReadaheadMetadataCache<FetchAdapter<S>> {
    ReadaheadMetadataCache::new(adapter)
}

/// TIFF 를 열어 전체 IFD 체인을 읽는다.
pub(crate) async fn read_ifds<S: ByteSource>(
    fetch: &ReadaheadMetadataCache<FetchAdapter<S>>,
) -> Result<Vec<async_tiff::ImageFileDirectory>, MetaError> {
    let mut reader = TiffMetadataReader::try_open(fetch)
        .await
        .map_err(|e| MetaError::Tiff(e.to_string()))?;
    reader
        .read_all_ifds(fetch)
        .await
        .map_err(|e| MetaError::Tiff(e.to_string()))
}

/// 읽어 둔 IFD 체인에서 [`CogMeta`] 를 구성한다 (IO 없음).
pub(crate) fn build_meta(ifds: &[async_tiff::ImageFileDirectory]) -> Result<CogMeta, MetaError> {
    // 밴드 수·nodata 도 IFD0 기준 (GDAL 관행상 전 밴드 공통)
    let num_bands = ifds
        .first()
        .map(|ifd0| u32::from(ifd0.samples_per_pixel()))
        .unwrap_or(0);
    let nodata = ifds
        .first()
        .and_then(|ifd0| ifd0.gdal_nodata())
        .and_then(parse_gdal_nodata);

    // georeference 는 IFD0 에서만 (GDAL COG 관행 — 오버뷰엔 geo 태그가 없다)
    let georef = ifds.first().and_then(|ifd0| {
        let scale = ifd0.model_pixel_scale()?;
        let tie = ifd0.model_tiepoint()?;
        if scale.len() < 2 || tie.len() < 6 {
            return None;
        }
        let epsg = ifd0
            .geo_key_directory()
            .and_then(|g| g.projected_type.or(g.geographic_type))
            .map(u32::from);
        // tiepoint = [i, j, k, x, y, z]: 래스터 (i,j) ↔ 모델 (x,y)
        Some(Georef {
            epsg,
            origin_x: tie[3] - tie[0] * scale[0],
            origin_y: tie[4] + tie[1] * scale[1],
            pixel_x: scale[0],
            pixel_y: scale[1],
        })
    });

    let mut levels = Vec::with_capacity(ifds.len());
    for (i, ifd) in ifds.iter().enumerate() {
        let level = u8::try_from(i)
            .map_err(|_| MetaError::KeyOverflow(format!("more than 256 levels ({i})")))?;
        let (tile_width, tile_height) = match (ifd.tile_width(), ifd.tile_height()) {
            (Some(w), Some(h)) => (w, h),
            _ => return Err(MetaError::NotTiled { level: i }),
        };
        let (tiles_x, tiles_y) = ifd.tile_count().ok_or(MetaError::NotTiled { level: i })?;
        let (tiles_x, tiles_y) = (tiles_x as u32, tiles_y as u32);
        if tiles_x > MAX_TILE_INDEX + 1 || tiles_y > MAX_TILE_INDEX + 1 {
            return Err(MetaError::KeyOverflow(format!(
                "tile grid {tiles_x}x{tiles_y} exceeds 24-bit index"
            )));
        }
        levels.push(LevelMeta {
            level,
            image_width: ifd.image_width(),
            image_height: ifd.image_height(),
            tile_width,
            tile_height,
            tiles_x,
            tiles_y,
        });
    }
    Ok(CogMeta {
        levels,
        georef,
        num_bands,
        nodata,
    })
}

/// 한 타일의 데이터 bbox — 이미지 경계로 클립, 레벨 픽셀 크기는 크기 비율로 유도.
fn tile_bbox(g: &Georef, base: (f64, f64), l: &LevelMeta, tx: u32, ty: u32) -> [f64; 4] {
    let sx = g.pixel_x * (base.0 / l.image_width as f64);
    let sy = g.pixel_y * (base.1 / l.image_height as f64);
    let px_min = (tx as u64 * l.tile_width as u64) as f64;
    let px_max = ((tx as u64 + 1) * l.tile_width as u64).min(l.image_width as u64) as f64;
    let py_min = (ty as u64 * l.tile_height as u64) as f64;
    let py_max = ((ty as u64 + 1) * l.tile_height as u64).min(l.image_height as u64) as f64;
    [
        g.origin_x + px_min * sx,
        g.origin_y - py_max * sy,
        g.origin_x + px_max * sx,
        g.origin_y - py_min * sy,
    ]
}

/// 닫힌 구간 bbox 교차 (경계 접촉 포함) — §6.6 pruning 의 판정 술어.
fn bbox_intersects(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}

/// [`enumerate_tiles`] 에 bbox 필터를 적용한다 (RFC §6.6 pruning).
///
/// 계약 (T4 성질로 고정): 결과 = 전체 열거 후 닫힌 교차 필터와 동일 집합.
/// 퇴화(점/선) bbox 허용. georef 없는 COG 에 필터를 주면 0행 침묵 대신 에러.
pub fn enumerate_tiles_filtered(
    meta: &CogMeta,
    filter: Option<[f64; 4]>,
) -> Result<Vec<TileRow>, MetaError> {
    let Some(f) = filter else {
        return Ok(enumerate_tiles(meta).collect());
    };
    if !f.iter().all(|v| v.is_finite()) || f[0] > f[2] || f[1] > f[3] {
        return Err(MetaError::InvalidFilter(format!(
            "[{}, {}, {}, {}] must be finite with xmin<=xmax and ymin<=ymax",
            f[0], f[1], f[2], f[3]
        )));
    }
    if meta.georef.is_none() {
        return Err(MetaError::FilterWithoutGeoref);
    }
    Ok(enumerate_tiles(meta)
        .filter(|r| r.bbox.is_some_and(|b| bbox_intersects(&b, &f)))
        .collect())
}

/// 레벨→행(y)→열(x) 순서로 전 타일을 나열한다. id 는 [`pack_tile_key`].
///
/// `read_cog_meta` 가 인덱스 범위를 검증했으므로 packing 은 실패하지 않는다.
pub fn enumerate_tiles(meta: &CogMeta) -> impl Iterator<Item = TileRow> + '_ {
    let base = meta
        .levels
        .first()
        .map(|l0| (l0.image_width as f64, l0.image_height as f64));
    meta.levels.iter().flat_map(move |l| {
        (0..l.tiles_y).flat_map(move |y| {
            (0..l.tiles_x).map(move |x| TileRow {
                id: pack_tile_key(l.level, x, y).expect("validated by read_cog_meta"),
                level: l.level,
                tile_x: x,
                tile_y: y,
                cols: l.tile_width,
                rows: l.tile_height,
                bbox: match (&meta.georef, base) {
                    (Some(g), Some(b)) => Some(tile_bbox(g, b, l, x, y)),
                    _ => None,
                },
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(level: u8, w: u32, h: u32, tx: u32, ty: u32) -> LevelMeta {
        LevelMeta {
            level,
            image_width: w,
            image_height: h,
            tile_width: 256,
            tile_height: 256,
            tiles_x: tx,
            tiles_y: ty,
        }
    }

    fn basic_meta(georef: Option<Georef>) -> CogMeta {
        // basic_512x512_u16 픽스처와 동형: 레벨0 2x2, 오버뷰 1x1, 1밴드, nodata 0
        CogMeta {
            levels: vec![level(0, 512, 512, 2, 2), level(1, 256, 256, 1, 1)],
            georef,
            num_bands: 1,
            nodata: Some(0.0),
        }
    }

    fn utm52(origin_x: f64, origin_y: f64) -> Georef {
        Georef {
            epsg: Some(32652),
            origin_x,
            origin_y,
            pixel_x: 10.0,
            pixel_y: 10.0,
        }
    }

    #[test]
    fn enumerates_all_levels_in_row_major_order() {
        let rows: Vec<TileRow> = enumerate_tiles(&basic_meta(None)).collect();
        assert_eq!(rows.len(), 5);
        let coords: Vec<(u8, u32, u32)> =
            rows.iter().map(|r| (r.level, r.tile_x, r.tile_y)).collect();
        assert_eq!(
            coords,
            [(0, 0, 0), (0, 1, 0), (0, 0, 1), (0, 1, 1), (1, 0, 0)]
        );
        // id = pack_tile_key 왕복 (E2E 기대값과 동일한 수치)
        assert_eq!(rows[1].id, 16777216);
        assert_eq!(rows[4].id, 281474976710656);
        assert!(rows.iter().all(|r| (r.cols, r.rows) == (256, 256)));
    }

    #[test]
    fn no_georef_yields_null_bbox() {
        assert!(enumerate_tiles(&basic_meta(None)).all(|r| r.bbox.is_none()));
        assert_eq!(basic_meta(None).crs(), None);
    }

    #[test]
    fn bbox_values_match_fixture_expectations() {
        // basic 픽스처와 동일 수치 — sqllogictest 기대값과 3중 대조 (rasterio 포함)
        let meta = basic_meta(Some(utm52(300000.0, 4000000.0)));
        let rows: Vec<TileRow> = enumerate_tiles(&meta).collect();
        assert_eq!(
            rows[0].bbox,
            Some([300000.0, 3997440.0, 302560.0, 4000000.0])
        );
        assert_eq!(
            rows[3].bbox,
            Some([302560.0, 3994880.0, 305120.0, 3997440.0])
        );
        // 오버뷰(20m 유도)는 전체 범위
        assert_eq!(
            rows[4].bbox,
            Some([300000.0, 3994880.0, 305120.0, 4000000.0])
        );
        assert_eq!(meta.crs(), Some("EPSG:32652".to_string()));
    }

    #[test]
    fn edge_tiles_clip_to_image_extent() {
        // edge_400x300_u16 픽스처와 동형: 우/하단 클립 + 오버뷰 200x150
        let meta = CogMeta {
            levels: vec![level(0, 400, 300, 2, 2), level(1, 200, 150, 1, 1)],
            georef: Some(utm52(500000.0, 3800000.0)),
            num_bands: 1,
            nodata: Some(0.0),
        };
        let rows: Vec<TileRow> = enumerate_tiles(&meta).collect();
        assert_eq!(
            rows[1].bbox,
            Some([502560.0, 3797440.0, 504000.0, 3800000.0])
        );
        assert_eq!(
            rows[3].bbox,
            Some([502560.0, 3797000.0, 504000.0, 3797440.0])
        );
        assert_eq!(
            rows[4].bbox,
            Some([500000.0, 3797000.0, 504000.0, 3800000.0])
        );
        // 물리 타일 크기는 클립되지 않는다
        assert!(rows.iter().all(|r| (r.cols, r.rows) == (256, 256)));
    }

    // ---- RS_ 메타데이터 접근자 재료 (RFC §6.8 Phase 1) ----

    #[test]
    fn dimension_accessors_read_level0() {
        let meta = basic_meta(None);
        assert_eq!(meta.width(), Some(512));
        assert_eq!(meta.height(), Some(512));
        let empty = CogMeta {
            levels: vec![],
            georef: None,
            num_bands: 0,
            nodata: None,
        };
        assert_eq!(empty.width(), None);
        assert_eq!(empty.height(), None);
    }

    #[test]
    fn srid_follows_sedona_zero_convention() {
        // Sedona/PostGIS 관례: SRID 부재 = 0 (crs 컬럼의 NULL 과 다름 — 문서화된 차이)
        assert_eq!(basic_meta(Some(utm52(0.0, 0.0))).srid(), 32652);
        assert_eq!(basic_meta(None).srid(), 0);
        let no_epsg = Georef {
            epsg: None,
            ..utm52(0.0, 0.0)
        };
        assert_eq!(basic_meta(Some(no_epsg)).srid(), 0);
    }

    #[test]
    fn band_nodata_is_one_based_and_null_out_of_range() {
        let meta = basic_meta(None);
        assert_eq!(meta.band_nodata(1), Some(0.0));
        assert_eq!(meta.band_nodata(0), None, "0 은 범위 밖 (1-based)");
        assert_eq!(meta.band_nodata(2), None, "밴드 1개뿐");
        // multiband_64x64_u8 픽스처와 동형: 3밴드, nodata 미설정
        let mb = CogMeta {
            levels: vec![level(0, 64, 64, 1, 1)],
            georef: Some(utm52(600000.0, 3900000.0)),
            num_bands: 3,
            nodata: None,
        };
        assert_eq!(mb.band_nodata(1), None);
        assert_eq!(mb.band_nodata(3), None);
        assert_eq!(mb.band_nodata(4), None);
    }

    #[test]
    fn gdal_scale_is_negative_y() {
        let g = utm52(300000.0, 4000000.0);
        assert_eq!(g.scale_gdal(), (10.0, -10.0));
        assert_eq!(
            g.skew(),
            (0.0, 0.0),
            "ModelPixelScale+Tiepoint 경로는 skew 없음"
        );
    }

    #[test]
    fn georeference_gdal_text_matches_sqllogictest_expectation() {
        // test/sql/rs_metadata.test 의 기대값과 동일 문자열 (3중 대조)
        let meta = basic_meta(Some(utm52(300000.0, 4000000.0)));
        assert_eq!(
            meta.georeference_gdal().as_deref(),
            Some("10.000000\n0.000000\n0.000000\n-10.000000\n300000.000000\n4000000.000000")
        );
        assert_eq!(basic_meta(None).georeference_gdal(), None);
    }

    #[test]
    fn nodata_string_parsing() {
        assert_eq!(parse_gdal_nodata("0"), Some(0.0));
        assert_eq!(parse_gdal_nodata(" -9999 "), Some(-9999.0));
        assert_eq!(parse_gdal_nodata("1.5"), Some(1.5));
        assert!(parse_gdal_nodata("nan").is_some_and(f64::is_nan));
        assert!(parse_gdal_nodata("NaN").is_some_and(f64::is_nan));
        assert_eq!(parse_gdal_nodata("abc"), None);
        assert_eq!(parse_gdal_nodata(""), None);
    }
}
