//! 타일-테이블 도메인 모델 (RFC §6.4): COG 메타데이터 → 레벨/타일 그리드.
//!
//! 픽셀은 건드리지 않는다 — IFD 메타데이터만 읽어 read_cog() 의 행을 만든다.

use async_tiff::metadata::TiffMetadataReader;

use crate::source::{ByteSource, FetchAdapter};
use crate::{pack_tile_key, MAX_TILE_INDEX};

/// COG 한 레벨(본체 IFD 또는 오버뷰 IFD)의 타일 그리드 메타데이터.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelMeta {
    pub level: u8,
    pub image_width: u32,
    pub image_height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
}

/// COG 전체 메타데이터 — 레벨 순서는 IFD 순서(본체=0, 오버뷰=1..).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CogMeta {
    pub levels: Vec<LevelMeta>,
}

/// read_cog() 한 행 — RFC §6.4 가벼운 컬럼 부분집합.
/// cols/rows 는 TIFF 물리 타일 크기(엣지 클리핑 아님).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileRow {
    pub id: u64,
    pub level: u8,
    pub tile_x: u32,
    pub tile_y: u32,
    pub cols: u32,
    pub rows: u32,
}

#[derive(Debug)]
pub enum MetaError {
    /// 타일 구조가 없는 IFD — COG 가 아니다.
    NotTiled { level: usize },
    /// pack_tile_key 표현 범위 초과 (level 255 / 타일 인덱스 24bit).
    KeyOverflow(String),
    /// TIFF 파싱 실패 (async-tiff 에러 문자열).
    Tiff(String),
}

impl std::fmt::Display for MetaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetaError::NotTiled { level } => {
                write!(f, "IFD {level} is not tiled — not a valid COG")
            }
            MetaError::KeyOverflow(msg) => write!(f, "tile key overflow: {msg}"),
            MetaError::Tiff(msg) => write!(f, "TIFF read error: {msg}"),
        }
    }
}

impl std::error::Error for MetaError {}

/// COG 메타데이터를 읽어 레벨별 타일 그리드를 구성한다.
///
/// async-tiff 호출은 여기(와 source.rs 어댑터)에만 존재한다 (RFC R8).
pub async fn read_cog_meta<S: ByteSource>(source: S) -> Result<CogMeta, MetaError> {
    let fetch = FetchAdapter(source);
    let mut reader = TiffMetadataReader::try_open(&fetch)
        .await
        .map_err(|e| MetaError::Tiff(e.to_string()))?;
    let ifds = reader
        .read_all_ifds(&fetch)
        .await
        .map_err(|e| MetaError::Tiff(e.to_string()))?;

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
    Ok(CogMeta { levels })
}

/// 레벨→행(y)→열(x) 순서로 전 타일을 나열한다. id 는 [`pack_tile_key`].
///
/// `read_cog_meta` 가 인덱스 범위를 검증했으므로 packing 은 실패하지 않는다.
pub fn enumerate_tiles(meta: &CogMeta) -> impl Iterator<Item = TileRow> + '_ {
    meta.levels.iter().flat_map(|l| {
        (0..l.tiles_y).flat_map(move |y| {
            (0..l.tiles_x).map(move |x| TileRow {
                id: pack_tile_key(l.level, x, y).expect("validated by read_cog_meta"),
                level: l.level,
                tile_x: x,
                tile_y: y,
                cols: l.tile_width,
                rows: l.tile_height,
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic_meta() -> CogMeta {
        // basic_512x512_u16 픽스처와 동형: 레벨0 2x2, 오버뷰 1x1
        CogMeta {
            levels: vec![
                LevelMeta {
                    level: 0,
                    image_width: 512,
                    image_height: 512,
                    tile_width: 256,
                    tile_height: 256,
                    tiles_x: 2,
                    tiles_y: 2,
                },
                LevelMeta {
                    level: 1,
                    image_width: 256,
                    image_height: 256,
                    tile_width: 256,
                    tile_height: 256,
                    tiles_x: 1,
                    tiles_y: 1,
                },
            ],
        }
    }

    #[test]
    fn enumerates_all_levels_in_row_major_order() {
        let rows: Vec<TileRow> = enumerate_tiles(&basic_meta()).collect();
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
    fn empty_meta_yields_no_rows() {
        let meta = CogMeta { levels: vec![] };
        assert_eq!(enumerate_tiles(&meta).count(), 0);
    }
}
