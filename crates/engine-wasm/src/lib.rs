//! engine-wasm — 브라우저 사이드카 바인딩 (#66, RFC Decision B·§6.5).
//!
//! engine 위의 얇은 JS 경계 계층: 타입 변환만 하고 도메인 로직은 전부 engine 에
//! 둔다. 네이티브 타깃에서는 빈 크레이트 — 판정은 `just wasm-check`/`just wasm-test`.
//!
//! 불변식: ReaderCache 사용 금지 — `Instant::now()` 가 wasm32-unknown-unknown
//! 에서 런타임 panic (G8 의 cargo check 게이트로는 안 잡힌다, cache.rs 참조).

#[cfg(target_arch = "wasm32")]
mod bindings;
#[cfg(target_arch = "wasm32")]
mod fetch;
#[cfg(target_arch = "wasm32")]
pub use bindings::*;
