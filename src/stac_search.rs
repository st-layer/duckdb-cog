// read_stac_search — STAC API POST /search + rel=next 페이지네이션 (RFC §6.7, 이슈 #29).
//
// HTTP 는 이 파일(ext)에만 있다 — 페이지 파싱·body 조립·next 해석은 engine 의
// 순수 함수 (parse_stac_page / build_search_body / apply_next). native 전용
// 표면 (ByteSource 는 range-GET 전용이라 API 검색을 나를 수 없다 — worklog 참조).
// (lib.rs 가 include! 로 포함하므로 //! 내부 독 주석은 못 쓴다.)

use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};

use duckdb::core::{DataChunkHandle, LogicalTypeHandle, LogicalTypeId};
use duckdb::vtab::{BindInfo, InitInfo, TableFunctionInfo, VTab};
use duckdb::Connection;

use super::{add_stac_columns, tokio_runtime, write_stac_batch};

fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// 요청 1회 — 익스텐션 수명의 tokio 런타임에 스폰, 호출측은 JoinHandle 대기
/// (ObjectStoreSource 와 같은 실행 모델). POST 는 body(없으면 `{}`)를 JSON 으로.
fn fetch_page(
    url: &str,
    method: &str,
    body: Option<engine::serde_json::Value>,
) -> std::result::Result<Vec<u8>, String> {
    let client = http_client().clone();
    let url = url.to_string();
    let post = method.eq_ignore_ascii_case("POST");
    let task = tokio_runtime().spawn(async move {
        let req = if post {
            client
                .post(&url)
                .json(&body.unwrap_or_else(|| engine::serde_json::json!({})))
        } else {
            client.get(&url)
        };
        let resp = req
            .header("Accept", "application/geo+json")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            let head: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
            return Err(format!("HTTP {status}: {head}"));
        }
        Ok::<_, String>(bytes.to_vec())
    });
    engine::futures::executor::block_on(task).map_err(|e| e.to_string())?
}

#[repr(C)]
pub struct StacSearchBindData {
    rows: Vec<engine::StacAssetRow>,
}

#[repr(C)]
pub struct StacSearchInitData {
    cursor: AtomicUsize,
}

/// `SELECT * FROM read_stac_search(url, collections := [...], bbox := [...],
/// datetime := '...', limit := N, max_rows := N);` — 스키마는 read_stac 과 동일.
///
/// `url` 은 search 엔드포인트 전체 (예: https://earth-search.aws.element84.com/v1/search).
/// `limit` 은 서버 페이지 크기 힌트, `max_rows`(기본 1,000) 는 클라이언트 행 상한 —
/// 도달 시 다음 페이지를 요청하지 않고 초과분을 자른다.
pub struct ReadStacSearchVTab;

impl VTab for ReadStacSearchVTab {
    type InitData = StacSearchInitData;
    type BindData = StacSearchBindData;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn Error>> {
        add_stac_columns(bind);
        let url = bind.get_parameter(0).to_string();
        let named = |k: &str| bind.get_named_parameter(k).filter(|v| !v.is_null());
        let collections: Option<Vec<String>> = match named("collections") {
            None => None,
            Some(v) => Some(
                v.to_list()
                    .ok_or("read_stac_search: collections must be a list of strings")?
                    .iter()
                    .map(|x| x.to_string())
                    .collect(),
            ),
        };
        let bbox = match named("bbox") {
            None => None,
            Some(v) => {
                let items = v
                    .to_list()
                    .ok_or("read_stac_search: bbox must be a list of 4 doubles")?;
                if items.len() != 4 {
                    return Err(format!(
                        "read_stac_search: bbox must have exactly 4 elements [xmin, ymin, xmax, ymax], got {}",
                        items.len()
                    )
                    .into());
                }
                let f: Vec<f64> = items.iter().map(|x| x.to_double()).collect();
                Some([f[0], f[1], f[2], f[3]])
            }
        };
        let datetime = named("datetime").map(|v| v.to_string());
        let limit = named("limit")
            .map(|v| u32::try_from(v.to_int64()))
            .transpose()
            .map_err(|_| "read_stac_search: limit must be a positive integer")?;
        let max_rows = match named("max_rows") {
            None => 1000usize,
            Some(v) => usize::try_from(v.to_int64())
                .ok()
                .filter(|n| *n > 0)
                .ok_or("read_stac_search: max_rows must be a positive integer")?,
        };

        let original = engine::build_search_body(
            collections.as_deref(),
            bbox,
            datetime.as_deref(),
            limit,
        );
        let mut rows: Vec<engine::StacAssetRow> = Vec::new();
        let mut href = url;
        let mut method = "POST".to_string();
        let mut body = Some(original.clone());
        loop {
            let bytes = fetch_page(&href, &method, body)
                .map_err(|e| format!("read_stac_search: '{href}': {e}"))?;
            let page = engine::parse_stac_page(&bytes)
                .map_err(|e| format!("read_stac_search: '{href}': {e}"))?;
            let got = page.rows.len();
            rows.extend(page.rows);
            if rows.len() >= max_rows {
                rows.truncate(max_rows);
                break;
            }
            let Some(next) = page.next else { break };
            // 0행 페이지가 next 를 달고 있으면 진행해도 얻을 게 없다 — 루프 가드
            if got == 0 {
                break;
            }
            // merge 는 항상 원 검색 body 기준 (STAC API next-link 규약)
            let (h, m, b) = engine::apply_next(&original, &next);
            href = h;
            method = m;
            body = b;
        }
        Ok(StacSearchBindData { rows })
    }

    fn init(_: &InitInfo) -> Result<Self::InitData, Box<dyn Error>> {
        Ok(StacSearchInitData {
            cursor: AtomicUsize::new(0),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn Error>> {
        const CHUNK: usize = 2048;
        let rows = &func.get_bind_data().rows;
        let start = func
            .get_init_data()
            .cursor
            .fetch_add(CHUNK, Ordering::Relaxed);
        if start >= rows.len() {
            output.set_len(0);
            return Ok(());
        }
        write_stac_batch(&rows[start..(start + CHUNK).min(rows.len())], output);
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        let double = LogicalTypeHandle::from(LogicalTypeId::Double);
        Some(vec![
            (
                "collections".to_string(),
                LogicalTypeHandle::list(&LogicalTypeHandle::from(LogicalTypeId::Varchar)),
            ),
            ("bbox".to_string(), LogicalTypeHandle::list(&double)),
            (
                "datetime".to_string(),
                LogicalTypeHandle::from(LogicalTypeId::Varchar),
            ),
            (
                "limit".to_string(),
                LogicalTypeHandle::from(LogicalTypeId::Integer),
            ),
            (
                "max_rows".to_string(),
                LogicalTypeHandle::from(LogicalTypeId::Integer),
            ),
        ])
    }
}

pub(crate) fn register(con: &Connection) -> duckdb::Result<()> {
    con.register_table_function::<ReadStacSearchVTab>("read_stac_search")
}
