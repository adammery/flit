//! Attachment files on disk: metadata for files dropped on the compose
//! window (only stat calls — the bytes are read when the MIME message is
//! built, mail::smtp), and the copies Quick Look shows of received
//! attachments that were never saved.

use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::models::AttachmentInfo;

/// Where Quick Look's copy of a received attachment lives under the app
/// cache: a folder per attachment id, so two attachments named alike never
/// collide, holding the file under its own name — the panel shows that
/// name as its title.
pub fn quick_look_copy(cache_dir: &Path, attachment_id: i64, filename: &str) -> PathBuf {
    cache_dir
        .join("quick-look")
        .join(attachment_id.to_string())
        .join(filename)
}

/// Write a Quick Look copy. It is written beside its final name and renamed
/// into place, so a fetch cut off half-way never leaves a truncated file
/// that a later Space would show as if it were whole.
pub async fn write_quick_look_copy(path: &Path, data: &[u8]) -> Result<(), AppError> {
    let dir = path
        .parent()
        .ok_or_else(|| AppError::Invalid(format!("{} has no folder", path.display())))?;
    tokio::fs::create_dir_all(dir).await?;
    let partial = dir.join(".partial");
    tokio::fs::write(&partial, data).await?;
    tokio::fs::rename(&partial, path).await?;
    Ok(())
}

/// Delete every Quick Look copy. Runs at launch: they are mail content on
/// disk outside the database, and must not pile up there.
pub async fn clear_quick_look_copies(cache_dir: &Path) -> Result<(), AppError> {
    match tokio::fs::remove_dir_all(cache_dir.join("quick-look")).await {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
        _ => Ok(()),
    }
}

/// Stat dropped paths into chip metadata: duplicates collapse (same file
/// dropped twice attaches once), directories are skipped (a drop of e.g. a
/// Finder selection may include folders we can't attach). A missing or
/// unreadable file errors so the user learns at drop time, not at send time.
pub async fn inspect(paths: Vec<String>) -> Result<Vec<AttachmentInfo>, AppError> {
    let mut seen = std::collections::HashSet::new();
    let mut infos = Vec::new();
    for path in paths {
        if !seen.insert(path.clone()) {
            continue;
        }
        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|e| AppError::Smtp(format!("cannot attach {path}: {e}")))?;
        if meta.is_dir() {
            continue;
        }
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        infos.push(AttachmentInfo {
            path,
            name,
            size: meta.len(),
        });
    }
    Ok(infos)
}

/// Sources above this size get no thumbnail — decoding a huge image for a
/// 256px preview is wasted work and a decompression-bomb risk.
const MAX_PREVIEW_SOURCE_BYTES: u64 = 10 * 1024 * 1024;
/// Longest edge of the generated thumbnail.
const PREVIEW_EDGE: u32 = 256;

/// Best-effort thumbnail for one attachment as a `data:image/png` URI.
/// `None` for anything that is not a decodable raster image (only PNG, JPEG
/// and WebP are compiled in) — the card then shows a generic placeholder.
/// Never errors: a preview is decoration, not part of the message.
pub async fn preview(path: String) -> Option<String> {
    let meta = tokio::fs::metadata(&path).await.ok()?;
    if !meta.is_file() || meta.len() > MAX_PREVIEW_SOURCE_BYTES {
        return None;
    }
    // why spawn_blocking: decode + resize is CPU-bound; on the async runtime
    // it would stall every other task for the duration.
    tokio::task::spawn_blocking(move || render_thumbnail(&path))
        .await
        .ok()
        .flatten()
}

fn render_thumbnail(path: &str) -> Option<String> {
    // why with_guessed_format: sniffs the actual bytes instead of trusting
    // the extension, so a mislabeled file can't pick a decoder we did not
    // intend (and unsupported formats bail out to None here).
    let img = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let thumb = img.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE);
    let mut buf = Vec::new();
    thumb
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    use base64::Engine;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(buf)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, contents: &[u8]) -> String {
        let dir = std::env::temp_dir().join("flit-attach-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn returns_name_and_size_per_file() {
        let path = temp_file("notes.txt", b"hello");

        let infos = inspect(vec![path.clone()]).await.unwrap();

        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].path, path);
        assert_eq!(infos[0].name, "notes.txt");
        assert_eq!(infos[0].size, 5);
    }

    #[tokio::test]
    async fn duplicate_paths_collapse_to_one() {
        let path = temp_file("dup.txt", b"x");

        let infos = inspect(vec![path.clone(), path]).await.unwrap();

        assert_eq!(infos.len(), 1);
    }

    #[tokio::test]
    async fn directories_are_skipped() {
        let file = temp_file("kept.txt", b"x");
        let dir = std::env::temp_dir().to_string_lossy().into_owned();

        let infos = inspect(vec![dir, file]).await.unwrap();

        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].name, "kept.txt");
    }

    #[tokio::test]
    async fn preview_thumbnails_a_png() {
        let dir = std::env::temp_dir().join("flit-attach-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tiny.png");
        image::RgbImage::from_pixel(4, 4, image::Rgb([200, 40, 40]))
            .save(&path)
            .unwrap();

        let uri = preview(path.to_string_lossy().into_owned()).await;

        assert!(uri.unwrap().starts_with("data:image/png;base64,"));
    }

    #[tokio::test]
    async fn preview_declines_non_images() {
        let path = temp_file("not-an-image.pdf", b"%PDF-1.4 nope");

        assert_eq!(preview(path).await, None);
    }

    #[tokio::test]
    async fn preview_declines_missing_and_oversized_files() {
        assert_eq!(preview("/nonexistent/flit/img.png".to_string()).await, None);

        let big = vec![0u8; (MAX_PREVIEW_SOURCE_BYTES + 1) as usize];
        let path = temp_file("huge.png", &big);
        assert_eq!(preview(path).await, None);
    }

    #[test]
    fn each_attachment_gets_its_own_quick_look_folder() {
        let cache = std::path::Path::new("/cache");

        let copy = quick_look_copy(cache, 7, "report.pdf");

        // Same name in two messages never collides; the file keeps its
        // name, which the panel shows as the title.
        assert_eq!(copy, cache.join("quick-look/7/report.pdf"));
        assert_ne!(copy, quick_look_copy(cache, 8, "report.pdf"));
    }

    #[tokio::test]
    async fn a_quick_look_copy_appears_only_once_complete() {
        let cache = std::env::temp_dir().join(format!("flit-ql-write-{}", std::process::id()));
        let copy = quick_look_copy(&cache, 3, "notes.txt");

        write_quick_look_copy(&copy, b"hello").await.unwrap();

        assert_eq!(tokio::fs::read(&copy).await.unwrap(), b"hello");
        // Nothing but the file itself is left in its folder.
        let mut entries = tokio::fs::read_dir(copy.parent().unwrap()).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name());
        }
        assert_eq!(names, ["notes.txt"]);
        let _ = tokio::fs::remove_dir_all(&cache).await;
    }

    #[tokio::test]
    async fn clearing_quick_look_copies_leaves_the_rest_of_the_cache() {
        let cache = std::env::temp_dir().join(format!("flit-ql-clear-{}", std::process::id()));
        let copy = quick_look_copy(&cache, 3, "notes.txt");
        write_quick_look_copy(&copy, b"hello").await.unwrap();
        tokio::fs::write(cache.join("other"), b"x").await.unwrap();

        clear_quick_look_copies(&cache).await.unwrap();

        assert!(!tokio::fs::try_exists(cache.join("quick-look"))
            .await
            .unwrap());
        assert!(tokio::fs::try_exists(cache.join("other")).await.unwrap());
        // Nothing to clear (first launch) is fine too.
        clear_quick_look_copies(&cache).await.unwrap();
        let _ = tokio::fs::remove_dir_all(&cache).await;
    }

    #[tokio::test]
    async fn missing_files_error_at_drop_time() {
        let err = inspect(vec!["/nonexistent/flit/void.txt".to_string()])
            .await
            .unwrap_err();

        assert!(err.to_string().contains("void.txt"));
    }
}
