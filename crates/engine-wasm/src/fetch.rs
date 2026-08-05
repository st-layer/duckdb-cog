//! 브라우저 fetch(HTTP Range) 기반 [`engine::ByteSource`] (#66 슬라이스 2).
//!
//! Send 경계: ByteSource/async-tiff 는 Send future 를 요구하지만 브라우저의
//! `JsFuture` 는 !Send — JS fetch 는 `spawn_local` 로 로컬 실행기에 던지고,
//! 호출자에게는 oneshot 수신(Send) future 만 돌려준다. engine 은 무수정.
//! wasm 은 단일 스레드라 실행 의미론도 안전하다.

use std::ops::Range;

use engine::bytes::Bytes;
use engine::futures::channel::oneshot;
use engine::futures::future::BoxFuture;
use engine::{ByteSource, SourceError};
use js_sys::Uint8Array;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, Response};

/// URL 하나를 읽는 소스 — String 만 보유해 `Send + Sync + 'static` 충족.
#[derive(Debug, Clone)]
pub(crate) struct FetchSource {
    url: String,
}

impl FetchSource {
    pub(crate) fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

impl ByteSource for FetchSource {
    fn fetch(&self, range: Range<u64>) -> BoxFuture<'_, Result<Bytes, SourceError>> {
        let (tx, rx) = oneshot::channel();
        let url = self.url.clone();
        wasm_bindgen_futures::spawn_local(async move {
            // 수신측이 먼저 drop 되면(호출 취소) 결과는 버려도 된다
            let _ = tx.send(fetch_range(&url, range).await);
        });
        Box::pin(async move {
            rx.await
                .unwrap_or_else(|_| Err(SourceError("fetch task dropped".into())))
        })
    }
}

fn js_msg(e: JsValue) -> String {
    js_sys::Error::from(e).message().into()
}

/// range 하나를 JS fetch 로 읽는다 — !Send 구간은 전부 이 함수 안.
/// 응답 처리는 ByteSource 의 EOF 클램프 계약(engine source.rs)을 따른다.
async fn fetch_range(url: &str, range: Range<u64>) -> Result<Bytes, SourceError> {
    if range.start > range.end {
        return Err(SourceError(format!(
            "invalid range {}..{} for '{url}'",
            range.start, range.end
        )));
    }
    let request = Request::new_with_str(url)
        .map_err(|e| SourceError(format!("bad url '{url}': {}", js_msg(e))))?;
    // HTTP Range 는 양끝 포함 — end-1. (Range 는 CORS-safelisted 가 아니라
    // 서버가 preflight 에서 허용해야 한다 — scripts/range_cors_server.py 참조.)
    request
        .headers()
        .set(
            "Range",
            &format!("bytes={}-{}", range.start, range.end.saturating_sub(1)),
        )
        .map_err(|e| SourceError(format!("Range header for '{url}': {}", js_msg(e))))?;
    let window = web_sys::window()
        .ok_or_else(|| SourceError("no window (worker 컨텍스트는 후속 슬라이스)".into()))?;
    let resp: Response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| SourceError(format!("fetch failed for '{url}': {}", js_msg(e))))?
        .dyn_into()
        .map_err(|_| SourceError(format!("non-Response fetch result for '{url}'")))?;

    match resp.status() {
        // 서버가 EOF 에서 클램프한 부분 응답 — 계약과 일치, 그대로 사용
        206 => body_bytes(&resp, url).await,
        // Range 를 무시하는 서버: 전체 body 에서 로컬 클램프로 계약 유지
        200 => {
            let all = body_bytes(&resp, url).await?;
            let len = all.len() as u64;
            if range.start >= len {
                return Err(SourceError(format!(
                    "range start {} >= len {len} for '{url}'",
                    range.start
                )));
            }
            Ok(all.slice(range.start as usize..range.end.min(len) as usize))
        }
        // range.start >= EOF (계약상 에러)
        416 => Err(SourceError(format!(
            "range {}..{} out of bounds for '{url}' (416)",
            range.start, range.end
        ))),
        s => Err(SourceError(format!("HTTP {s} for '{url}'"))),
    }
}

async fn body_bytes(resp: &Response, url: &str) -> Result<Bytes, SourceError> {
    let promise = resp
        .array_buffer()
        .map_err(|e| SourceError(format!("array_buffer for '{url}': {}", js_msg(e))))?;
    let buf = JsFuture::from(promise)
        .await
        .map_err(|e| SourceError(format!("body read for '{url}': {}", js_msg(e))))?;
    Ok(Bytes::from(Uint8Array::new(&buf).to_vec()))
}
