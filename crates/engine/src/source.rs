//! reader 경계 (RFC R8): async-tiff 직접 호출은 이 모듈 뒤에서만.
//!
//! 외부(익스텐션 크레이트)는 [`ByteSource`] 만 구현한다 — async-tiff 타입은
//! engine 밖으로 새지 않는다.

use std::fmt::Debug;
use std::ops::Range;

use async_tiff::error::{AsyncTiffError, AsyncTiffResult};
use bytes::Bytes;
use futures::future::BoxFuture;

/// 바이트 range 를 비동기로 공급하는 소스. 로컬 파일·object store·인메모리 등
/// 어떤 저장소든 이 trait 하나로 engine 에 연결된다.
pub trait ByteSource: Debug + Send + Sync + 'static {
    /// `range` 의 바이트를 반환한다. **EOF 를 넘는 요청은 가용 분까지 클램프**
    /// (object_store/HTTP Range 의미론) — readahead 계층이 파일 길이를 모른 채
    /// 선요청하기 때문에 에러가 아니라 짧은 버퍼가 계약이다.
    /// `range.start` 가 EOF 이상이거나 역전된 range 는 에러.
    fn fetch(&self, range: Range<u64>) -> BoxFuture<'_, Result<Bytes, SourceError>>;
}

/// 소스 구현을 런타임에 고르는 호출자(스킴 디스패치)를 위한 boxed 위임.
impl ByteSource for Box<dyn ByteSource> {
    fn fetch(&self, range: Range<u64>) -> BoxFuture<'_, Result<Bytes, SourceError>> {
        (**self).fetch(range)
    }
}

/// [`ByteSource`] 구현이 반환하는 에러 (경로/원인 문자열 포함).
#[derive(Debug)]
pub struct SourceError(pub String);

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SourceError {}

/// 인메모리 소스 — 단위 테스트와 향후 WASM(브라우저 버퍼) 경로용.
#[derive(Debug, Clone)]
pub struct MemorySource(Bytes);

impl MemorySource {
    pub fn new(data: impl Into<Bytes>) -> Self {
        Self(data.into())
    }
}

impl ByteSource for MemorySource {
    fn fetch(&self, range: Range<u64>) -> BoxFuture<'_, Result<Bytes, SourceError>> {
        Box::pin(async move {
            let len = self.0.len() as u64;
            if range.start >= len || range.start > range.end {
                return Err(SourceError(format!(
                    "range {}..{} out of bounds (len {})",
                    range.start, range.end, len
                )));
            }
            // EOF 클램프 (trait 계약): end 만 넘으면 가용 분 반환
            let end = range.end.min(len);
            Ok(self.0.slice(range.start as usize..end as usize))
        })
    }
}

/// 문서 전체를 읽는다 — 길이를 모르는 [`ByteSource`] 에서 점진 배증으로
/// (EOF 클램프 계약 활용: 반환이 요청보다 짧으면 끝에 닿은 것).
/// STAC 등 통짜 JSON 문서용. 64MiB 초과는 에러 (COG 메타가 아닌 오용 방지).
pub async fn fetch_all<S: ByteSource>(source: &S) -> Result<Bytes, SourceError> {
    let mut cap: u64 = 64 * 1024;
    loop {
        let bytes = source.fetch(0..cap).await?;
        if (bytes.len() as u64) < cap {
            return Ok(bytes);
        }
        if cap >= 64 * 1024 * 1024 {
            return Err(SourceError("document exceeds 64MiB".into()));
        }
        cap *= 4;
    }
}

/// [`ByteSource`] → async-tiff 어댑터 (engine 내부 전용).
///
/// 메타데이터 경로(`MetadataFetch`, readahead 캐시에 소유됨)와 픽셀 경로
/// (`AsyncFileReader`, `CogReader` 가 보유)가 같은 소스를 공유하도록
/// `Arc` 로 감싼다 — clone 은 참조 복제.
#[derive(Debug)]
pub(crate) struct FetchAdapter<S: ByteSource>(pub(crate) std::sync::Arc<S>);

impl<S: ByteSource> Clone for FetchAdapter<S> {
    fn clone(&self) -> Self {
        Self(std::sync::Arc::clone(&self.0))
    }
}

// AsyncFileReader 하나로 픽셀(타일) 경로와 메타데이터 경로를 모두 커버한다 —
// async-tiff 가 `impl<T: AsyncFileReader> MetadataFetch for T` blanket 을 제공.
// 메타데이터 쪽은 반드시 readahead 캐시로 감싸 쓴다 (meta::new_metadata_fetch).
#[async_trait::async_trait]
impl<S: ByteSource> async_tiff::reader::AsyncFileReader for FetchAdapter<S> {
    async fn get_bytes(&self, range: Range<u64>) -> AsyncTiffResult<Bytes> {
        self.0
            .fetch(range)
            .await
            .map_err(|e| AsyncTiffError::External(Box::new(e)))
    }
}
