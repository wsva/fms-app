//! What a phone picker hands back, resolved into a name and a type.
//!
//! `open()` on Android returns a Storage Access Framework / MediaStore
//! `content://` URI, not a filesystem path — and its tail is a document id, not
//! a filename. The photo picker's
//! `content://media/picker/0/com.android.providers.media.photopicker/media/1000000034`
//! ends in a number, so taking the last path segment produced an attachment
//! literally called `1000000034`: no extension, and therefore a file whose type
//! nothing downstream could recognize (an octet-stream chip instead of a
//! thumbnail, an extensionless copy when saved). The real name lives behind
//! `OpenableColumns.DISPLAY_NAME` and the real type behind
//! `ContentResolver.getType`, both one query away.

use jni::objects::{JObject, JString, JValue};
use jni::JNIEnv;

use super::android_jni::{android_context, call_object, swallow, uri_parse};

/// The picked document's own name (`OpenableColumns.DISPLAY_NAME`), e.g.
/// `IMG_20260112_101112.jpg`. `None` when the provider refuses to answer, which
/// is the caller's cue to fall back to the URI tail.
pub(super) fn display_name(uri: &str) -> Option<String> {
    let (vm, context) = android_context().ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let resolver = call_object(
        &mut env,
        &context,
        "getContentResolver",
        "()Landroid/content/ContentResolver;",
        &[],
    )
    .ok()?;
    let parsed = uri_parse(&mut env, uri).ok()?;
    cursor_display_name(&mut env, &resolver, &parsed)
}

/// The type the provider reports for the document (`ContentResolver.getType`),
/// e.g. `image/jpeg`. Used when the picked name carries no usable extension, and
/// preferred over the extension table otherwise — the provider knows, the name
/// only hints.
pub(super) fn mime_type(uri: &str) -> Option<String> {
    let (vm, context) = android_context().ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let resolver = call_object(
        &mut env,
        &context,
        "getContentResolver",
        "()Landroid/content/ContentResolver;",
        &[],
    )
    .ok()?;
    let parsed = uri_parse(&mut env, uri).ok()?;
    let kind = env
        .call_method(
            &resolver,
            "getType",
            "(Landroid/net/Uri;)Ljava/lang/String;",
            &[JValue::Object(&parsed)],
        )
        .ok()?
        .l()
        .ok()?;
    if kind.is_null() {
        return None;
    }
    env.get_string(&JString::from(kind)).ok().map(String::from)
}

/// Read `_display_name` off a `query` of `uri`. A null projection is passed
/// rather than a one-column one, because a provider is free to rename or drop a
/// requested column; looking the name up on whatever came back tolerates both.
/// Shared with the save path, which asks MediaStore what it actually stored
/// (it appends ` (1)` on a collision).
pub(super) fn cursor_display_name(
    env: &mut JNIEnv,
    resolver: &JObject,
    uri: &JObject,
) -> Option<String> {
    let null = JObject::null();
    let cursor = call_object(
        env,
        resolver,
        "query",
        "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)\
         Landroid/database/Cursor;",
        &[
            JValue::Object(uri),
            JValue::Object(&null),
            JValue::Object(&null),
            JValue::Object(&null),
            JValue::Object(&null),
        ],
    )
    .ok()?;
    if cursor.is_null() {
        return None;
    }
    let key = env.new_string("_display_name").ok()?;
    let col = env
        .call_method(
            &cursor,
            "getColumnIndex",
            "(Ljava/lang/String;)I",
            &[JValue::Object(&key)],
        )
        .ok()?
        .i()
        .ok()?;
    let name = if col >= 0
        && env
            .call_method(&cursor, "moveToFirst", "()Z", &[])
            .ok()?
            .z()
            .unwrap_or(false)
    {
        let value = env
            .call_method(
                &cursor,
                "getString",
                "(I)Ljava/lang/String;",
                &[JValue::Int(col)],
            )
            .ok()?
            .l()
            .ok()?;
        if value.is_null() {
            None
        } else {
            env.get_string(&JString::from(value)).ok().map(String::from)
        }
    } else {
        None
    };
    swallow(env, &cursor, "close", "()V", &[]);
    name
}
