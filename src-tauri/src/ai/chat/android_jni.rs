//! The JNI plumbing both Android bridges of this group share.
//!
//! Every attachment direction on a phone goes through the ContentResolver: a
//! picked `content://` URI is read into a real file (`android_content` resolves
//! its name and type, `chat::copy_content_uri` the bytes), and a saved one is
//! written into the public Downloads collection (`android_downloads`). They all
//! need the same handful of calls — the app context, `Uri.parse`, one Java
//! method invocation with the exception cleared — so they live here once.

use jni::objects::{JObject, JValue};
use jni::JNIEnv;

/// The app's `Context` plus a Java VM handle, taken from Tauri's Android
/// runtime (`ndk_context`) rather than looked up again.
pub(super) fn android_context() -> Result<(jni::JavaVM, JObject<'static>), String> {
    let ctx = ndk_context::android_context();
    let vm =
        unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| format!("jni vm: {e}"))?;
    let context = unsafe { JObject::from_raw(ctx.context().cast()) };
    Ok((vm, context))
}

pub(super) fn sdk_int() -> Result<i32, String> {
    let (vm, _context) = android_context()?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("jni attach: {e}"))?;
    let version = env
        .find_class("android/os/Build$VERSION")
        .map_err(|e| format!("jni find_class: {e}"))?;
    env.get_static_field(version, "SDK_INT", "I")
        .map_err(|e| format!("jni SDK_INT: {e}"))?
        .i()
        .map_err(|e| format!("jni SDK_INT: {e}"))
}

/// `android.net.Uri.parse(text)`.
pub(super) fn uri_parse(env: &mut JNIEnv, text: &str) -> Result<JObject<'static>, String> {
    let raw = env.new_string(text).map_err(|e| e.to_string())?;
    let raw_obj: &JObject = &raw;
    let parsed = env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(raw_obj)],
        )
        .map(|v| v.l())
        .map_err(|e| format!("Uri.parse({text}): {e}"))?
        .map_err(|e| format!("Uri.parse({text}): {e}"))?;
    Ok(unsafe { JObject::from_raw(parsed.into_raw()) })
}

/// One object-returning Java call, with the `null` check every caller here needs.
/// A Java exception is cleared as soon as it is seen: leaving one pending makes
/// every later JNI call on this thread fail, and the caller only ever reports
/// the message.
pub(super) fn call_object(
    env: &mut JNIEnv,
    receiver: &JObject,
    name: &str,
    sig: &str,
    args: &[JValue],
) -> Result<JObject<'static>, String> {
    let value = match env.call_method(receiver, name, sig, args) {
        Ok(v) => v.l().map_err(|e| format!("{name}: {e}"))?,
        Err(e) => {
            let _ = env.exception_clear();
            return Err(format!("{name}: {e}"));
        }
    };
    if value.is_null() {
        return Err(format!("{name} returned null"));
    }
    Ok(unsafe { JObject::from_raw(value.into_raw()) })
}

/// One `int`-returning Java call, exception cleared on failure.
pub(super) fn call_int(
    env: &mut JNIEnv,
    receiver: &JObject,
    name: &str,
    sig: &str,
    args: &[JValue],
) -> Result<i32, String> {
    let value = match env.call_method(receiver, name, sig, args) {
        Ok(v) => v,
        Err(e) => {
            let _ = env.exception_clear();
            return Err(format!("{name}: {e}"));
        }
    };
    value.i().map_err(|e| format!("{name}: {e}"))
}

/// One `void`-returning Java call, exception cleared on failure.
pub(super) fn call_void(
    env: &mut JNIEnv,
    receiver: &JObject,
    name: &str,
    sig: &str,
    args: &[JValue],
) -> Result<(), String> {
    match env.call_method(receiver, name, sig, args) {
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = env.exception_clear();
            Err(format!("{name}: {e}"))
        }
    }
}

/// A call whose failure is uninteresting (closing a stream, deleting a row we
/// are abandoning) — but whose exception must still be cleared.
pub(super) fn swallow(
    env: &mut JNIEnv,
    receiver: &JObject,
    name: &str,
    sig: &str,
    args: &[JValue],
) {
    let _ = call_void(env, receiver, name, sig, args);
}
