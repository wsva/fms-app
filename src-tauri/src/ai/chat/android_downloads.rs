//! Where a phone-side chat download finally lands: the public Downloads folder.
//!
//! Every other attachment path in this module runs the opposite way — a SAF
//! `content://` URI is *read* into app-private storage. Saving has no such
//! shortcut: the cache under `<workspace>/chat/cache` is internal storage that no
//! file manager, gallery, or share target can reach, so a "save" that stopped
//! there left the user holding a green check mark and no file they could find.
//!
//! Two routes, chosen by API level:
//!
//! * **Android 10 (API 29) and newer** — insert a row into the MediaStore
//!   `Downloads` collection under `Download/FmS/` and stream the bytes into the
//!   stream MediaStore opens. No permission is involved: an app may always write
//!   the media rows it created itself.
//! * **Older releases** — the manifest declares `WRITE_EXTERNAL_STORAGE` with
//!   `maxSdkVersion="28"`, so ask for it through `MainActivity` and then write
//!   `/sdcard/Download/FmS/<name>` directly.
//!
//! Both return the location in a form the UI can report, so the file is findable
//! afterwards instead of merely "saved" somewhere inside the app's sandbox.

use std::io::Read;
use std::path::{Path, PathBuf};

use jni::objects::{JObject, JString, JValue};
use jni::JNIEnv;

use super::android_content::cursor_display_name;
use super::android_jni::{
    android_context, call_int, call_object, call_void, sdk_int, swallow, uri_parse,
};

/// Sub-folder of the public Downloads directory that FmS writes into.
const FOLDER: &str = "FmS";

/// The public collection MediaStore exposes for `Download/` (`MediaStore.Downloads
/// .EXTERNAL_CONTENT_URI`), spelled out so no class lookup is needed.
const MEDIASTORE_DOWNLOADS: &str = "content://media/external/downloads";

const WRITE_PERMISSION: &str = "android.permission.WRITE_EXTERNAL_STORAGE";

/// Copies `src` into `Download/FmS/` on the public volume and returns where it
/// landed. `mime` is what the attachment claimed to be; when it is missing or
/// implausible the extension decides, because MediaStore files itself by type.
pub async fn save(src: &Path, name: &str, mime: Option<&str>) -> Result<String, String> {
    let mime = mime
        .filter(|m| m.contains('/'))
        .map(str::to_owned)
        .unwrap_or_else(|| super::guess_mime(name).to_string());

    if sdk_int()? >= 29 {
        let actual = media_store_copy(src, name, &mime)?;
        let shown = format!("Download/{FOLDER}/{actual}");
        log::info!("[chat] saved attachment {name} -> {shown}");
        return Ok(shown);
    }

    await_write_permission().await?;
    let path = legacy_copy(src, name)?;
    log::info!("[chat] saved attachment {name} -> {path}");
    Ok(path)
}

// ---------------------------------------------------------------------------
// Android 10+: MediaStore
// ---------------------------------------------------------------------------

/// Insert a `Downloads` row, stream the bytes into it, then clear the pending
/// flag — the order MediaStore requires so half-written files never become
/// visible to other apps. On any failure the row is deleted again.
fn media_store_copy(src: &Path, name: &str, mime: &str) -> Result<String, String> {
    let (vm, context) = android_context()?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("jni attach: {e}"))?;

    let resolver = call_object(
        &mut env,
        &context,
        "getContentResolver",
        "()Landroid/content/ContentResolver;",
        &[],
    )?;

    let values = content_values(&mut env, name, mime)?;
    let collection = uri_parse(&mut env, MEDIASTORE_DOWNLOADS)?;
    let uri = call_object(
        &mut env,
        &resolver,
        "insert",
        "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;",
        &[JValue::Object(&collection), JValue::Object(&values)],
    )
    .map_err(|e| format!("Android refused the Downloads entry: {e}"))?;
    let null = JObject::null();

    let stream = call_object(
        &mut env,
        &resolver,
        "openOutputStream",
        "(Landroid/net/Uri;)Ljava/io/OutputStream;",
        &[JValue::Object(&uri)],
    )
    .and_then(|out| {
        if out.is_null() {
            Err("Android returned no stream for the Downloads entry".to_string())
        } else {
            Ok(out)
        }
    });

    let written = match stream {
        Ok(out) => {
            let streamed = stream_into(&mut env, &out, src);
            swallow(&mut env, &out, "close", "()V", &[]);
            streamed
        }
        Err(e) => Err(e),
    };
    if let Err(e) = written {
        swallow(
            &mut env,
            &resolver,
            "delete",
            "(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I",
            &[
                JValue::Object(&uri),
                JValue::Object(&null),
                JValue::Object(&null),
            ],
        );
        return Err(e);
    }

    // Publishing the row: is_pending = 0 is what makes the file appear. A row
    // left pending stays invisible to every other app, so a failure here is
    // reported rather than swallowed.
    put_int(&mut env, &values, "is_pending", 0)?;
    let published = call_int(
        &mut env,
        &resolver,
        "update",
        "(Landroid/net/Uri;Landroid/content/ContentValues;Ljava/lang/String;[Ljava/lang/String;)I",
        &[
            JValue::Object(&uri),
            JValue::Object(&values),
            JValue::Object(&null),
            JValue::Object(&null),
        ],
    );
    if published == Ok(0) {
        log::warn!("[chat] MediaStore did not commit the pending entry for {name}");
    }

    // MediaStore renames on a collision (` (1)` and friends), so report what it
    // actually stored rather than what was asked for.
    Ok(cursor_display_name(&mut env, &resolver, &uri).unwrap_or_else(|| name.to_string()))
}

/// `ContentValues` for a new pending download: display name, MIME type, the
/// folder it belongs to, and the pending flag that hides it until committed.
fn content_values(env: &mut JNIEnv, name: &str, mime: &str) -> Result<JObject<'static>, String> {
    let values = env
        .new_object("android/content/ContentValues", "()V", &[])
        .map_err(|e| format!("new ContentValues: {e}"))?;
    put_str(env, &values, "_display_name", name)?;
    put_str(env, &values, "mime_type", mime)?;
    put_str(env, &values, "relative_path", &format!("Download/{FOLDER}"))?;
    put_int(env, &values, "is_pending", 1)?;
    Ok(unsafe { JObject::from_raw(values.into_raw()) })
}

fn put_str(env: &mut JNIEnv, values: &JObject, key: &str, value: &str) -> Result<(), String> {
    let k = env.new_string(key).map_err(|e| e.to_string())?;
    let v = env.new_string(value).map_err(|e| e.to_string())?;
    let k_obj: &JObject = &k;
    let v_obj: &JObject = &v;
    env.call_method(
        values,
        "put",
        "(Ljava/lang/String;Ljava/lang/String;)V",
        &[JValue::Object(k_obj), JValue::Object(v_obj)],
    )
    .map(|_| ())
    .map_err(|e| format!("ContentValues.put({key}): {e}"))
}

fn put_int(env: &mut JNIEnv, values: &JObject, key: &str, value: i32) -> Result<(), String> {
    let k = env.new_string(key).map_err(|e| e.to_string())?;
    let boxed = env
        .new_object("java/lang/Integer", "(I)V", &[JValue::Int(value)])
        .map_err(|e| e.to_string())?;
    let k_obj: &JObject = &k;
    env.call_method(
        values,
        "put",
        "(Ljava/lang/String;Ljava/lang/Integer;)V",
        &[JValue::Object(k_obj), JValue::Object(&boxed)],
    )
    .map(|_| ())
    .map_err(|e| format!("ContentValues.put({key}): {e}"))
}

/// Pump the source file into the Java output stream in 256 KiB writes. Each
/// chunk's local reference is released immediately — a multi-hundred-megabyte
/// attachment would otherwise overflow the JNI local reference table.
fn stream_into(env: &mut JNIEnv, out: &JObject, src: &Path) -> Result<(), String> {
    let mut input =
        std::fs::File::open(src).map_err(|e| format!("cannot open {}: {e}", src.display()))?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = input
            .read(&mut buf)
            .map_err(|e| format!("cannot read {}: {e}", src.display()))?;
        if n == 0 {
            break;
        }
        let chunk = env
            .byte_array_from_slice(&buf[..n])
            .map_err(|e| format!("jni byte array: {e}"))?;
        let chunk_obj: &JObject = &chunk;
        let write = env.call_method(out, "write", "([B)V", &[JValue::Object(chunk_obj)]);
        let _ = env.delete_local_ref(chunk);
        write.map_err(|e| {
            let _ = env.exception_clear();
            format!("write failed: {e}")
        })?;
    }
    call_void(env, out, "flush", "()V", &[])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Android 9 and older: direct writes
// ---------------------------------------------------------------------------

/// `WRITE_EXTERNAL_STORAGE` is a runtime permission below API 29. Ask once
/// through `MainActivity` (which must hop to the UI thread) and then poll, since
/// a plain JNI call has no callback to wait on. Twenty seconds is generous for a
/// dialog tap; past that the user either dismissed it or moved on.
async fn await_write_permission() -> Result<(), String> {
    if write_permission_granted()? {
        return Ok(());
    }
    request_write_permission()?;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if write_permission_granted()? {
            return Ok(());
        }
    }
    Err("Storage permission not granted — allow \"Files and media\" for FmS in \
         Android Settings > Apps > FmS > Permissions, then save again."
        .into())
}

fn write_permission_granted() -> Result<bool, String> {
    let (vm, context) = android_context()?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("jni attach: {e}"))?;
    let permission = env.new_string(WRITE_PERMISSION).map_err(|e| e.to_string())?;
    let permission_obj: &JObject = &permission;
    let granted = env
        .call_method(
            &context,
            "checkSelfPermission",
            "(Ljava/lang/String;)I",
            &[JValue::Object(permission_obj)],
        )
        .map_err(|e| {
            let _ = env.exception_clear();
            e.to_string()
        })?
        .i()
        .map_err(|e| e.to_string())?;
    Ok(granted == 0) // PackageManager.PERMISSION_GRANTED
}

/// `MainActivity.requestStoragePermission` posts `requestPermissions` onto the UI
/// thread — the call is illegal anywhere else.
fn request_write_permission() -> Result<(), String> {
    let (vm, context) = android_context()?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("jni attach: {e}"))?;
    match env.call_static_method(
        "com/wsva/fms_app/MainActivity",
        "requestStoragePermission",
        "(Landroid/app/Activity;)V",
        &[JValue::Object(&context)],
    ) {
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = env.exception_clear();
            Err(format!("cannot request the storage permission: {e}"))
        }
    }
}

/// Copy straight into `/sdcard/Download/FmS/`, leaving an existing file of the
/// same name alone (the new one gets a ` (n)` suffix, like MediaStore does).
fn legacy_copy(src: &Path, name: &str) -> Result<String, String> {
    let dir = legacy_downloads_dir()?.join(FOLDER);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let dest = unique_path(&dir, name);
    std::fs::copy(src, &dest).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    Ok(dest.to_string_lossy().into_owned())
}

/// `Environment.getExternalStoragePublicDirectory(DIRECTORY_DOWNLOADS)`.
fn legacy_downloads_dir() -> Result<PathBuf, String> {
    let (vm, _context) = android_context()?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("jni attach: {e}"))?;
    let kind = env.new_string("Download").map_err(|e| e.to_string())?;
    let kind_obj: &JObject = &kind;
    let dir = env
        .call_static_method(
            "android/os/Environment",
            "getExternalStoragePublicDirectory",
            "(Ljava/lang/String;)Ljava/io/File;",
            &[JValue::Object(kind_obj)],
        )
        .map_err(|e| format!("getExternalStoragePublicDirectory: {e}"))?
        .l()
        .map_err(|e| format!("getExternalStoragePublicDirectory: {e}"))?;
    if dir.is_null() {
        return Err("Android reported no public Downloads directory".into());
    }
    let absolute = call_object(&mut env, &dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?;
    let text = JString::from(absolute);
    let path = env
        .get_string(&text)
        .map_err(|e| e.to_string())?;
    Ok(PathBuf::from(String::from(path)))
}

/// `name`, or `name (n)` when that name is taken in `dir`.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1..1000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem} ({}){ext}", std::process::id()))
}

