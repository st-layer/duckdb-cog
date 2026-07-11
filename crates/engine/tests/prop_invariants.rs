//! T4 property-based 불변식 (RFC §6.9): 타일 키 산술·bbox 클리핑·오버뷰 유도.
//!
//! 예시 기반 단위 테스트(meta.rs)가 특정 수치를 고정한다면, 여기는 임의 입력에서
//! 성립해야 하는 **성질**을 고정한다 — "임의 그리드에서 bbox 는 extent 를 벗어나지
//! 않고, 인접 타일은 정확히 이어지며, 키 왕복은 항등".

use engine::{
    enumerate_tiles, pack_tile_key, unpack_tile_key, CogMeta, Georef, LevelMeta, MAX_TILE_INDEX,
};
use proptest::prelude::*;

/// 이미지/타일 크기에서 일관된 LevelMeta (tiles = ceil — read_cog_meta 와 동일 규칙).
fn level(level: u8, w: u32, h: u32, tw: u32, th: u32) -> LevelMeta {
    LevelMeta {
        level,
        image_width: w,
        image_height: h,
        tile_width: tw,
        tile_height: th,
        tiles_x: w.div_ceil(tw),
        tiles_y: h.div_ceil(th),
    }
}

/// 본체 + 절반 크기 오버뷰 1단 (GDAL COG 관행, 홀수 올림).
fn meta(w: u32, h: u32, tw: u32, th: u32, georef: Option<Georef>) -> CogMeta {
    CogMeta {
        levels: vec![
            level(0, w, h, tw, th),
            level(1, w.div_ceil(2), h.div_ceil(2), tw, th),
        ],
        georef,
        num_bands: 1,
        nodata: None,
    }
}

proptest! {
    #[test]
    fn tile_key_roundtrip_is_identity(
        l in any::<u8>(),
        x in 0..=MAX_TILE_INDEX,
        y in 0..=MAX_TILE_INDEX,
    ) {
        let key = pack_tile_key(l, x, y).expect("표현 범위 내");
        prop_assert_eq!(unpack_tile_key(key), (l, x, y));
    }

    #[test]
    fn tile_key_rejects_out_of_range(
        l in any::<u8>(),
        over in MAX_TILE_INDEX + 1..=u32::MAX,
        ok in 0..=MAX_TILE_INDEX,
    ) {
        prop_assert!(pack_tile_key(l, over, ok).is_none());
        prop_assert!(pack_tile_key(l, ok, over).is_none());
    }

    /// 모든 레벨(오버뷰 포함)의 bbox 는 정렬돼 있고 level0 extent 안에 있으며,
    /// cols/rows 는 클리핑 없이 물리 타일 크기를 유지한다.
    #[test]
    fn bboxes_ordered_within_extent_and_tiles_physical(
        w in 1u32..5000, h in 1u32..5000,
        tw in prop::sample::select(vec![128u32, 256, 512]),
        th in prop::sample::select(vec![128u32, 256, 512]),
        ox in -1.0e7f64..1.0e7, oy in -1.0e7f64..1.0e7,
        px in 0.05f64..1000.0, py in 0.05f64..1000.0,
    ) {
        let g = Georef { epsg: Some(32652), origin_x: ox, origin_y: oy, pixel_x: px, pixel_y: py };
        let m = meta(w, h, tw, th, Some(g));
        let xmax_ext = ox + w as f64 * px;
        let ymin_ext = oy - h as f64 * py;
        // 오버뷰 스케일 유도(크기 비율 곱셈)의 부동소수 오차 허용치
        let eps = 1e-9 * (xmax_ext.abs() + ymin_ext.abs() + ox.abs() + oy.abs() + 1.0);
        for row in enumerate_tiles(&m) {
            let b = row.bbox.expect("georef 있음");
            prop_assert!(b[0] < b[2] && b[1] < b[3], "bbox 정렬 위반: {:?}", b);
            prop_assert!(b[0] >= ox - eps && b[2] <= xmax_ext + eps, "x extent 밖: {:?}", b);
            prop_assert!(b[3] <= oy + eps && b[1] >= ymin_ext - eps, "y extent 밖: {:?}", b);
            prop_assert_eq!((row.cols, row.rows), (tw, th), "물리 타일 크기 불변 위반");
        }
    }

    /// level0 에서 같은 행의 인접 타일 bbox 는 정확히 이어지고(동일 산술식 → 비트
    /// 동일), 마지막 열은 이미지 경계로 클립된다 — §6.6 pruning 이 기대는 성질.
    #[test]
    fn level0_row_tiles_are_seamless_and_clipped(
        w in 1u32..3000, h in 1u32..3000,
        tw in prop::sample::select(vec![128u32, 256, 512]),
        ox in -1.0e6f64..1.0e6, oy in -1.0e6f64..1.0e6,
        px in 0.1f64..100.0,
    ) {
        let g = Georef { epsg: None, origin_x: ox, origin_y: oy, pixel_x: px, pixel_y: px };
        let m = meta(w, h, tw, tw, Some(g));
        let l0: Vec<_> = enumerate_tiles(&m).filter(|r| r.level == 0).collect();
        let tiles_x = w.div_ceil(tw) as usize;
        prop_assert_eq!(l0.len(), tiles_x * h.div_ceil(tw) as usize);
        // enumerate_tiles 는 행 우선(y 바깥, x 안쪽) — chunks(tiles_x) = 한 행
        for row_tiles in l0.chunks(tiles_x) {
            for pair in row_tiles.windows(2) {
                prop_assert_eq!(
                    pair[0].bbox.unwrap()[2], pair[1].bbox.unwrap()[0],
                    "행 내 인접 타일 bbox 불연속"
                );
            }
            // 행의 마지막 타일 xmax == 이미지 경계 (level0 은 sx == px 로 정확)
            let last = row_tiles.last().unwrap().bbox.unwrap();
            prop_assert_eq!(last[2], ox + w as f64 * px, "마지막 열이 경계로 클립 안 됨");
        }
    }

    #[test]
    fn no_georef_means_all_bbox_null(w in 1u32..2000, h in 1u32..2000) {
        let m = meta(w, h, 256, 256, None);
        prop_assert!(enumerate_tiles(&m).all(|r| r.bbox.is_none()));
    }
}
