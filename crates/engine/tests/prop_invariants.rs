//! T4 property-based 불변식 (RFC §6.9): 타일 키 산술·bbox 클리핑·오버뷰 유도.
//!
//! 예시 기반 단위 테스트(meta.rs)가 특정 수치를 고정한다면, 여기는 임의 입력에서
//! 성립해야 하는 **성질**을 고정한다 — "임의 그리드에서 bbox 는 extent 를 벗어나지
//! 않고, 인접 타일은 정확히 이어지며, 키 왕복은 항등".

use engine::{
    enumerate_tiles, enumerate_tiles_filtered, pack_tile_key, unpack_tile_key, CogMeta, Georef,
    LevelMeta, MAX_TILE_INDEX,
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

    /// RFC §6.9 T4 원문 그대로: "임의 bbox 에 대해 (pushdown 결과) ⊆ (전체 스캔 후
    /// 필터 결과)이며 두 집합은 동일". 교차 판정은 테스트가 독립적으로 재유도한다 —
    /// 향후 Hilbert/Morton pruning(§6.6) 최적화가 들어와도 이 성질은 유지돼야 한다.
    #[test]
    fn bbox_pushdown_equals_full_scan_then_filter(
        w in 1u32..3000, h in 1u32..3000,
        tw in prop::sample::select(vec![128u32, 256, 512]),
        ox in -1.0e6f64..1.0e6, oy in -1.0e6f64..1.0e6,
        px in 0.1f64..100.0,
        // extent 안팎을 두루 찍도록 필터 코너를 extent 비율로 뽑는다 (밖 포함 -0.5..1.5)
        fx0 in -0.5f64..1.5, fy0 in -0.5f64..1.5,
        fdx in 0.0f64..1.0, fdy in 0.0f64..1.0,
    ) {
        let g = Georef { epsg: None, origin_x: ox, origin_y: oy, pixel_x: px, pixel_y: px };
        let m = meta(w, h, tw, tw, Some(g));
        let (ew, eh) = (w as f64 * px, h as f64 * px);
        let f = [
            ox + fx0 * ew,
            oy - eh + fy0 * eh,
            ox + (fx0 + fdx) * ew,
            oy - eh + (fy0 + fdy) * eh,
        ];
        // 독립 재유도한 닫힌 교차 술어 (구현과 별개 식)
        let hits = |b: [f64; 4]| !(b[2] < f[0] || b[0] > f[2] || b[3] < f[1] || b[1] > f[3]);
        let pushed = enumerate_tiles_filtered(&m, Some(f)).expect("유효 필터");
        let scanned: Vec<_> = enumerate_tiles(&m)
            .filter(|r| r.bbox.is_some_and(hits))
            .collect();
        prop_assert_eq!(pushed, scanned);
    }

    /// 필터 없음(None)은 전체 열거와 동일.
    #[test]
    fn no_filter_equals_full_enumeration(w in 1u32..2000, h in 1u32..2000) {
        let g = Georef { epsg: None, origin_x: 0.0, origin_y: 0.0, pixel_x: 1.0, pixel_y: 1.0 };
        let m = meta(w, h, 256, 256, Some(g));
        let full: Vec<_> = enumerate_tiles(&m).collect();
        prop_assert_eq!(enumerate_tiles_filtered(&m, None).expect("필터 없음"), full);
    }
}

proptest! {
    /// 좌표 왕복 (RFC §6.9 T4 유사): raster→world 코너에 픽셀 내부 오프셋을 더한
    /// 점은 같은 픽셀로 돌아온다. **정확한 코너의 항등은 임의 실수 해상도에선
    /// 부동소수로 보장 불가** (예: 50422*py/py < 50422) — rasterio 도 동일한 float
    /// 의미론이라 스냅 보정 없이 그대로 둔다 (T1 오라클 정합 우선). 코너 항등은
    /// 아래 정수 격자 프로퍼티가 보장한다.
    #[test]
    fn coord_roundtrip_interior_points(
        col in 1i64..100_000, row in 1i64..100_000,
        ox in -1.0e6f64..1.0e6, oy in -1.0e6f64..1.0e6,
        px in 0.1f64..100.0, py in 0.1f64..100.0,
        fx in 0.01f64..0.99, fy in 0.01f64..0.99,
    ) {
        let g = Georef { epsg: None, origin_x: ox, origin_y: oy, pixel_x: px, pixel_y: py };
        let (wx, wy) = g.raster_to_world(col, row);
        let (ix, iy) = (wx + fx * px, wy - fy * py);
        prop_assert_eq!(g.world_to_raster(ix, iy), (col, row), "내부점 왕복");
    }

    /// 정수 origin·해상도(우리 픽스처 계열, 위성영상 관행)에서는 코너 왕복도 항등.
    #[test]
    fn coord_roundtrip_exact_on_integer_grids(
        col in 1i64..1_000_000, row in 1i64..1_000_000,
        ox in -1_000_000i64..1_000_000, oy in -1_000_000i64..1_000_000,
        px in 1i64..1000, py in 1i64..1000,
    ) {
        let g = Georef {
            epsg: None,
            origin_x: ox as f64,
            origin_y: oy as f64,
            pixel_x: px as f64,
            pixel_y: py as f64,
        };
        let (wx, wy) = g.raster_to_world(col, row);
        prop_assert_eq!(g.world_to_raster(wx, wy), (col, row), "정수 격자 코너 왕복");
    }
}

/// 필터 오류 경로 — 예시 기반이 더 명료한 계약들.
#[test]
fn bbox_filter_error_paths() {
    let g = Georef {
        epsg: None,
        origin_x: 0.0,
        origin_y: 0.0,
        pixel_x: 1.0,
        pixel_y: 1.0,
    };
    let with_geo = meta(512, 512, 256, 256, Some(g));
    let no_geo = meta(512, 512, 256, 256, None);
    // georef 없는 파일에 bbox 필터 → 명시적 에러 (0행 침묵 금지)
    assert!(enumerate_tiles_filtered(&no_geo, Some([0.0, 0.0, 1.0, 1.0])).is_err());
    // min > max 역전 → 에러
    assert!(enumerate_tiles_filtered(&with_geo, Some([1.0, 0.0, 0.0, 1.0])).is_err());
    assert!(enumerate_tiles_filtered(&with_geo, Some([0.0, 1.0, 1.0, 0.0])).is_err());
    // 비유한값 → 에러
    assert!(enumerate_tiles_filtered(&with_geo, Some([f64::NAN, 0.0, 1.0, 1.0])).is_err());
    // 퇴화(점/선) bbox 는 허용 — 닫힌 교차라 의미 있음
    assert!(enumerate_tiles_filtered(&with_geo, Some([10.0, -10.0, 10.0, -10.0])).is_ok());
}
