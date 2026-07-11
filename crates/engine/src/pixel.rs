//! 픽셀 접근 (RFC §6.8 Phase 2 재료): 좌표 변환·타일 인덱싱·밴드 선택.
//!
//! decode/fetch 는 async-tiff 위임 (N7) — 직접 호출은 engine 내부(R8)에만.
//! 픽셀값은 변형 없이 그대로 (N2): 반올림·보간 없이 floor 격자 판독.

use std::sync::Arc;

use async_tiff::decoder::DecoderRegistry;
use async_tiff::tags::PlanarConfiguration;
use async_tiff::{Array, ImageFileDirectory, TypedArray};

use crate::meta::{build_meta, new_metadata_fetch, read_ifds, CogMeta, MetaError};
use crate::source::{ByteSource, FetchAdapter};

/// 열린 COG 핸들 — IFD 체인을 보관해 픽셀 fetch 에 재사용한다.
///
/// [`crate::read_cog_meta`] 와 달리 소스를 계속 소유한다 (타일 range-read 용).
#[derive(Debug)]
pub struct CogReader<S: ByteSource> {
    fetch: FetchAdapter<S>,
    ifds: Vec<ImageFileDirectory>,
    decoders: DecoderRegistry,
}

/// COG 를 열어 메타데이터와 픽셀 리더를 함께 얻는다.
pub async fn open_cog<S: ByteSource>(source: S) -> Result<(CogMeta, CogReader<S>), MetaError> {
    let adapter = FetchAdapter(Arc::new(source));
    let fetch = new_metadata_fetch(adapter.clone());
    let ifds = read_ifds(&fetch).await?;
    let meta = build_meta(&ifds)?;
    Ok((
        meta,
        CogReader {
            fetch: adapter,
            ifds,
            decoders: DecoderRegistry::default(),
        },
    ))
}

impl<S: ByteSource> CogReader<S> {
    /// level 0 월드 좌표 `(x, y)` 의 `band`(1-based) 픽셀값.
    ///
    /// NULL 규약 (RFC §6.8): extent 밖·범위 밖 밴드·nodata → `Ok(None)`.
    /// georef 없는 COG 는 월드 좌표를 해석할 수 없다 → 에러 (침묵 금지,
    /// bbox 필터와 동일 결정 — worklog 2026-07-11 참조).
    pub async fn read_pixel(
        &self,
        meta: &CogMeta,
        x: f64,
        y: f64,
        band: u32,
    ) -> Result<Option<f64>, MetaError> {
        let Some(g) = &meta.georef else {
            return Err(MetaError::NotGeoreferenced);
        };
        let (Some(l0), Some(ifd0)) = (meta.levels.first(), self.ifds.first()) else {
            return Ok(None);
        };
        if band == 0 || band > meta.num_bands {
            return Ok(None);
        }
        // floor 격자: 원점 코너는 픽셀 (0,0), 우/하단 경계 좌표는 밖
        let col = ((x - g.origin_x) / g.pixel_x).floor();
        let row = ((g.origin_y - y) / g.pixel_y).floor();
        if col < 0.0 || row < 0.0 || col >= l0.image_width as f64 || row >= l0.image_height as f64 {
            return Ok(None);
        }
        let (col, row) = (col as u64, row as u64);
        let tile = ifd0
            .fetch_tile(
                (col / l0.tile_width as u64) as usize,
                (row / l0.tile_height as u64) as usize,
                &self.fetch,
            )
            .await
            .map_err(|e| MetaError::Tiff(e.to_string()))?;
        let planar = ifd0.planar_configuration();
        let array = tile
            .decode(&self.decoders)
            .map_err(|e| MetaError::Tiff(e.to_string()))?;
        let value = sample_array(
            &array,
            planar,
            (row % l0.tile_height as u64) as usize,
            (col % l0.tile_width as u64) as usize,
            (band - 1) as usize,
        )
        .ok_or_else(|| {
            MetaError::Tiff(format!(
                "decoded tile shape {:?} does not contain pixel (row {}, col {}, band {})",
                array.shape(),
                row % l0.tile_height as u64,
                col % l0.tile_width as u64,
                band
            ))
        })?;
        Ok(apply_nodata(value, meta.nodata))
    }
}

/// 디코드된 타일 배열에서 한 픽셀을 읽는다.
///
/// shape 해석은 PlanarConfiguration 에 따른다 (async-tiff 문서):
/// chunky = (height, width, bands), planar = (bands, height, width).
fn sample_array(
    array: &Array,
    planar: PlanarConfiguration,
    row: usize,
    col: usize,
    band0: usize,
) -> Option<f64> {
    let [d0, d1, d2] = array.shape();
    let idx = match planar {
        PlanarConfiguration::Chunky => {
            if row >= d0 || col >= d1 || band0 >= d2 {
                return None;
            }
            (row * d1 + col) * d2 + band0
        }
        PlanarConfiguration::Planar => {
            if band0 >= d0 || row >= d1 || col >= d2 {
                return None;
            }
            (band0 * d1 + row) * d2 + col
        }
        // non_exhaustive: 알 수 없는 planar 배치는 판독 거부 (호출측이 에러로 승격)
        _ => return None,
    };
    Some(match array.data() {
        TypedArray::Bool(v) => u8::from(*v.get(idx)?) as f64,
        TypedArray::UInt8(v) => *v.get(idx)? as f64,
        TypedArray::UInt16(v) => *v.get(idx)? as f64,
        TypedArray::UInt32(v) => *v.get(idx)? as f64,
        TypedArray::UInt64(v) => *v.get(idx)? as f64,
        TypedArray::Int8(v) => *v.get(idx)? as f64,
        TypedArray::Int16(v) => *v.get(idx)? as f64,
        TypedArray::Int32(v) => *v.get(idx)? as f64,
        TypedArray::Int64(v) => *v.get(idx)? as f64,
        TypedArray::Float32(v) => *v.get(idx)? as f64,
        TypedArray::Float64(v) => *v.get(idx)?,
    })
}

/// nodata 매핑 (RFC §6.8): nodata 픽셀 → None. NaN nodata 는 NaN 픽셀과 짝.
pub fn apply_nodata(value: f64, nodata: Option<f64>) -> Option<f64> {
    match nodata {
        Some(nd) if value == nd || (nd.is_nan() && value.is_nan()) => None,
        _ => Some(value),
    }
}
