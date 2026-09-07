use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tar::Archive;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

use crate::model::{DownloadProgress, FileDownloadInfo};

/// No data for this long means the transfer is wedged, not slow: the stream
/// errors out (keeping the partial for resume) and the retry loop takes over.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Bound on connection setup for HTTP downloads.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Public orchestration
// ---------------------------------------------------------------------------

/// Download model files **in parallel** with mirror fallback.
///
/// All files are spawned as concurrent tokio tasks so the connection is
/// fully saturated.  Tries the preferred source first for each file; if
/// all retries fail, resets the partial file and tries the alternate source.
#[allow(dead_code)]
pub async fn download_model_files(
    app: &AppHandle,
    download_progress: &Arc<Mutex<DownloadProgress>>,
    cancel_flags: &Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>,
    model_dir: &Path,
    repo: &str,
    files: &[&str],
    file_sizes: &[(String, u64)],
    preferred_base: &str,
    cancel_flag: &Arc<AtomicBool>,
    version: &str,
    _completed_bytes: u64,
    overall_total_bytes_init: u64,
) -> Result<(), String> {
    let alternate_base = if preferred_base == "https://huggingface.co" {
        "https://hf-mirror.com"
    } else {
        "https://huggingface.co"
    };

    // Initialize per-file progress entries (each parallel task updates its own).
    {
        let mut p = download_progress.lock().unwrap();
        p.files = files
            .iter()
            .map(|f| {
                let total = file_sizes
                    .iter()
                    .find(|(n, _)| n == f)
                    .map(|(_, s)| *s)
                    .filter(|s| *s > 0);
                FileDownloadInfo {
                    file: f.to_string(),
                    bytes_downloaded: 0,
                    total_bytes: total,
                    speed: 0,
                    eta_seconds: None,
                }
            })
            .collect();
        p.overall_total_bytes = overall_total_bytes_init;
        p.overall_bytes_downloaded = 0;
    }

    // Check for already-completed files and skip them.
    for (file_idx, file) in files.iter().enumerate() {
        let final_path = model_dir.join(file);
        let partial_path = model_dir.join(format!("{}.partial", file));
        let expected_size = file_sizes
            .iter()
            .find(|(f, _)| f == file)
            .map(|(_, s)| *s);

        if let Some(expected) = expected_size {
            if expected > 0 {
                // .partial already at expected size? Just rename.
                if let Ok(meta) = std::fs::metadata(&partial_path) {
                    if meta.len() == expected {
                        std::fs::rename(&partial_path, &final_path).map_err(|e| {
                            format!("Failed to finalize {}: {}", file, e)
                        })?;
                        let mut p = download_progress.lock().unwrap();
                        if let Some(f) = p.files.get_mut(file_idx) {
                            f.bytes_downloaded = expected;
                        }
                        continue;
                    }
                    if meta.len() > expected {
                        let _ = std::fs::remove_file(&partial_path);
                    }
                }
                // Final file already exists — mark complete.
                if final_path.exists() {
                    if let Ok(meta) = std::fs::metadata(&final_path) {
                        if meta.len() == expected {
                            let mut p = download_progress.lock().unwrap();
                            if let Some(f) = p.files.get_mut(file_idx) {
                                f.bytes_downloaded = expected;
                            }
                            continue;
                        }
                    }
                }
            }
        }
    }

    // Spawn parallel download tasks — one per file that still needs downloading.
    let dp_arc = download_progress.clone();

    let mut handles: Vec<JoinHandle<Result<(), String>>> = Vec::new();

    for (file_idx, file) in files.iter().enumerate() {
        // Skip files already marked complete.
        {
            let p = download_progress.lock().unwrap();
            if let Some(f) = p.files.get(file_idx) {
                if f.total_bytes.is_some() && f.bytes_downloaded >= f.total_bytes.unwrap() {
                    continue;
                }
            }
        }

        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        let final_path = model_dir.join(file);
        let partial_path = model_dir.join(format!("{}.partial", file));
        let cancel_flag = cancel_flag.clone();
        let app = app.clone();
        let repo = repo.to_string();
        let file = file.to_string();
        let preferred_base = preferred_base.to_string();
        let alternate_base = alternate_base.to_string();

        let dp_arc = Arc::clone(&dp_arc);

        let handle: JoinHandle<Result<(), String>> = tokio::spawn(async move {
            // Try preferred source first, then alternate on failure.
            let mut download_ok = false;
            let mut last_error = String::new();

            for source_base in [&preferred_base, &alternate_base] {
                match download_single_file(
                    &client,
                    source_base,
                    &repo,
                    &file,
                    &partial_path,
                    &final_path,
                    &cancel_flag,
                    &dp_arc,
                    file_idx,
                    &app,
                )
                .await
                {
                    Ok(()) => {
                        download_ok = true;
                        break;
                    }
                    Err(e) => {
                        if e.contains("cancelled") {
                            return Err(e);
                        }
                        last_error = e;
                        let _ = std::fs::remove_file(&partial_path);
                    }
                }
            }

            if !download_ok {
                return Err(last_error);
            }
            Ok(())
        });

        handles.push(handle);
    }

    // Wait for all parallel downloads to finish.
    for handle in handles {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                cancel_flag.store(true, Ordering::Relaxed);
                // Clean up partial files.
                for f in files.iter() {
                    let partial = model_dir.join(format!("{}.partial", f));
                    let _ = std::fs::remove_file(partial);
                }
                cancel_flags.lock().unwrap().remove(version);
                return Err(e);
            }
            Err(join_err) => {
                cancel_flags.lock().unwrap().remove(version);
                return Err(format!("Download task panicked: {}", join_err));
            }
        }
    }

    // Final progress snapshot — compute overall from per-file states.
    {
        let mut p = download_progress.lock().unwrap();
        p.overall_bytes_downloaded = p.files.iter().map(|f| f.bytes_downloaded).sum();
        p.overall_total_bytes = p
            .files
            .iter()
            .map(|f| f.total_bytes.unwrap_or(0))
            .sum();
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Single-file download with retry / resume / stall detection
// ---------------------------------------------------------------------------

/// Download a single file with retry, resume, stall timeout, and cancellation.
/// Returns Ok(()) on success, Err(message) on failure.
#[allow(dead_code)]
async fn download_single_file(
    client: &reqwest::Client,
    base_url: &str,
    repo: &str,
    file: &str,
    partial_path: &Path,
    final_path: &Path,
    cancel_flag: &Arc<AtomicBool>,
    download_progress: &Mutex<DownloadProgress>,
    file_idx: usize,
    app: &AppHandle,
) -> Result<(), String> {
    let url = format!("{}/{}/resolve/main/{}", base_url, repo, file);

    const MAX_RETRIES: u32 = 5;
    let mut total_bytes: Option<u64> = None;
    let mut resume_from = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
    let file_start = Instant::now();
    let mut last_speed_update = Instant::now();
    let mut last_bytes: u64 = 0;
    let mut prev_speed: f64 = 0.0;

    for attempt in 0..=MAX_RETRIES {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("Download cancelled".to_string());
        }

        if attempt > 0 {
            let delay = Duration::from_secs(2u64.pow(attempt));
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if cancel_flag.load(Ordering::Relaxed) { break; }
                    }
                } => {
                    return Err("Download cancelled".to_string());
                }
            }
            resume_from = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
        }

        let mut req = client.get(&url);
        if resume_from > 0 {
            req = req.header("Range", format!("bytes={}-", resume_from));
        }

        let response = tokio::select! {
            r = tokio::time::timeout(STALL_TIMEOUT, req.send()) => {
                match r {
                    Ok(Ok(resp)) => resp,
                    Ok(Err(e)) => {
                        if attempt == MAX_RETRIES {
                            return Err(format!(
                                "Failed to download {} after {} retries: {}",
                                file, MAX_RETRIES, e
                            ));
                        }
                        continue;
                    }
                    Err(_) => {
                        if attempt == MAX_RETRIES {
                            return Err(format!(
                                "No response within {}s for {}",
                                STALL_TIMEOUT.as_secs(),
                                file
                            ));
                        }
                        continue;
                    }
                }
            }
            _ = async {
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if cancel_flag.load(Ordering::Relaxed) { break; }
                }
            } => {
                return Err("Download cancelled".to_string());
            }
        };

        let status = response.status();

        // Server ignored Range header — restart
        if resume_from > 0 && status == reqwest::StatusCode::OK {
            let _ = std::fs::remove_file(partial_path);
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
                let _ = std::fs::remove_file(partial_path);
                resume_from = 0;
                if attempt == MAX_RETRIES {
                    return Err(format!(
                        "Content-Range mismatch for {}: server started at {:?}, expected {}",
                        file, starts_at, resume_from
                    ));
                }
                continue;
            }
        }

        if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
            if attempt == MAX_RETRIES {
                return Err(format!("Failed to download {}: HTTP {}", file, status));
            }
            resume_from = 0;
            continue;
        }

        // Learn total size from first successful response
        if total_bytes.is_none() {
            total_bytes = response.content_length().map(|cl| {
                if status == reqwest::StatusCode::PARTIAL_CONTENT {
                    cl + resume_from
                } else {
                    cl
                }
            });
            // Update this file's total_bytes in shared progress state.
            if let Some(file_total) = total_bytes {
                let mut p = download_progress.lock().unwrap();
                if let Some(f) = p.files.get_mut(file_idx) {
                    f.total_bytes = Some(file_total);
                }
                // Recompute overall_total_bytes from all files.
                p.overall_total_bytes = p.files.iter().map(|f| f.total_bytes.unwrap_or(0)).sum();
            }
        }

        let known_total =
            total_bytes.or_else(|| response.content_length().map(|l| resume_from + l));

        // Open file: append on resume, create on fresh start
        let file_handle =
            if resume_from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(partial_path)
                    .await
            } else {
                resume_from = 0;
                tokio::fs::File::create(partial_path).await
            };
        let mut file_handle =
            file_handle.map_err(|e| format!("Failed to open {}: {}", file, e))?;

        let mut stream = response.bytes_stream();
        let mut bytes_downloaded = resume_from;
        let mut download_ok = true;

        loop {
            let chunk = tokio::select! {
                c = tokio::time::timeout(STALL_TIMEOUT, stream.next()) => {
                    match c {
                        Ok(None) => break,
                        Ok(Some(Ok(chunk))) => chunk,
                        Ok(Some(Err(_e))) => {
                            if attempt < MAX_RETRIES {
                                let _ = file_handle.flush().await;
                            }
                            download_ok = false;
                            break;
                        }
                        Err(_) => {
                            let _ = file_handle.flush().await;
                            if attempt < MAX_RETRIES {
                                download_ok = false;
                                break;
                            }
                            return Err(format!(
                                "Transfer stalled: no data for {}s from {}",
                                STALL_TIMEOUT.as_secs(),
                                file
                            ));
                        }
                    }
                }
                _ = async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if cancel_flag.load(Ordering::Relaxed) { break; }
                    }
                } => {
                    let _ = file_handle.flush().await;
                    return Err("Download cancelled".to_string());
                }
            };

            // Oversize protection
            if let Some(cap) = known_total {
                if bytes_downloaded + chunk.len() as u64 > cap {
                    drop(file_handle);
                    let _ = std::fs::remove_file(partial_path);
                    return Err(format!(
                        "Server sent more than the expected {} bytes for {}",
                        cap, file
                    ));
                }
            }

            if let Err(_e) = file_handle.write_all(&chunk).await {
                if attempt < MAX_RETRIES {
                    let _ = file_handle.flush().await;
                }
                download_ok = false;
                break;
            }
            bytes_downloaded += chunk.len() as u64;

            // Throttled progress: max 10 events/sec (100ms)
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
                        Some(
                            ((tb.saturating_sub(bytes_downloaded)) as f64 / smooth)
                                as u64,
                        )
                    } else {
                        None
                    }
                });

                // Update shared progress state and emit.
                {
                    let mut p = download_progress.lock().unwrap();
                    if let Some(f) = p.files.get_mut(file_idx) {
                        f.bytes_downloaded = bytes_downloaded;
                        if smooth > 0.0 {
                            f.speed = smooth as u64;
                        }
                        f.eta_seconds = file_eta;
                        if total_bytes.is_some() {
                            f.total_bytes = total_bytes;
                        }
                    }
                    // Compute overall progress from all per-file states.
                    p.overall_bytes_downloaded =
                        p.files.iter().map(|f| f.bytes_downloaded).sum();
                    p.overall_total_bytes =
                        p.files.iter().map(|f| f.total_bytes.unwrap_or(0)).sum();
                    p.speed = p.files.iter().map(|f| f.speed).sum();
                    p.eta_seconds = p
                        .files
                        .iter()
                        .filter_map(|f| f.eta_seconds)
                        .max();
                    let snapshot = p.clone();
                    let _ = app.emit("model-download-progress", snapshot);
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
    let actual_size = partial_path.metadata().map(|m| m.len()).unwrap_or(0);

    if let Some(expected) = total_bytes {
        if actual_size != expected {
            let _ = std::fs::remove_file(partial_path);
            return Err(format!(
                "Incomplete download for {}: got {} bytes, expected {} bytes",
                file, actual_size, expected
            ));
        }
    } else if actual_size == 0 {
        let _ = std::fs::remove_file(partial_path);
        return Err(format!("Downloaded file {} is empty", file));
    }

    // Atomic rename: .partial → final
    std::fs::rename(partial_path, final_path)
        .map_err(|e| format!("Failed to finalize {}: {}", file, e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Blob model download (single URL from blob.handy.computer)
// ---------------------------------------------------------------------------

/// Download a model from blob.handy.computer (single URL) with retry/resume.
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
    let partial_path = model_dir.join(format!("{}.partial", model_id));
    
    // Download the file using the existing retry/resume logic
    download_single_url(
        app,
        download_progress,
        blob_url,
        &partial_path,
        cancel_flag,
        model_id,
    )
    .await?;

    // Verify SHA256 if provided
    if let Some(expected_hash) = expected_sha256 {
        verify_sha256(&partial_path, expected_hash)?;
    }

    // Post-download processing
    if is_directory {
        // Extract tar.gz archive
        extract_tar_gz(&partial_path, model_dir, model_id)?;
        // Remove the archive after successful extraction
        let _ = std::fs::remove_file(&partial_path);
    } else {
        // Single file model - rename .partial to final filename
        let final_path = model_dir.join(model_id);
        std::fs::rename(&partial_path, &final_path)
            .map_err(|e| format!("Failed to finalize model: {}", e))?;
    }

    Ok(())
}

/// Download a single file from a URL with retry/resume/stall detection.
async fn download_single_url(
    app: &AppHandle,
    download_progress: &Arc<Mutex<DownloadProgress>>,
    url: &str,
    partial_path: &Path,
    cancel_flag: &Arc<AtomicBool>,
    model_id: &str,
) -> Result<(), String> {
    const MAX_RETRIES: u32 = 5;
    let mut total_bytes: Option<u64> = None;
    let mut resume_from = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
    let file_start = Instant::now();
    let mut last_speed_update = Instant::now();
    let mut last_bytes: u64 = 0;
    let mut prev_speed: f64 = 0.0;

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    for attempt in 0..=MAX_RETRIES {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("Download cancelled".to_string());
        }

        if attempt > 0 {
            let delay = Duration::from_secs(2u64.pow(attempt));
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if cancel_flag.load(Ordering::Relaxed) { break; }
                    }
                } => {
                    return Err("Download cancelled".to_string());
                }
            }
            resume_from = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
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
                        if attempt == MAX_RETRIES {
                            return Err(format!(
                                "Failed to download {} after {} retries: {}",
                                model_id, MAX_RETRIES, e
                            ));
                        }
                        continue;
                    }
                    Err(_) => {
                        if attempt == MAX_RETRIES {
                            return Err(format!(
                                "No response within {}s for {}",
                                STALL_TIMEOUT.as_secs(),
                                model_id
                            ));
                        }
                        continue;
                    }
                }
            }
            _ = async {
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if cancel_flag.load(Ordering::Relaxed) { break; }
                }
            } => {
                return Err("Download cancelled".to_string());
            }
        };

        let status = response.status();

        // Server ignored Range header — restart
        if resume_from > 0 && status == reqwest::StatusCode::OK {
            let _ = std::fs::remove_file(partial_path);
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
                let _ = std::fs::remove_file(partial_path);
                resume_from = 0;
                if attempt == MAX_RETRIES {
                    return Err(format!(
                        "Content-Range mismatch for {}: server started at {:?}, expected {}",
                        model_id, starts_at, resume_from
                    ));
                }
                continue;
            }
        }

        if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
            if attempt == MAX_RETRIES {
                return Err(format!("Failed to download {}: HTTP {}", model_id, status));
            }
            resume_from = 0;
            continue;
        }

        // Learn total size from first successful response
        if total_bytes.is_none() {
            total_bytes = response.content_length().map(|cl| {
                if status == reqwest::StatusCode::PARTIAL_CONTENT {
                    cl + resume_from
                } else {
                    cl
                }
            });
            // Update progress with total size
            if let Some(file_total) = total_bytes {
                let mut p = download_progress.lock().unwrap();
                if let Some(f) = p.files.get_mut(0) {
                    f.total_bytes = Some(file_total);
                }
                p.overall_total_bytes = file_total;
            }
        }

        let known_total =
            total_bytes.or_else(|| response.content_length().map(|l| resume_from + l));

        // Open file: append on resume, create on fresh start
        let file_handle =
            if resume_from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(partial_path)
                    .await
            } else {
                resume_from = 0;
                tokio::fs::File::create(partial_path).await
            };
        let mut file_handle =
            file_handle.map_err(|e| format!("Failed to open {}: {}", model_id, e))?;

        let mut stream = response.bytes_stream();
        let mut bytes_downloaded = resume_from;
        let mut download_ok = true;

        loop {
            let chunk = tokio::select! {
                c = tokio::time::timeout(STALL_TIMEOUT, stream.next()) => {
                    match c {
                        Ok(None) => break,
                        Ok(Some(Ok(chunk))) => chunk,
                        Ok(Some(Err(_e))) => {
                            if attempt < MAX_RETRIES {
                                let _ = file_handle.flush().await;
                            }
                            download_ok = false;
                            break;
                        }
                        Err(_) => {
                            let _ = file_handle.flush().await;
                            if attempt < MAX_RETRIES {
                                download_ok = false;
                                break;
                            }
                            return Err(format!(
                                "Transfer stalled: no data for {}s from {}",
                                STALL_TIMEOUT.as_secs(),
                                model_id
                            ));
                        }
                    }
                }
                _ = async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if cancel_flag.load(Ordering::Relaxed) { break; }
                    }
                } => {
                    let _ = file_handle.flush().await;
                    return Err("Download cancelled".to_string());
                }
            };

            // Oversize protection
            if let Some(cap) = known_total {
                if bytes_downloaded + chunk.len() as u64 > cap {
                    drop(file_handle);
                    let _ = std::fs::remove_file(partial_path);
                    return Err(format!(
                        "Server sent more than the expected {} bytes for {}",
                        cap, model_id
                    ));
                }
            }

            if let Err(_e) = file_handle.write_all(&chunk).await {
                if attempt < MAX_RETRIES {
                    let _ = file_handle.flush().await;
                }
                download_ok = false;
                break;
            }
            bytes_downloaded += chunk.len() as u64;

            // Throttled progress: max 10 events/sec (100ms)
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
                        Some(
                            ((tb.saturating_sub(bytes_downloaded)) as f64 / smooth)
                                as u64,
                        )
                    } else {
                        None
                    }
                });

                // Update progress state and emit.
                {
                    let mut p = download_progress.lock().unwrap();
                    if let Some(f) = p.files.get_mut(0) {
                        f.bytes_downloaded = bytes_downloaded;
                        if smooth > 0.0 {
                            f.speed = smooth as u64;
                        }
                        f.eta_seconds = file_eta;
                        if total_bytes.is_some() {
                            f.total_bytes = total_bytes;
                        }
                    }
                    p.overall_bytes_downloaded = bytes_downloaded;
                    p.overall_total_bytes = total_bytes.unwrap_or(0);
                    p.speed = prev_speed as u64;
                    p.eta_seconds = file_eta;
                    let snapshot = p.clone();
                    let _ = app.emit("model-download-progress", snapshot);
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
    let actual_size = partial_path.metadata().map(|m| m.len()).unwrap_or(0);

    if let Some(expected) = total_bytes {
        if actual_size != expected {
            let _ = std::fs::remove_file(partial_path);
            return Err(format!(
                "Incomplete download for {}: got {} bytes, expected {} bytes",
                model_id, actual_size, expected
            ));
        }
    } else if actual_size == 0 {
        let _ = std::fs::remove_file(partial_path);
        return Err(format!("Downloaded file {} is empty", model_id));
    }

    Ok(())
}

/// Verify SHA256 hash of a file.
fn verify_sha256(file_path: &Path, expected_hash: &str) -> Result<(), String> {
    use std::io::Read;
    
    let mut file = std::fs::File::open(file_path)
        .map_err(|e| format!("Failed to open file for SHA256 verification: {}", e))?;
    
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    
    loop {
        let bytes_read = file.read(&mut buffer)
            .map_err(|e| format!("Failed to read file for SHA256 verification: {}", e))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    
    let actual_hash = format!("{:x}", hasher.finalize());
    
    if actual_hash != expected_hash {
        let _ = std::fs::remove_file(file_path);
        return Err(format!(
            "SHA256 mismatch: expected {}, got {}",
            expected_hash, actual_hash
        ));
    }
    
    Ok(())
}

/// Extract a tar.gz archive to a model directory.
/// Finds the single extracted directory and renames it to the final model directory.
fn extract_tar_gz(archive_path: &Path, model_dir: &Path, _model_id: &str) -> Result<(), String> {
    use std::fs;
    
    // model_dir is already the per-model directory (e.g. models/parakeet-v3/),
    // so we extract directly into it — no extra nesting.
    let temp_extract_dir = model_dir.join(".extracting");
    
    // Clean up any previous incomplete extraction
    if temp_extract_dir.exists() {
        let _ = fs::remove_dir_all(&temp_extract_dir);
    }
    
    // Create temporary extraction directory
    fs::create_dir_all(&temp_extract_dir)
        .map_err(|e| format!("Failed to create temp extraction directory: {}", e))?;
    
    // Open and extract the tar.gz file
    let tar_gz = fs::File::open(archive_path)
        .map_err(|e| format!("Failed to open archive: {}", e))?;
    let tar = GzDecoder::new(tar_gz);
    let mut archive = Archive::new(tar);
    
    archive.unpack(&temp_extract_dir).map_err(|e| {
        let error_msg = format!("Failed to extract archive: {}", e);
        let _ = fs::remove_dir_all(&temp_extract_dir);
        let _ = fs::remove_file(archive_path);
        error_msg
    })?;
    
    // Move extracted contents into model_dir (stripping any top-level directory
    // the archive may contain to avoid double-nesting).
    let extracted_entries: Vec<_> = fs::read_dir(&temp_extract_dir)
        .map_err(|e| format!("Failed to read temp extraction directory: {}", e))?
        .filter_map(|entry| entry.ok())
        .collect();
    
    let has_single_dir = extracted_entries.len() == 1
        && extracted_entries[0]
            .file_type()
            .map(|ft| ft.is_dir())
            .unwrap_or(false);
    
    // If archive had a single top-level dir, move its contents; otherwise move
    // everything from the temp dir directly.
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
    
    // Clean up temp directory
    let _ = fs::remove_dir_all(&temp_extract_dir);
    
    Ok(())
}
