//! Unified model download module.
//!
//! Combines the clean architecture of `model-hub` (multi-platform, auth, pagination,
//! path safety, bounded concurrency) with fms-app's desktop features (cancellation,
//! progress reporting, stall detection, mirror fallback, SHA256 verification).

// Allow dead_code: ModelDownloader API is infrastructure for future HF/ModelScope downloads
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tar::Archive;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::model::{DownloadProgress, FileDownloadInfo};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// No data for this long means the transfer is wedged, not slow.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Bound on connection setup for HTTP downloads.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Default number of concurrent file downloads.
const DEFAULT_CONCURRENCY: usize = 4;

/// Default maximum retry attempts per file.
const DEFAULT_MAX_RETRIES: u32 = 3;

// ---------------------------------------------------------------------------
// Provider abstraction (from model-hub)
// ---------------------------------------------------------------------------

/// Download platform and authentication.
#[derive(Debug, Clone)]
pub enum HubProvider {
    /// Hugging Face, optional token for gated models.
    HuggingFace { token: Option<String> },
    /// HuggingFace mirror (hf-mirror.com), useful in regions with HF access issues.
    HfMirror { token: Option<String> },
    /// ModelScope, optional access token.
    ModelScope { token: Option<String> },
}

impl HubProvider {
    fn token(&self) -> Option<&str> {
        match self {
            Self::HuggingFace { token }
            | Self::HfMirror { token }
            | Self::ModelScope { token } => token.as_deref(),
        }
    }

    fn default_revision(&self) -> &'static str {
        match self {
            Self::HuggingFace { .. } | Self::HfMirror { .. } => "main",
            Self::ModelScope { .. } => "master",
        }
    }

    fn base_url(&self) -> &'static str {
        match self {
            Self::HuggingFace { .. } => "https://huggingface.co",
            Self::HfMirror { .. } => "https://hf-mirror.com",
            Self::ModelScope { .. } => "https://modelscope.cn",
        }
    }
}

// ---------------------------------------------------------------------------
// Internal data structures (from model-hub)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct HfFile {
    path: String,
    size: u64,
    r#type: String,
}

#[derive(Debug, serde::Deserialize)]
struct MsResponse {
    #[serde(rename = "Success")]
    success: bool,
    #[serde(rename = "Data")]
    data: Option<MsData>,
}

#[derive(Debug, serde::Deserialize)]
struct MsData {
    #[serde(rename = "Files")]
    files: Vec<MsFile>,
}

#[derive(Debug, serde::Deserialize)]
struct MsFile {
    #[serde(rename = "Path")]
    path: String,
    #[serde(rename = "Size")]
    size: u64,
    #[serde(rename = "Type")]
    r#type: String,
}

/// Platform-agnostic file descriptor.
#[derive(Clone)]
struct UnifiedFile {
    path: String,
    size: u64,
    download_url: String,
}

// ---------------------------------------------------------------------------
// Public API: ModelDownloader
// ---------------------------------------------------------------------------

/// Options for a single download operation.
pub struct DownloadOptions {
    /// Repository ID (e.g., "meta-llama/Llama-2-7b-hf").
    pub repo_id: String,
    /// Branch, tag, or commit hash. None uses platform default.
    pub revision: Option<String>,
    /// Local root directory; `<owner>--<model>` subdirectory is created automatically.
    pub save_dir: PathBuf,
    /// Optional whitelist of relative paths to download.
    pub files: Option<Vec<String>>,
}

/// Reusable model downloader with configurable concurrency and retry.
pub struct ModelDownloader {
    client: reqwest::Client,
    provider: HubProvider,
    concurrency: usize,
    max_retries: u32,
}

impl ModelDownloader {
    /// Create a downloader for the specified platform.
    pub fn new(provider: HubProvider) -> Result<Self, String> {
        let client = Self::build_client(&provider)?;
        Ok(Self {
            client,
            provider,
            concurrency: DEFAULT_CONCURRENCY,
            max_retries: DEFAULT_MAX_RETRIES,
        })
    }

    /// Set maximum concurrent downloads (default 4, min 1).
    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.max(1);
        self
    }

    /// Set maximum retry attempts per file (default 3).
    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// Execute the download with progress reporting and cancellation support.
    pub async fn download(
        &self,
        options: DownloadOptions,
        app: Option<&AppHandle>,
        progress: Option<&Arc<Mutex<DownloadProgress>>>,
        cancel_flag: Option<&Arc<AtomicBool>>,
    ) -> Result<(), String> {
        Self::validate_options(&options)?;

        let revision = options
            .revision
            .as_deref()
            .unwrap_or_else(|| self.provider.default_revision());

        let model_dir = options
            .repo_id
            .split('/')
            .fold(options.save_dir.clone(), |p, c| p.join(c));
        tokio::fs::create_dir_all(&model_dir)
            .await
            .map_err(|e| format!("Failed to create directory: {}", e))?;

        // Get file list from the platform
        let files = match &self.provider {
            HubProvider::HuggingFace { .. } | HubProvider::HfMirror { .. } => {
                self.get_hf_files(&options.repo_id, revision).await?
            }
            HubProvider::ModelScope { .. } => {
                self.get_ms_files(&options.repo_id, revision).await?
            }
        };

        // Apply file filter
        let filter: Option<HashSet<String>> = options.files.map(|v| v.into_iter().collect());

        // Initialize progress tracking
        if let Some(prog) = progress {
            let mut p = prog.lock().unwrap();
            p.files = files
                .iter()
                .filter(|f| filter.as_ref().map(|s| s.contains(&f.path)).unwrap_or(true))
                .map(|f| FileDownloadInfo {
                    file: f.path.clone(),
                    bytes_downloaded: 0,
                    total_bytes: Some(f.size).filter(|s| *s > 0),
                    speed: 0,
                    eta_seconds: None,
                })
                .collect();
            p.overall_total_bytes = p.files.iter().filter_map(|f| f.total_bytes).sum();
            p.overall_bytes_downloaded = 0;
        }

        // Bounded concurrent downloads
        let sem = Arc::new(Semaphore::new(self.concurrency));
        let mut join_set: JoinSet<Result<(), String>> = JoinSet::new();

        for (file_idx, file) in files.iter().enumerate() {
            if let Some(ref set) = filter {
                if !set.contains(&file.path) {
                    continue;
                }
            }

            // Path traversal protection
            let dest = safe_join(&model_dir, &file.path)?;

            // Skip already-downloaded files
            if let Ok(meta) = tokio::fs::metadata(&dest).await {
                if meta.len() == file.size && file.size > 0 {
                    if let Some(prog) = progress {
                        let mut p = prog.lock().unwrap();
                        if let Some(f) = p.files.get_mut(file_idx) {
                            f.bytes_downloaded = file.size;
                        }
                    }
                    continue;
                }
            }

            let client = self.client.clone();
            let sem = Arc::clone(&sem);
            let max_retries = self.max_retries;
            let cancel_flag = cancel_flag.cloned();
            let progress = progress.cloned();
            let app = app.cloned();
            // Clone file data into the closure (files is dropped after loop)
            let file_url = file.download_url.clone();
            let file_size = file.size;

            join_set.spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore closed");

                let result = with_retry(max_retries, || {
                    let c = client.clone();
                    let u = file_url.clone();
                    let d = dest.clone();
                    let cf = cancel_flag.clone();
                    let prog = progress.clone();
                    let app = app.clone();
                    async move {
                        download_single_file_with_progress(
                            &c, &u, &d, cf.as_ref(), prog.as_ref(), app.as_ref(), file_size,
                        )
                        .await
                    }
                })
                .await;

                result
            });
        }

        // Collect results; abort on first failure
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    join_set.abort_all();
                    return Err(e);
                }
                Err(e) if e.is_cancelled() => {}
                Err(e) => {
                    join_set.abort_all();
                    return Err(format!("Download task panicked: {}", e));
                }
            }
        }

        // Final progress update
        if let Some(prog) = progress {
            let mut p = prog.lock().unwrap();
            p.overall_bytes_downloaded = p.files.iter().map(|f| f.bytes_downloaded).sum();
            p.overall_total_bytes = p.files.iter().filter_map(|f| f.total_bytes).sum();
        }

        Ok(())
    }

    // ── Private methods ──────────────────────────────────────────────────────

    fn build_client(provider: &HubProvider) -> Result<reqwest::Client, String> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            concat!("fms-app/", env!("CARGO_PKG_VERSION")).parse().map_err(|e| format!("Invalid UA: {}", e))?,
        );

        if let Some(token) = provider.token() {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", token).parse().map_err(|e| format!("Invalid auth: {}", e))?,
            );
        }

        reqwest::Client::builder()
            .default_headers(headers)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))
    }

    fn validate_options(options: &DownloadOptions) -> Result<(), String> {
        if options.repo_id.is_empty() {
            return Err("repo_id cannot be empty".to_string());
        }
        if options.repo_id.contains("..") {
            return Err("repo_id contains illegal '..'".to_string());
        }
        if let Some(ref files) = options.files {
            for path in files {
                if path.contains("..") || path.starts_with('/') || path.starts_with('\\') {
                    return Err(format!("Invalid path in files list: {:?}", path));
                }
            }
        }
        Ok(())
    }

    /// Fetch HuggingFace file list with pagination support.
    async fn get_hf_files(&self, repo_id: &str, revision: &str) -> Result<Vec<UnifiedFile>, String> {
        // Support HF_ENDPOINT env var for mirrors
        let base_url = std::env::var("HF_ENDPOINT")
            .unwrap_or_else(|_| self.provider.base_url().to_string());

        let mut all_files = Vec::new();
        let mut next_url: Option<String> = Some(format!(
            "{}/api/models/{}/tree/{}?recursive=1",
            base_url, repo_id, revision
        ));

        while let Some(url) = next_url.take() {
            let resp = self.client.get(&url).send().await.map_err(|e| format!("HF API request failed: {}", e))?;

            if !resp.status().is_success() {
                return Err(format!("HuggingFace API error (HTTP {}): {}", resp.status(), url));
            }

            // Parse Link header for pagination
            next_url = resp
                .headers()
                .get(reqwest::header::LINK)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_link_next);

            let page: Vec<HfFile> = resp.json().await.map_err(|e| format!("Failed to parse HF response: {}", e))?;
            all_files.extend(page.into_iter().filter(|f| f.r#type == "file").map(|f| UnifiedFile {
                download_url: format!("{}/{}/resolve/{}/{}", base_url, repo_id, revision, f.path),
                path: f.path,
                size: f.size,
            }));
        }

        Ok(all_files)
    }

    /// Fetch ModelScope file list.
    async fn get_ms_files(&self, repo_id: &str, revision: &str) -> Result<Vec<UnifiedFile>, String> {
        let url = format!(
            "https://modelscope.cn/api/v1/models/{}/repo/files?Recursive=true&Revision={}",
            repo_id, revision
        );

        let resp = self.client.get(&url).send().await.map_err(|e| format!("ModelScope API request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("ModelScope API error (HTTP {})", resp.status()));
        }

        let parsed: MsResponse = resp.json().await.map_err(|e| format!("Failed to parse ModelScope response: {}", e))?;

        if !parsed.success {
            return Err("ModelScope API returned failure status".to_string());
        }

        let files = parsed.data.ok_or("No file data from ModelScope")?.files;

        Ok(files
            .into_iter()
            .filter(|f| f.r#type == "blob")
            .map(|f| UnifiedFile {
                download_url: format!(
                    "https://modelscope.cn/models/{}/resolve/{}/{}",
                    repo_id, revision, f.path
                ),
                path: f.path,
                size: f.size,
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Safe path join: prevents path traversal attacks.
fn safe_join(base: &Path, file_path: &str) -> Result<PathBuf, String> {
    let clean: PathBuf = file_path
        .split('/')
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .collect();

    let dest = base.join(&clean);

    if !dest.starts_with(base) {
        return Err(format!("Path traversal detected: {:?}", file_path));
    }
    Ok(dest)
}

/// Parse HTTP Link header for pagination.
fn parse_link_next(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let mut seg = part.trim().splitn(2, ';');
        let url_part = seg.next()?.trim();
        let rel_part = seg.next()?.trim();
        if rel_part == r#"rel="next""# {
            Some(url_part.trim_start_matches('<').trim_end_matches('>').to_string())
        } else {
            None
        }
    })
}

/// Retry wrapper with exponential backoff.
async fn with_retry<F, Fut, T>(max_retries: u32, mut f: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    for attempt in 0..max_retries {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if e.contains("cancelled") {
                    return Err(e);
                }
                let secs = 2u64.saturating_pow(attempt).min(60);
                tokio::time::sleep(Duration::from_secs(secs)).await;
            }
        }
    }
    f().await.map_err(|e| format!("Failed after {} retries: {}", max_retries, e))
}

/// Download a single file with progress, stall detection, and cancellation.
async fn download_single_file_with_progress(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    cancel_flag: Option<&Arc<AtomicBool>>,
    progress: Option<&Arc<Mutex<DownloadProgress>>>,
    app: Option<&AppHandle>,
    expected_size: u64,
) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    // Use .partial suffix for incomplete downloads
    let partial_path = {
        let mut p = dest.as_os_str().to_os_string();
        p.push(".partial");
        PathBuf::from(p)
    };

    let mut resume_from = tokio::fs::metadata(&partial_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    // Skip if already complete
    if resume_from == expected_size && expected_size > 0 {
        tokio::fs::rename(&partial_path, dest)
            .await
            .map_err(|e| format!("Failed to finalize: {}", e))?;
        return Ok(());
    }

    let file_start = Instant::now();
    let mut last_speed_update = Instant::now();
    let mut last_bytes: u64 = 0;
    let mut prev_speed: f64 = 0.0;

    let mut total_bytes: Option<u64> = None;

    for attempt in 0..=3u32 {
        // Check cancellation
        if let Some(cf) = cancel_flag {
            if cf.load(Ordering::Relaxed) {
                return Err("Download cancelled".to_string());
            }
        }

        if attempt > 0 {
            let delay = Duration::from_secs(2u64.pow(attempt));
            // Sleep with cancellation check
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = async {
                    if let Some(cf) = cancel_flag {
                        loop {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            if cf.load(Ordering::Relaxed) { break; }
                        }
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    return Err("Download cancelled".to_string());
                }
            }
            resume_from = tokio::fs::metadata(&partial_path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
        }

        let mut req = client.get(url);
        if resume_from > 0 {
            req = req.header("Range", format!("bytes={}-", resume_from));
        }

        let response = tokio::select! {
            r = tokio::time::timeout(STALL_TIMEOUT, req.send()) => {
                match r {
                    Ok(Ok(resp)) => resp,
                    Ok(Err(e)) => {
                        if attempt == 3 {
                            return Err(format!("Download failed after retries: {}", e));
                        }
                        continue;
                    }
                    Err(_) => {
                        if attempt == 3 {
                            return Err(format!("No response within {}s", STALL_TIMEOUT.as_secs()));
                        }
                        continue;
                    }
                }
            }
            _ = async {
                if let Some(cf) = cancel_flag {
                    loop {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if cf.load(Ordering::Relaxed) { break; }
                    }
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                return Err("Download cancelled".to_string());
            }
        };

        let status = response.status();

        // Server ignored Range header — restart
        if resume_from > 0 && status == reqwest::StatusCode::OK {
            let _ = tokio::fs::remove_file(&partial_path).await;
            resume_from = 0;
        }

        // Validate Content-Range offset on 206
        if resume_from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
            let starts_at = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    let range = v.trim().strip_prefix("bytes")?.trim_start();
                    range.split('-').next()?.trim().parse::<u64>().ok()
                });
            if starts_at != Some(resume_from) {
                let _ = tokio::fs::remove_file(&partial_path).await;
                resume_from = 0;
                if attempt == 3 {
                    return Err(format!("Content-Range mismatch: server started at {:?}, expected {}", starts_at, resume_from));
                }
                continue;
            }
        }

        if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
            if attempt == 3 {
                return Err(format!("Download failed: HTTP {}", status));
            }
            resume_from = 0;
            continue;
        }

        // Learn total size
        let chunk_total: Option<u64> = response.content_length().map(|cl| {
            if status == reqwest::StatusCode::PARTIAL_CONTENT {
                cl + resume_from
            } else {
                cl
            }
        });
        if total_bytes.is_none() {
            total_bytes = chunk_total;
        }

        let known_total = total_bytes.or(chunk_total);

        // Open file: append on resume, create on fresh start
        let file_handle = if resume_from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
            tokio::fs::OpenOptions::new().append(true).open(&partial_path).await
        } else {
            resume_from = 0;
            tokio::fs::File::create(&partial_path).await
        };
        let mut file_handle = file_handle.map_err(|e| format!("Failed to open file: {}", e))?;

        let mut stream = response.bytes_stream();
        let mut bytes_downloaded = resume_from;
        let mut download_ok = true;

        loop {
            let chunk = tokio::select! {
                c = tokio::time::timeout(STALL_TIMEOUT, stream.next()) => {
                    match c {
                        Ok(None) => break,
                        Ok(Some(Ok(chunk))) => chunk,
                        Ok(Some(Err(_))) => {
                            if attempt < 3 {
                                let _ = file_handle.flush().await;
                            }
                            download_ok = false;
                            break;
                        }
                        Err(_) => {
                            let _ = file_handle.flush().await;
                            if attempt < 3 {
                                download_ok = false;
                                break;
                            }
                            return Err(format!("Transfer stalled: no data for {}s", STALL_TIMEOUT.as_secs()));
                        }
                    }
                }
                _ = async {
                    if let Some(cf) = cancel_flag {
                        loop {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            if cf.load(Ordering::Relaxed) { break; }
                        }
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    let _ = file_handle.flush().await;
                    return Err("Download cancelled".to_string());
                }
            };

            // Oversize protection
            if let Some(cap) = known_total {
                if cap > 0 && bytes_downloaded + chunk.len() as u64 > cap {
                    drop(file_handle);
                    let _ = tokio::fs::remove_file(&partial_path).await;
                    return Err(format!("Server sent more than expected {} bytes", cap));
                }
            }

            if let Err(_) = file_handle.write_all(&chunk).await {
                if attempt < 3 {
                    let _ = file_handle.flush().await;
                }
                download_ok = false;
                break;
            }
            bytes_downloaded += chunk.len() as u64;

            // Throttled progress: max 10 events/sec
            let now = Instant::now();
            if now.duration_since(last_speed_update) >= Duration::from_millis(100) {
                let elapsed = now.duration_since(last_speed_update).as_secs_f64();
                let bytes_in_interval = bytes_downloaded - last_bytes;
                let instant_speed = bytes_in_interval as f64 / elapsed;

                let elapsed_total = file_start.elapsed().as_secs_f64();
                let alpha = if elapsed_total < 3.0 { 0.3 } else { 0.1 };
                let smooth = if prev_speed == 0.0 {
                    instant_speed
                } else {
                    alpha * instant_speed + (1.0 - alpha) * prev_speed
                };
                prev_speed = smooth;

                let file_eta = total_bytes.and_then(|tb| {
                    if smooth > 0.0 {
                        Some(((tb.saturating_sub(bytes_downloaded)) as f64 / smooth) as u64)
                    } else {
                        None
                    }
                });

                // Update progress
                if let Some(prog) = progress {
                    let mut p = prog.lock().unwrap();
                    // Find the file entry by matching path
                    if let Some(f) = p.files.iter_mut().find(|f| dest.ends_with(&f.file)) {
                        f.bytes_downloaded = bytes_downloaded;
                        if smooth > 0.0 {
                            f.speed = smooth as u64;
                        }
                        f.eta_seconds = file_eta;
                        if let Some(tb) = total_bytes {
                            f.total_bytes = Some(tb);
                        }
                    }
                    p.overall_bytes_downloaded = p.files.iter().map(|f| f.bytes_downloaded).sum();
                    p.overall_total_bytes = p.files.iter().filter_map(|f| f.total_bytes).sum();
                    p.speed = p.files.iter().map(|f| f.speed).sum();
                    p.eta_seconds = p.files.iter().filter_map(|f| f.eta_seconds).max();

                    if let Some(app) = app {
                        let snapshot = p.clone();
                        let _ = app.emit("model-download-progress", snapshot);
                    }
                }

                last_speed_update = now;
                last_bytes = bytes_downloaded;
            }
        }

        let _ = file_handle.flush().await;
        drop(file_handle);

        if download_ok {
            break;
        }
    }

    // Post-download integrity check
    let actual_size = tokio::fs::metadata(&partial_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    if let Some(expected) = total_bytes {
        if actual_size != expected {
            let _ = tokio::fs::remove_file(&partial_path).await;
            return Err(format!("Incomplete download: got {} bytes, expected {}", actual_size, expected));
        }
    } else if actual_size == 0 {
        let _ = tokio::fs::remove_file(&partial_path).await;
        return Err("Downloaded file is empty".to_string());
    }

    // Atomic rename: .partial → final
    tokio::fs::rename(&partial_path, dest)
        .await
        .map_err(|e| format!("Failed to finalize: {}", e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Blob URL download (for non-HF model sources)
// ---------------------------------------------------------------------------

/// Download a model from a single URL (e.g., blob.handy.computer) with retry/resume.
/// After download, extract tar.gz if is_directory, otherwise rename to final path.
/// Verifies SHA256 if provided.
pub async fn download_blob_model(
    app: &AppHandle,
    download_progress: &Arc<Mutex<DownloadProgress>>,
    model_dir: &Path,
    blob_url: &str,
    expected_sha256: Option<&str>,
    is_directory: bool,
    cancel_flag: &Arc<AtomicBool>,
    model_id: &str,
) -> Result<(), String> {
    log::info!("download_blob_model: id={}, url={}, is_dir={}", model_id, blob_url, is_directory);

    let partial_path = model_dir.join(format!("{}.partial", model_id));

    // Initialize progress. The file name must match the destination's trailing
    // path components (`download_single_file_with_progress` locates the entry
    // via `dest.ends_with(&f.file)`), hence the `.partial` suffix here.
    // Reuse any total already known (set by model_download_inner from the
    // catalog size); a single-file blob download starts at 0 bytes.
    {
        let mut p = download_progress.lock().unwrap();
        let known_total = p
            .files
            .first()
            .and_then(|f| f.total_bytes)
            .filter(|t| *t > 0);
        p.files = vec![FileDownloadInfo {
            file: format!("{}.partial", model_id),
            bytes_downloaded: 0,
            total_bytes: known_total,
            speed: 0,
            eta_seconds: None,
        }];
        p.overall_bytes_downloaded = 0;
        p.overall_total_bytes = known_total.unwrap_or(0);
    }

    // Download using the unified single-file function
    download_single_file_with_progress(
        &ModelDownloader::new(HubProvider::HuggingFace { token: None })?.client,
        blob_url,
        &partial_path,
        Some(cancel_flag),
        Some(download_progress),
        Some(app),
        0, // Unknown size for blob URLs
    )
    .await?;

    // Verify SHA256 if provided
    if let Some(expected_hash) = expected_sha256 {
        log::info!("Verifying SHA256 for model '{}'", model_id);
        verify_sha256(&partial_path, expected_hash)?;
    }

    // Post-download processing
    if is_directory {
        log::info!("Extracting tar.gz archive for model '{}'", model_id);
        extract_tar_gz(&partial_path, model_dir, model_id)?;
        let _ = std::fs::remove_file(&partial_path);
    } else {
        let final_path = model_dir.join(model_id);
        std::fs::rename(&partial_path, &final_path)
            .map_err(|e| format!("Failed to finalize model: {}", e))?;
    }

    Ok(())
}

/// Verify SHA256 hash of a file.
fn verify_sha256(file_path: &Path, expected_hash: &str) -> Result<(), String> {
    use std::io::Read;
    log::debug!("Verifying SHA256 of: {}", file_path.display());

    let mut file =
        std::fs::File::open(file_path).map_err(|e| format!("Failed to open file for SHA256: {}", e))?;

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read =
            file.read(&mut buffer).map_err(|e| format!("Failed to read file: {}", e))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let actual_hash = format!("{:x}", hasher.finalize());

    if actual_hash != expected_hash {
        log::error!("SHA256 mismatch: expected {}, got {}", expected_hash, actual_hash);
        let _ = std::fs::remove_file(file_path);
        return Err(format!("SHA256 mismatch: expected {}, got {}", expected_hash, actual_hash));
    }

    log::debug!("SHA256 verified successfully");
    Ok(())
}

/// Extract a tar.gz archive to a model directory.
fn extract_tar_gz(archive_path: &Path, model_dir: &Path, _model_id: &str) -> Result<(), String> {
    use std::fs;

    let temp_extract_dir = model_dir.join(".extracting");

    // Clean up any previous incomplete extraction
    if temp_extract_dir.exists() {
        let _ = fs::remove_dir_all(&temp_extract_dir);
    }

    fs::create_dir_all(&temp_extract_dir)
        .map_err(|e| format!("Failed to create temp extraction directory: {}", e))?;

    let tar_gz = fs::File::open(archive_path).map_err(|e| format!("Failed to open archive: {}", e))?;
    let tar = GzDecoder::new(tar_gz);
    let mut archive = Archive::new(tar);

    archive.unpack(&temp_extract_dir).map_err(|e| {
        let error_msg = format!("Failed to extract archive: {}", e);
        let _ = fs::remove_dir_all(&temp_extract_dir);
        let _ = fs::remove_file(archive_path);
        error_msg
    })?;

    // Move extracted contents into model_dir
    let extracted_entries: Vec<_> = fs::read_dir(&temp_extract_dir)
        .map_err(|e| format!("Failed to read temp extraction directory: {}", e))?
        .filter_map(|entry| entry.ok())
        .collect();

    let has_single_dir = extracted_entries.len() == 1
        && extracted_entries[0]
            .file_type()
            .map(|ft| ft.is_dir())
            .unwrap_or(false);

    if has_single_dir {
        let inner_dir = extracted_entries[0].path();
        for entry in fs::read_dir(&inner_dir)
            .map_err(|e| format!("Failed to read extracted directory: {}", e))?
        {
            let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
            let dest = model_dir.join(entry.file_name());
            fs::rename(entry.path(), &dest)
                .map_err(|e| format!("Failed to move extracted file: {}", e))?;
        }
    } else {
        for entry in fs::read_dir(&temp_extract_dir)
            .map_err(|e| format!("Failed to read temp extraction directory: {}", e))?
        {
            let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
            let dest = model_dir.join(entry.file_name());
            fs::rename(entry.path(), &dest)
                .map_err(|e| format!("Failed to move extracted file: {}", e))?;
        }
    }

    let _ = fs::remove_dir_all(&temp_extract_dir);
    Ok(())
}
