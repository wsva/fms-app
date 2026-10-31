//! Extension-only media classification, shared by every surface that has to
//! decide what a file *is* before it reads it.
//!
//! This is the single place that names the extension lists. Classification used
//! to live in [`super::wiki`] as two private consts, which forced a third copy
//! in the wiki page's own TypeScript (`VIDEO_EXTS`, carrying a "must stay in
//! sync" comment), and a card reader then needed a fourth. One module means one
//! list per medium, and adding a format is one line here.
//!
//! The rule is deliberately extension-only: no probing, no magic bytes. A
//! preview path and an attach path both need a fast, allocation-free answer to
//! "how do I render this", and both get it wrong in the same way when a file is
//! misnamed — which is the user's problem to fix, not the classifier's to guess.
//!
//! Two sets live here. [`kind_of`] is the broad set the wiki viewer answers
//! with, chosen so nothing that a webview can stream is refused.
//! [`card_kind_of`] is a deliberately narrower set: card media is authored on a
//! PC and played on a phone, and Android WebView will not reliably decode the
//! extras (`mov`, `m4b`, `opus`, `flac`, `avif`), so a card that carried them
//! would look fine on the machine that made it and broken everywhere else.

use std::path::Path;

/// What a file is, decided by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaKind {
    Image,
    Audio,
    Video,
    /// Anything else — including a file with no extension at all.
    Other,
}

impl MediaKind {
    /// The value stored in a `kind` column and returned over the wire.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Audio => "audio",
            MediaKind::Video => "video",
            MediaKind::Other => "other",
        }
    }
}

// ---------------------------------------------------------------------------
// Extension lists
// ---------------------------------------------------------------------------

pub(crate) const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif",
];
pub(crate) const AUDIO_EXTS: &[&str] = &[
    "mp3", "wav", "m4a", "m4b", "ogg", "opus", "flac", "aac",
];
pub(crate) const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mov", "m4v"];

/// The subset safe to freeze into a card: every entry plays in both WebView2 on
/// Windows and the Android System WebView, so a card made on either device
/// renders on the other. Keep this a strict subset of the three lists above —
/// `card_kind_of` is the only gate a card author sees.
pub(crate) const CARD_IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];
pub(crate) const CARD_AUDIO_EXTS: &[&str] = &["mp3", "m4a", "wav", "ogg"];
pub(crate) const CARD_VIDEO_EXTS: &[&str] = &["mp4", "webm"];

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Lowercased extension of a file name or dataset-relative path ("" when none;
/// dotfiles like `.gitignore` have no extension per [`Path::extension`]).
pub(crate) fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Classify against the broad set. Audio and video stay distinct here; a caller
/// that lumps them (the wiki's `kind` string) maps both to its own name.
pub(crate) fn kind_of(name: &str) -> MediaKind {
    let ext = extension_of(name);
    if ext.is_empty() {
        return MediaKind::Other;
    }
    if IMAGE_EXTS.contains(&ext.as_str()) {
        MediaKind::Image
    } else if AUDIO_EXTS.contains(&ext.as_str()) {
        MediaKind::Audio
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        MediaKind::Video
    } else {
        MediaKind::Other
    }
}

/// Classify against the card-portable set, refusing anything else. A `None` is
/// an actionable answer, not a silent one: the attach command turns it into an
/// error naming the extension and the list that is accepted.
pub(crate) fn card_kind_of(name: &str) -> Option<MediaKind> {
    let ext = extension_of(name);
    if CARD_IMAGE_EXTS.contains(&ext.as_str()) {
        Some(MediaKind::Image)
    } else if CARD_AUDIO_EXTS.contains(&ext.as_str()) {
        Some(MediaKind::Audio)
    } else if CARD_VIDEO_EXTS.contains(&ext.as_str()) {
        Some(MediaKind::Video)
    } else {
        None
    }
}

/// The accepted extensions of one card medium, for a file-picker filter and for
/// the error message that names what a rejected file would have needed.
pub(crate) fn card_exts_for(kind: MediaKind) -> &'static [&'static str] {
    match kind {
        MediaKind::Image => CARD_IMAGE_EXTS,
        MediaKind::Audio => CARD_AUDIO_EXTS,
        MediaKind::Video => CARD_VIDEO_EXTS,
        // Not a medium a card can hold. Nothing reaches this arm: the picker
        // and `card_kind_of` only ever produce the three above.
        MediaKind::Other => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_of_ignores_directories_and_dotfiles() {
        assert_eq!(extension_of("media/photo.JPG"), "jpg");
        assert_eq!(extension_of(".gitignore"), "");
        assert_eq!(extension_of("no_extension"), "");
        assert_eq!(extension_of("archive.tar.gz"), "gz");
    }

    #[test]
    fn kind_of_covers_the_viewer_set() {
        assert_eq!(kind_of("a.svg"), MediaKind::Image);
        assert_eq!(kind_of("a.flac"), MediaKind::Audio);
        assert_eq!(kind_of("a.mov"), MediaKind::Video);
        assert_eq!(kind_of("readme.md"), MediaKind::Other);
    }

    /// The point of the narrow set: a format the wiki will happily stream and a
    /// card must refuse, so a phone never meets it.
    #[test]
    fn card_set_refuses_what_android_webview_may_not_play() {
        assert_eq!(card_kind_of("clip.mov"), None);
        assert_eq!(card_kind_of("audiobook.m4b"), None);
        assert_eq!(card_kind_of("picture.avif"), None);
        assert_eq!(card_kind_of("clip.mp4"), Some(MediaKind::Video));
        assert_eq!(card_kind_of("clip.mp3"), Some(MediaKind::Audio));
        assert_eq!(card_kind_of("pic.webp"), Some(MediaKind::Image));
    }

    /// `card_exts_for` is the list a picker and an error message both read, so
    /// an empty answer for a real medium would silently offer nothing.
    #[test]
    fn every_card_medium_names_its_extensions() {
        for kind in [MediaKind::Image, MediaKind::Audio, MediaKind::Video] {
            assert!(!card_exts_for(kind).is_empty(), "{kind:?} names no extensions");
            for ext in card_exts_for(kind) {
                assert!(
                    matches!(kind_of(&format!("x.{ext}")), k if k == kind),
                    "card list for {kind:?} disagrees with the broad set on {ext}"
                );
            }
        }
    }

    #[test]
    fn stored_kind_strings_are_the_ones_the_columns_hold() {
        // The `kind` column and the wire field both carry these, and a rename
        // here is a data migration — pinning them is the point of the test.
        assert_eq!(MediaKind::Image.as_str(), "image");
        assert_eq!(MediaKind::Audio.as_str(), "audio");
        assert_eq!(MediaKind::Video.as_str(), "video");
    }
}
