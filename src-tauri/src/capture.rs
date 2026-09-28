use std::time::Duration;

use base64::Engine;
use tauri::{AppHandle, Manager, WebviewWindow};

// `Instant` is only needed by the Windows clipboard-polling path.
#[cfg(windows)]
use std::time::Instant;

/// Sentinel returned by the blocking capture task when the user never produced
/// an image (snipping overlay dismissed / timed out).
#[cfg(windows)]
const CANCELLED: &str = "__ocr_screenshot_cancelled__";

/// Minimize the app window, capture the screen (region on Windows via the
/// native snipping overlay, full-screen elsewhere), restore the window, and
/// return the result as a `data:image/png;base64,...` URL.
#[tauri::command]
pub async fn capture_screenshot(app: AppHandle) -> Result<String, String> {
    let window = app
        .get_webview_window("main")
        .or_else(|| app.webview_windows().into_values().next())
        .ok_or_else(|| "No application window found".to_string())?;

    capture_impl(window).await
}

fn encode_png_base64(png: Vec<u8>) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    format!("data:image/png;base64,{}", b64)
}

// ---------------------------------------------------------------------------
// Windows: native snipping tool (`ms-screenclip:`) + clipboard image
// ---------------------------------------------------------------------------

#[cfg(windows)]
async fn capture_impl(window: WebviewWindow) -> Result<String, String> {
    // Minimize our window so the snip overlay captures the desktop, not the app.
    window.minimize().map_err(|e| e.to_string())?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|e| format!("Clipboard unavailable: {}", e))?;
        // Drop any stale clipboard image so we only read the fresh snip.
        let _ = clipboard.clear();

        // Launch the built-in Windows snipping overlay.
        std::process::Command::new("explorer.exe")
            .arg("ms-screenclip:")
            .spawn()
            .map_err(|e| format!("Failed to launch snipping tool: {}", e))?;

        // Poll the clipboard until the user completes (or cancels) the snip.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match clipboard.get_image() {
                Ok(img) => {
                    let rgba = image::RgbaImage::from_raw(
                        img.width as u32,
                        img.height as u32,
                        img.bytes.into_owned(),
                    )
                    .ok_or_else(|| "Invalid clipboard image dimensions".to_string())?;

                    let mut png: Vec<u8> = Vec::new();
                    {
                        let mut cursor = std::io::Cursor::new(&mut png);
                        image::DynamicImage::ImageRgba8(rgba)
                            .write_to(&mut cursor, image::ImageFormat::Png)
                            .map_err(|e| format!("PNG encode failed: {}", e))?;
                    }
                    return Ok(png);
                }
                Err(_) => {
                    // No image yet (overlay still open) or a transient clipboard
                    // lock — keep polling until the deadline.
                    if Instant::now() >= deadline {
                        return Err(CANCELLED.to_string());
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    })
    .await
    .map_err(|e| format!("Capture task failed: {}", e))?;

    // Always restore the window, whether the capture succeeded or not.
    let _ = window.unminimize();
    let _ = window.set_focus();

    match result {
        Ok(png) => Ok(encode_png_base64(png)),
        Err(e) if e == CANCELLED => Err("Screenshot cancelled".to_string()),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// macOS / Linux: full-screen capture via xcap (region crop happens in-app)
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
async fn capture_impl(window: WebviewWindow) -> Result<String, String> {
    window.minimize().map_err(|e| e.to_string())?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        use xcap::Monitor;

        let monitors = Monitor::all().map_err(|e| format!("Failed to enumerate monitors: {}", e))?;
        let monitor = monitors
            .iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .or_else(|| monitors.first())
            .ok_or_else(|| "No monitor found".to_string())?;

        let image = monitor
            .capture_image()
            .map_err(|e| format!("Screen capture failed: {}", e))?;

        let mut png: Vec<u8> = Vec::new();
        {
            let mut cursor = std::io::Cursor::new(&mut png);
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut cursor, image::ImageFormat::Png)
                .map_err(|e| format!("PNG encode failed: {}", e))?;
        }
        Ok(png)
    })
    .await
    .map_err(|e| format!("Capture task failed: {}", e))?;

    let _ = window.unminimize();
    let _ = window.set_focus();

    result.map(encode_png_base64)
}
