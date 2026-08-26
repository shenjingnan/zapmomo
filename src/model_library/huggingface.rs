//! Hugging Face Catalog Provider（**HF 专属数据层**；UI/Domain 不依赖本模块）。
//!
//! - `HfApiClient`：低层客户端（URL 构建 / ureq transport / TTL 缓存 / JSON Mapper / 错误映射）。
//! - `HuggingFaceCatalogProvider`：实现 [`super::catalog::CatalogProvider`]。
//!
//! 原则：
//! - 模型元数据始终来自 `{base}/api/models`；文件下载 URL 由 `download.rs` 单独解析（镜像可切换）。
//! - token 只进 `Authorization` header，绝不进 log / error message / cache key / URL。
//! - 列表只请求 Summary 字段；detail 不含完整文件树；files 懒加载。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::catalog::{
    CatalogError, CatalogPage, CatalogProvider, CatalogProviderId, CatalogQuery, CatalogSort,
    ModelCategory, RemoteModelDetail, RemoteModelFile, RemoteModelSummary,
};

/// 目录缓存默认 TTL（秒）。
const LIST_TTL_SECS: u64 = 300;
const DETAIL_TTL_SECS: u64 = 1800;
const MAX_CACHE_ENTRIES: usize = 64;

// ---------------------------------------------------------------------------
// Transport（测试可注入）
// ---------------------------------------------------------------------------

/// 一次 GET 的响应。
#[derive(Debug, Clone)]
pub struct HfResponse {
    pub status: u16,
    pub body: String,
    /// `X-Error-Message` 头（HF 用于说明 gated/权限原因）。
    pub error_message: Option<String>,
}

/// HTTP 传输抽象（真实实现用 ureq；测试用 fixture）。
pub trait HfTransport: Send + Sync {
    fn get(&self, url: &str, token: Option<&str>) -> Result<HfResponse, CatalogError>;
}

/// ureq 实现（阻塞；调用方应在 `spawn_blocking` 中执行）。
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_send_body(Some(Duration::from_secs(60)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(Duration::from_secs(60)))
            // 4xx/5xx 不转 Err：保持 ureq 2.x 行为，状态码统一由 map_status 映射
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HfTransport for UreqTransport {
    fn get(&self, url: &str, token: Option<&str>) -> Result<HfResponse, CatalogError> {
        let mut req = self.agent.get(url);
        if let Some(t) = token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        // http_status_as_error(false)：任何状态码都返回 Ok，错误仅来自传输层
        let mut resp = req
            .call()
            .map_err(|e| CatalogError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let error_message = resp
            .headers()
            .get("X-Error-Message")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        Ok(HfResponse {
            status,
            body,
            error_message,
        })
    }
}

// ---------------------------------------------------------------------------
// TTL 缓存（内存）
// ---------------------------------------------------------------------------

struct TtlCache<V> {
    map: Mutex<HashMap<String, (Instant, V)>>,
    ttl: Duration,
    max_entries: usize,
}

impl<V> TtlCache<V> {
    fn new(ttl: Duration) -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            ttl,
            max_entries: MAX_CACHE_ENTRIES,
        }
    }

    fn get(&self, key: &str) -> Option<V>
    where
        V: Clone,
    {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if let Some((at, v)) = map.get(key) {
            if now.duration_since(*at) < self.ttl {
                return Some(v.clone());
            }
            map.remove(key);
        }
        None
    }

    fn put(&self, key: String, value: V) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= self.max_entries && !map.contains_key(&key) {
            // FIFO：淘汰最早插入的一条（保证小缓存有界）
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(key, (Instant::now(), value));
    }

    fn clear(&self) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.clear();
    }
}

// ---------------------------------------------------------------------------
// URL 编码（不引入新依赖）
// ---------------------------------------------------------------------------

/// 编码非 unreserved 字符（用于 query 参数值）。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 编码 path 段（保留 `/` 作为分隔符）。
fn encode_path_segment(s: &str) -> String {
    s.split('/').map(urlencode).collect::<Vec<_>>().join("/")
}

// ---------------------------------------------------------------------------
// HF 客户端
// ---------------------------------------------------------------------------

/// Hugging Face 目录客户端。
pub struct HfApiClient {
    base_url: String,
    token: Option<String>,
    transport: Box<dyn HfTransport>,
    list_cache: TtlCache<CatalogPage<RemoteModelSummary>>,
    detail_cache: TtlCache<RemoteModelDetail>,
    files_cache: TtlCache<Vec<RemoteModelFile>>,
}

impl HfApiClient {
    pub fn new(base_url: String, token: Option<String>, transport: Box<dyn HfTransport>) -> Self {
        Self {
            base_url,
            token,
            transport,
            list_cache: TtlCache::new(Duration::from_secs(LIST_TTL_SECS)),
            detail_cache: TtlCache::new(Duration::from_secs(DETAIL_TTL_SECS)),
            files_cache: TtlCache::new(Duration::from_secs(DETAIL_TTL_SECS)),
        }
    }

    /// 用配置构建真实客户端。
    pub fn from_settings(settings: &crate::config::settings::ModelLibrarySettings) -> Self {
        Self::new(
            settings.hf_catalog_base_url.clone(),
            settings.hf_token.clone(),
            Box::new(UreqTransport::new()),
        )
    }

    /// token（**内部使用**：只进 Authorization header，绝不进入任何日志/错误/缓存 key）。
    fn bearer(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn get_json(&self, url: &str) -> Result<Value, CatalogError> {
        let resp = self.transport.get(url, self.bearer())?;
        map_status(resp)
    }

    /// 分页搜索（带缓存）。
    pub fn search(
        &self,
        query: &CatalogQuery,
    ) -> Result<CatalogPage<RemoteModelSummary>, CatalogError> {
        let cache_key = format!("search:{}", query_cache_key(query));
        if let Some(page) = self.list_cache.get(&cache_key) {
            return Ok(page);
        }
        let url = self.build_search_url(query)?;
        let json = self.get_json(&url)?;
        let page = parse_search_page(&json, query.page_size);
        self.list_cache.put(cache_key, page.clone());
        Ok(page)
    }

    /// 模型详情（不带完整文件树）。
    pub fn model_detail(
        &self,
        repo_id: &str,
        revision: Option<&str>,
    ) -> Result<RemoteModelDetail, CatalogError> {
        let cache_key = detail_cache_key(repo_id, revision);
        if let Some(d) = self.detail_cache.get(&cache_key) {
            return Ok(d);
        }
        let url = self.build_detail_url(repo_id, revision);
        let json = self.get_json(&url)?;
        let detail = parse_model_detail(&json, repo_id);
        self.detail_cache.put(cache_key, detail.clone());
        Ok(detail)
    }

    /// 文件树（懒加载，带缓存）。
    pub fn model_files(
        &self,
        repo_id: &str,
        revision: Option<&str>,
    ) -> Result<Vec<RemoteModelFile>, CatalogError> {
        let cache_key = detail_cache_key(repo_id, revision);
        if let Some(f) = self.files_cache.get(&cache_key) {
            return Ok(f);
        }
        let url = self.build_tree_url(repo_id, revision);
        let json = self.get_json(&url)?;
        let files = parse_model_files(&json);
        self.files_cache.put(cache_key, files.clone());
        Ok(files)
    }

    /// README（懒加载，不缓存或短生命周期由调用方决定）。
    pub fn model_readme(
        &self,
        repo_id: &str,
        revision: Option<&str>,
    ) -> Result<Option<String>, CatalogError> {
        let rev = revision.unwrap_or("main");
        let url = format!(
            "{}/{}/raw/{}/README.md",
            self.base_url,
            encode_path_segment(repo_id),
            encode_path_segment(rev)
        );
        let resp = self.transport.get(&url, self.bearer())?;
        match resp.status {
            200 => Ok(Some(resp.body)),
            404 => Ok(None),
            _ => Err(map_status(resp).unwrap_err()),
        }
    }

    /// 刷新时清空缓存。
    pub fn invalidate(&self) {
        self.list_cache.clear();
        self.detail_cache.clear();
        self.files_cache.clear();
    }

    // ---- URL 构建 ----

    fn build_search_url(&self, query: &CatalogQuery) -> Result<String, CatalogError> {
        let mut params: Vec<(String, String)> = Vec::new();
        let page_size = query.page_size.clamp(1, 100);
        let skip = query.page.saturating_mul(page_size);
        params.push(("limit".into(), page_size.to_string()));
        params.push(("skip".into(), skip.to_string()));
        if let Some(s) = query.search.as_deref().filter(|s| !s.trim().is_empty()) {
            params.push(("search".into(), s.trim().to_string()));
        }
        // 排序：Recommended → 下载量（verified boost 由 merge 层叠加）
        let sort = match query.sort {
            CatalogSort::Downloads | CatalogSort::Recommended => "downloads",
            CatalogSort::Likes => "likes",
            CatalogSort::LastModified => "lastModified",
            CatalogSort::Trending => "trending",
        };
        params.push(("sort".into(), sort.to_string()));
        params.push(("direction".into(), "-1".into()));
        // 分类 pipeline filter
        if let Some(p) = query.category.and_then(category_pipeline_filter) {
            params.push(("filter".into(), p.to_string()));
        }
        // LLM：源头过滤掉非 GGUF 仓库（纯 transformers 无法在当前 runtime 运行）。
        // 避免列表塞满 Unsupported，导致前端过滤后"滚动加载但无可显示模型"的闪烁。
        if query.category == Some(ModelCategory::Llm) {
            params.push(("filter".into(), "gguf".into()));
        }
        if let Some(lang) = query.language.as_deref().filter(|l| !l.trim().is_empty()) {
            params.push((
                "filter".into(),
                format!("language:{}", lang.trim().to_lowercase()),
            ));
        }
        if let Some(lic) = query.license.as_deref().filter(|l| !l.trim().is_empty()) {
            params.push((
                "filter".into(),
                format!("license:{}", lic.trim().to_lowercase()),
            ));
        }
        if let Some(p) = query.parameters {
            let (min, max) = p.min_max();
            let seg = format!(
                "{}{}",
                min.map(|m| format!("min:{m}")).unwrap_or_default(),
                if min.is_some() && max.is_some() {
                    ","
                } else {
                    ""
                },
            );
            let seg = if seg.is_empty() {
                format!("max:{}", max.unwrap_or_default())
            } else {
                format!(
                    "{seg}{}",
                    max.map(|m| format!("max:{m}")).unwrap_or_default()
                )
            };
            if !seg.is_empty() {
                params.push(("num_parameters".into(), seg));
            }
        }
        let qs = params
            .into_iter()
            .map(|(k, v)| format!("{}={}", urlencode(&k), urlencode(&v)))
            .collect::<Vec<_>>()
            .join("&");
        Ok(format!("{}/api/models?{}", self.base_url, qs))
    }

    fn build_detail_url(&self, repo_id: &str, revision: Option<&str>) -> String {
        let mut url = format!(
            "{}/api/models/{}",
            self.base_url,
            encode_path_segment(repo_id)
        );
        if let Some(rev) = revision.filter(|r| !r.is_empty()) {
            url.push_str(&format!("?revision={}", urlencode(rev)));
        }
        url
    }

    fn build_tree_url(&self, repo_id: &str, revision: Option<&str>) -> String {
        let rev = revision.unwrap_or("main");
        format!(
            "{}/api/models/{}/tree/{}?recursive=true&expand=true",
            self.base_url,
            encode_path_segment(repo_id),
            encode_path_segment(rev)
        )
    }
}

/// 分类 → pipeline filter（KWS 无标准 pipeline，用 tag 兜底；具体在 Phase 2 细化）。
fn category_pipeline_filter(cat: ModelCategory) -> Option<&'static str> {
    match cat {
        ModelCategory::Llm => Some("text-generation"),
        ModelCategory::Asr => Some("automatic-speech-recognition"),
        ModelCategory::Tts => Some("text-to-speech"),
        ModelCategory::Kws => Some("wake-word"),
    }
}

fn query_cache_key(query: &CatalogQuery) -> String {
    // 只含影响结果的字段（排除 page → 分页各自缓存）
    format!(
        "cat={:?}&q={}&lang={}&lic={}&param={:?}&sort={:?}&ps={}&unsup={}",
        query.category,
        query.search.as_deref().unwrap_or(""),
        query.language.as_deref().unwrap_or(""),
        query.license.as_deref().unwrap_or(""),
        query.parameters,
        query.sort,
        query.page_size,
        query.include_unsupported,
    )
}

fn detail_cache_key(repo_id: &str, revision: Option<&str>) -> String {
    format!("{repo_id}@{}", revision.unwrap_or("main"))
}

/// 把 HTTP 状态映射为领域错误（区分 gated / token / 不存在 / 限流）。
fn map_status(resp: HfResponse) -> Result<Value, CatalogError> {
    match resp.status {
        200 => {
            let preview: String = resp.body.chars().take(200).collect();
            serde_json::from_str(&resp.body)
                .map_err(|e| CatalogError::Parse(format!("{e}（body 前 200 字符：{preview}）")))
        }
        401 => Err(CatalogError::AuthRequired),
        403 => {
            let msg = resp.error_message.unwrap_or_default().to_lowercase();
            if msg.contains("gated") || msg.contains("cannot access") {
                Err(CatalogError::GatedRequiresAgreement)
            } else {
                Err(CatalogError::AuthRequired)
            }
        }
        404 => Err(CatalogError::NotFound),
        429 => Err(CatalogError::RateLimited),
        code => Err(CatalogError::Http {
            status: code,
            detail: resp.body.chars().take(300).collect(),
        }),
    }
}

// ---------------------------------------------------------------------------
// JSON Mapper
// ---------------------------------------------------------------------------

fn get_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn get_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn get_str_array(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 从 tags 中提取 `prefix:*` 值。
fn tags_with_prefix(tags: &[String], prefix: &str) -> Vec<String> {
    tags.iter()
        .filter_map(|t| t.strip_prefix(prefix).map(|rest| rest.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// 参数量：优先 num_parameters 字段，其次 tags 中的 params:/param_count:。
fn parse_parameter_count(v: &Value, tags: &[String]) -> Option<String> {
    if let Some(n) = v.get("num_parameters") {
        if let Some(f) = n.as_f64() {
            if f > 0.0 {
                return Some(format_parameters(f));
            }
        } else if let Some(s) = n.as_str().filter(|s| !s.is_empty()) {
            return Some(s.to_string());
        }
    }
    for prefix in ["params:", "param_count:", "parameter-count:"] {
        if let Some(s) = tags_with_prefix(tags, prefix).into_iter().next() {
            return Some(s);
        }
    }
    None
}

/// 把参数个数格式化为人类可读（4_000_000_000 → "4B"）。
fn format_parameters(count: f64) -> String {
    if count >= 1e9 {
        let b = count / 1e9;
        if (b - b.round()).abs() < 1e-6 {
            format!("{:.0}B", b)
        } else {
            format!("{b:.1}B")
        }
    } else if count >= 1e6 {
        format!("{:.1}M", count / 1e6)
    } else {
        format!("{count:.0}")
    }
}

fn repo_id_of(v: &Value) -> String {
    get_str(v, "id")
        .or_else(|| get_str(v, "modelId"))
        .or_else(|| get_str(v, "_id"))
        .unwrap_or_default()
        .to_string()
}

fn display_name_of(repo_id: &str) -> String {
    repo_id.split('/').nth(1).unwrap_or(repo_id).to_string()
}

fn author_of(v: &Value, repo_id: &str) -> String {
    get_str(v, "author")
        .map(str::to_string)
        .unwrap_or_else(|| repo_id.split('/').next().unwrap_or("").to_string())
}

fn gated_of(v: &Value) -> Option<String> {
    v.get("gated").and_then(|g| match g {
        Value::Bool(b) => Some(b.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    })
}

pub fn parse_model_summary(v: &Value) -> RemoteModelSummary {
    let repo_id = repo_id_of(v);
    let tags = get_str_array(v, "tags");
    let languages = tags_with_prefix(&tags, "language:");
    RemoteModelSummary {
        repo_id: repo_id.clone(),
        author: author_of(v, &repo_id),
        display_name: display_name_of(&repo_id),
        description: get_str(v, "description").map(str::to_string),
        pipeline_tag: get_str(v, "pipeline_tag").map(str::to_string),
        library_name: get_str(v, "library_name").map(str::to_string),
        tags,
        downloads: get_u64(v, "downloads"),
        likes: get_u64(v, "likes"),
        trending_score: v.get("trendingScore").and_then(Value::as_f64),
        last_modified: get_str(v, "lastModified").map(str::to_string),
        created_at: get_str(v, "createdAt").map(str::to_string),
        license: tags_with_prefix(&get_str_array(v, "tags"), "license:")
            .into_iter()
            .next(),
        languages,
        parameter_count: parse_parameter_count(v, &get_str_array(v, "tags")),
        gated: gated_of(v),
        private: v.get("private").and_then(Value::as_bool),
        sha: get_str(v, "sha").map(str::to_string),
    }
}

pub fn parse_model_detail(v: &Value, fallback_repo_id: &str) -> RemoteModelDetail {
    let repo_id = repo_id_of(v);
    let repo_id = if repo_id.is_empty() {
        fallback_repo_id.to_string()
    } else {
        repo_id
    };
    let tags = get_str_array(v, "tags");
    let siblings = v
        .get("siblings")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| get_str(s, "rfilename").map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    RemoteModelDetail {
        repo_id,
        description: get_str(v, "description").map(str::to_string),
        pipeline_tag: get_str(v, "pipeline_tag").map(str::to_string),
        library_name: get_str(v, "library_name").map(str::to_string),
        tags: tags.clone(),
        license: tags_with_prefix(&tags, "license:").into_iter().next(),
        languages: tags_with_prefix(&tags, "language:"),
        downloads: get_u64(v, "downloads"),
        likes: get_u64(v, "likes"),
        last_modified: get_str(v, "lastModified").map(str::to_string),
        created_at: get_str(v, "createdAt").map(str::to_string),
        sha: get_str(v, "sha").map(str::to_string),
        gated: gated_of(v),
        private: v.get("private").and_then(Value::as_bool),
        card_data: v.get("cardData").cloned(),
        siblings,
    }
}

pub fn parse_model_files(v: &Value) -> Vec<RemoteModelFile> {
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| {
            let path = get_str(e, "path")?.to_string();
            let file_type = get_str(e, "type").unwrap_or("file").to_string();
            if file_type == "directory" {
                return Some(RemoteModelFile {
                    path,
                    size: None,
                    file_type,
                    lfs: None,
                    sha256: None,
                });
            }
            let lfs = e.get("lfs").and_then(|l| {
                let sha256 = get_str(l, "sha256")?.to_string();
                let size = l.get("size").and_then(Value::as_u64)?;
                Some(super::catalog::FileLfs { sha256, size })
            });
            let size = e.get("size").and_then(Value::as_u64);
            let sha256 = get_str(e, "sha256").map(str::to_string);
            Some(RemoteModelFile {
                path,
                size,
                file_type,
                lfs,
                sha256,
            })
        })
        .collect()
}

/// 解析分页响应：HF 返回数组；has_more = 返回数量达到页大小。
fn parse_search_page(v: &Value, page_size: u32) -> CatalogPage<RemoteModelSummary> {
    let items = v
        .as_array()
        .map(|a| a.iter().map(parse_model_summary).collect::<Vec<_>>())
        .unwrap_or_default();
    let has_more = items.len() as u32 >= page_size.clamp(1, 100);
    CatalogPage { items, has_more }
}

// ---------------------------------------------------------------------------
// Provider 实现
// ---------------------------------------------------------------------------

/// Hugging Face 目录 Provider。
pub struct HuggingFaceCatalogProvider {
    client: HfApiClient,
}

impl HuggingFaceCatalogProvider {
    pub fn new(client: HfApiClient) -> Self {
        Self { client }
    }

    pub fn invalidate(&self) {
        self.client.invalidate();
    }
}

impl CatalogProvider for HuggingFaceCatalogProvider {
    fn provider_id(&self) -> CatalogProviderId {
        CatalogProviderId::HuggingFace
    }

    fn search(
        &self,
        query: &CatalogQuery,
    ) -> Result<CatalogPage<RemoteModelSummary>, CatalogError> {
        self.client.search(query)
    }

    fn model_detail(
        &self,
        repo_id: &str,
        revision: Option<&str>,
    ) -> Result<RemoteModelDetail, CatalogError> {
        self.client.model_detail(repo_id, revision)
    }

    fn model_files(
        &self,
        repo_id: &str,
        revision: Option<&str>,
    ) -> Result<Vec<RemoteModelFile>, CatalogError> {
        self.client.model_files(repo_id, revision)
    }

    fn model_readme(
        &self,
        repo_id: &str,
        revision: Option<&str>,
    ) -> Result<Option<String>, CatalogError> {
        self.client.model_readme(repo_id, revision)
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_library::catalog::{ModelCategory, ParameterRange};

    /// 内存 fixture transport：按 URL 关键字返回预设响应。
    struct FakeTransport {
        search_body: Option<String>,
        detail_body: Option<String>,
        files_body: Option<String>,
        readme_body: Option<String>,
        status: u16,
        error_header: Option<String>,
    }

    impl FakeTransport {
        fn ok(search: &str) -> Self {
            Self {
                search_body: Some(search.to_string()),
                detail_body: Some(search.to_string()),
                files_body: Some("[]".to_string()),
                readme_body: Some("# hello".to_string()),
                status: 200,
                error_header: None,
            }
        }
    }

    impl HfTransport for FakeTransport {
        fn get(&self, url: &str, _token: Option<&str>) -> Result<HfResponse, CatalogError> {
            if url.contains("/api/models/") && url.contains("/tree/") {
                return Ok(HfResponse {
                    status: self.status,
                    body: self.files_body.clone().unwrap_or_default(),
                    error_message: self.error_header.clone(),
                });
            }
            if url.contains("/raw/") {
                return Ok(HfResponse {
                    status: if self.readme_body.is_some() { 200 } else { 404 },
                    body: self.readme_body.clone().unwrap_or_default(),
                    error_message: None,
                });
            }
            if url.contains("/api/models/") {
                return Ok(HfResponse {
                    status: self.status,
                    body: self.detail_body.clone().unwrap_or_default(),
                    error_message: self.error_header.clone(),
                });
            }
            Ok(HfResponse {
                status: self.status,
                body: self.search_body.clone().unwrap_or_default(),
                error_message: self.error_header.clone(),
            })
        }
    }

    fn sample_search_json() -> Value {
        serde_json::json!([
            {
                "id": "Qwen/Qwen3-4B-GGUF",
                "author": "Qwen",
                "sha": "abc123",
                "downloads": 812345,
                "likes": 1234,
                "trendingScore": 42.5,
                "pipeline_tag": "text-generation",
                "library_name": "gguf",
                "tags": ["license:apache-2.0", "language:zh", "language:en", "gguf", "qwen3"],
                "lastModified": "2025-05-20T00:00:00Z",
                "createdAt": "2025-01-01T00:00:00Z",
                "private": false,
                "gated": false
            }
        ])
    }

    fn client_with(body: Value, status: u16) -> HfApiClient {
        HfApiClient::new(
            "https://huggingface.co".into(),
            None,
            Box::new(FakeTransport {
                search_body: Some(body.to_string()),
                detail_body: Some(body.to_string()),
                files_body: Some(
                    serde_json::json!([
                        {"type":"file","path":"Qwen3-4B-Q4_K_M.gguf","size":2497281120u64,
                         "lfs":{"sha256":"deadbeef","size":2497281120u64}}
                    ])
                    .to_string(),
                ),
                readme_body: Some("# qwen".into()),
                status,
                error_header: None,
            }),
        )
    }

    #[test]
    fn test_search_maps_summary() {
        let client = client_with(sample_search_json(), 200);
        let page = client.search(&CatalogQuery::default()).unwrap();
        assert_eq!(page.items.len(), 1);
        let m = &page.items[0];
        assert_eq!(m.repo_id, "Qwen/Qwen3-4B-GGUF");
        assert_eq!(m.author, "Qwen");
        assert_eq!(m.display_name, "Qwen3-4B-GGUF");
        assert_eq!(m.downloads, 812345);
        assert_eq!(m.license.as_deref(), Some("apache-2.0"));
        assert_eq!(m.languages, vec!["zh".to_string(), "en".to_string()]);
        assert_eq!(m.pipeline_tag.as_deref(), Some("text-generation"));
    }

    #[test]
    fn test_search_page_has_more() {
        let client = client_with(sample_search_json(), 200);
        let page = client.search(&CatalogQuery::default()).unwrap();
        // 返回 1 < page_size(20) → 无更多
        assert!(!page.has_more);
    }

    #[test]
    fn test_search_url_contains_params() {
        let client = HfApiClient::new(
            "https://huggingface.co".into(),
            None,
            Box::new(FakeTransport::ok("[]")),
        );
        let q = CatalogQuery {
            category: Some(ModelCategory::Llm),
            search: Some("Qwen 3".into()),
            language: Some("zh".into()),
            license: Some("apache-2.0".into()),
            parameters: Some(ParameterRange::B3to7),
            sort: CatalogSort::Downloads,
            page: 2,
            page_size: 20,
            include_unsupported: false,
        };
        let url = client.build_search_url(&q).unwrap();
        assert!(url.starts_with("https://huggingface.co/api/models?"));
        assert!(url.contains("search=Qwen%203"), "url={url}");
        assert!(url.contains("filter=text-generation"));
        assert!(url.contains("filter=gguf"), "LLM 应源头过滤非 GGUF 仓库");
        assert!(url.contains("filter=language%3Azh"));
        assert!(url.contains("filter=license%3Aapache-2.0"));
        assert!(url.contains("num_parameters=min%3A3B%2Cmax%3A7B"));
        assert!(url.contains("sort=downloads"));
        assert!(url.contains("skip=40"));
        assert!(url.contains("limit=20"));
    }

    #[test]
    fn test_model_files_parses_lfs() {
        let client = client_with(sample_search_json(), 200);
        let files = client.model_files("Qwen/Qwen3-4B-GGUF", None).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "Qwen3-4B-Q4_K_M.gguf");
        assert_eq!(files[0].size, Some(2_497_281_120));
        assert_eq!(
            files[0].lfs.as_ref().map(|l| l.sha256.as_str()),
            Some("deadbeef")
        );
    }

    #[test]
    fn test_error_mapping() {
        let cases: &[(u16, Option<&str>, CatalogError)] = &[
            (401, None, CatalogError::AuthRequired),
            (404, None, CatalogError::NotFound),
            (429, None, CatalogError::RateLimited),
            (
                403,
                Some("Cannot access gated repo"),
                CatalogError::GatedRequiresAgreement,
            ),
            (403, Some("Invalid token"), CatalogError::AuthRequired),
        ];
        for (status, header, expected) in cases {
            let client = HfApiClient::new(
                "https://huggingface.co".into(),
                None,
                Box::new(FakeTransport {
                    search_body: None,
                    detail_body: None,
                    files_body: None,
                    readme_body: None,
                    status: *status,
                    error_header: header.map(str::to_string),
                }),
            );
            let err = client.search(&CatalogQuery::default()).unwrap_err();
            assert!(
                std::mem::discriminant(&err) == std::mem::discriminant(expected),
                "{status} → {err}"
            );
        }
    }

    #[test]
    fn test_cache_ttl_hits() {
        let client = client_with(sample_search_json(), 200);
        let a = client.search(&CatalogQuery::default()).unwrap();
        let b = client.search(&CatalogQuery::default()).unwrap();
        assert_eq!(a.items.len(), b.items.len());
    }

    #[test]
    fn test_encode_path_segment_keeps_slash() {
        assert_eq!(
            encode_path_segment("Qwen/Qwen3-4B-GGUF"),
            "Qwen/Qwen3-4B-GGUF"
        );
        assert_eq!(encode_path_segment("a b/c"), "a%20b/c");
        assert_eq!(urlencode("Qwen 3"), "Qwen%203");
    }

    #[test]
    fn test_format_parameters() {
        assert_eq!(format_parameters(4_000_000_000.0), "4B");
        assert_eq!(format_parameters(1_500_000_000.0), "1.5B");
    }

    /// 真实网络冒烟：搜索 qwen 应得到真实 HF repo（不进 CI）。
    #[test]
    #[ignore = "真实网络请求，不进 CI"]
    fn real_api_search_qwen() {
        let client = HfApiClient::new(
            "https://huggingface.co".into(),
            None,
            Box::new(UreqTransport::new()),
        );
        let query = CatalogQuery {
            category: Some(crate::model_library::catalog::ModelCategory::Llm),
            search: Some("qwen".into()),
            page_size: 20,
            ..Default::default()
        };
        let page = client.search(&query).expect("search 失败");
        assert!(!page.items.is_empty(), "应返回真实 repo");
        assert!(
            page.items
                .iter()
                .any(|m| m.repo_id.to_lowercase().contains("qwen")),
            "应包含 qwen 相关 repo"
        );
        eprintln!(
            "找到 {} 个，首个：{}",
            page.items.len(),
            page.items[0].repo_id
        );
    }

    /// 真实网络冒烟：detail + files 懒加载（不进 CI）。
    #[test]
    #[ignore = "真实网络请求，不进 CI"]
    fn real_api_detail_and_files() {
        let client = HfApiClient::new(
            "https://huggingface.co".into(),
            None,
            Box::new(UreqTransport::new()),
        );
        let detail = client
            .model_detail("Qwen/Qwen3-4B-GGUF", None)
            .expect("detail 失败");
        assert!(!detail.repo_id.is_empty());
        let files = client
            .model_files("Qwen/Qwen3-4B-GGUF", None)
            .expect("files 失败");
        assert!(
            files.iter().any(|f| f.path.ends_with(".gguf")),
            "应包含 .gguf 文件"
        );
        eprintln!(
            "detail pipeline={:?} library={:?} files={}",
            detail.pipeline_tag,
            detail.library_name,
            files.len()
        );
    }
}
