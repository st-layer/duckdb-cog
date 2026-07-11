//! reader 경계 (RFC R8): async-tiff 직접 호출은 이 모듈 뒤에서만.
//!
//! 외부(익스텐션 크레이트)는 [`ByteSource`] 만 구현한다 — async-tiff 타입은
//! engine 밖으로 새지 않는다.

use std::fmt::Debug;
use std::ops::Range;

use async_tiff::error::{AsyncTiffError, AsyncTiffResult};
use async_tiff::metadata::MetadataFetch;
use bytes::Bytes;
use futures::future::BoxFuture;

/// 바이트 range 를 비동기로 공급하는 소스. 로컬 파일·object store·인메모리 등
/// 어떤 저장소든 이 trait 하나로 engine 에 연결된다.
pub trait ByteSource: Debug + Send + Sync + 'static {
    /// `range` 의 바이트를 정확히 그 길이만큼 반환한다. 범위 밖이면 에러.
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
            if range.end > len || range.start > range.end {
                return Err(SourceError(format!(
                    "range {}..{} out of bounds (len {})",
                    range.start, range.end, len
                )));
            }
            Ok(self.0.slice(range.start as usize..range.end as usize))
        })
    }
}

/// [`ByteSource`] → async-tiff `MetadataFetch` 어댑터 (engine 내부 전용).
#[derive(Debug)]
pub(crate) struct FetchAdapter<S: ByteSource>(pub(crate) S);

#[async_trait::async_trait]
impl<S: ByteSource> MetadataFetch for FetchAdapter<S> {
    async fn fetch(&self, range: Range<u64>) -> AsyncTiffResult<Bytes> {
        self.0
            .fetch(range)
            .await
            .map_err(|e| AsyncTiffError::External(Box::new(e)))
    }
}
