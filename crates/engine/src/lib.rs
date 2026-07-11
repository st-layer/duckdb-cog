//! duckdb-cog 엔진 크레이트 (임시명 `engine`).
//!
//! async-tiff 위의 얇은 도메인 계층: 타일-테이블 모델, 오버뷰 선택,
//! 공간 pruning 키, 통계 매핑. 설계 준거는 docs/RFC-001-rev3.md.
//!
//! 불변식: GDAL/PROJ/GEOS 링크 금지(N4), TIFF 파싱 재구현 금지(N7),
//! wasm32-unknown-unknown 컴파일 가능 유지(G8).

#[cfg(feature = "reader")]
mod meta;
#[cfg(feature = "reader")]
mod pixel;
#[cfg(feature = "reader")]
mod source;
#[cfg(feature = "reader")]
mod stac;

#[cfg(feature = "reader")]
pub use meta::{
    enumerate_tiles, enumerate_tiles_filtered, read_cog_meta, CogMeta, Georef, LevelMeta,
    MetaError, TileRow,
};
#[cfg(feature = "reader")]
pub use pixel::{apply_nodata, normalized_difference, open_cog, CogReader, ZonalStats};
#[cfg(feature = "reader")]
pub use source::{fetch_all, ByteSource, MemorySource, SourceError};
#[cfg(feature = "reader")]
pub use stac::{parse_stac, StacAssetRow, StacError};
// ByteSource 구현자가 시그니처 타입(Bytes, BoxFuture)과 block_on 을 별도 의존성
// 없이 쓰도록 재수출.
#[cfg(feature = "reader")]
pub use {bytes, futures};

/// pack_tile_key 가 표현 가능한 최대 타일 인덱스 (24bit).
pub const MAX_TILE_INDEX: u32 = (1 << 24) - 1;

/// 타일 좌표를 단일 u64 키로 packing한다 (level 8bit | x 24bit | y 24bit).
///
/// 이후 Hilbert/Morton SFC 키(RFC §6.6)로 대체될 자리표시 구현이며,
/// 부트스트랩 단계에서 `just check` 가 실제로 무언가를 판정하게 하는 용도.
pub fn pack_tile_key(level: u8, tile_x: u32, tile_y: u32) -> Option<u64> {
    if tile_x > MAX_TILE_INDEX || tile_y > MAX_TILE_INDEX {
        return None;
    }
    Some(((level as u64) << 48) | ((tile_x as u64) << 24) | (tile_y as u64))
}

/// `pack_tile_key` 의 역변환.
pub fn unpack_tile_key(key: u64) -> (u8, u32, u32) {
    let level = (key >> 48) as u8;
    let x = ((key >> 24) & 0xFF_FFFF) as u32;
    let y = (key & 0xFF_FFFF) as u32;
    (level, x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_key_roundtrip() {
        for (l, x, y) in [(0u8, 0u32, 0u32), (3, 123, 456), (8, (1 << 24) - 1, 1)] {
            let key = pack_tile_key(l, x, y).expect("in range");
            assert_eq!(unpack_tile_key(key), (l, x, y));
        }
    }

    #[test]
    fn tile_key_rejects_out_of_range() {
        assert!(pack_tile_key(0, 1 << 24, 0).is_none());
        assert!(pack_tile_key(0, 0, 1 << 24).is_none());
    }
}
