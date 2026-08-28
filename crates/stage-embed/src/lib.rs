//! Image and text embedding stages.

pub mod embedder;
pub mod preprocess;
pub mod tokenizer;

use anyhow::{ensure, Context};
use crush_store::Store;
use embedder::Embedder;

/// Embed every shot in a video that does not already have a stored vector.
///
/// Version 1 intentionally runs a batch of one so the persistence and recovery contract stays
/// simple. The thumbnail path is validated by [`Store::thumbnail_path`].
pub fn embed_missing_shots(
    store: &Store,
    owner_id: &str,
    video_id: &str,
    embedder: &mut Embedder,
) -> anyhow::Result<usize> {
    let mut embedded = 0;
    for shot in store.shots_for_video(owner_id, video_id)? {
        if store.vector_for_shot(owner_id, &shot.id)?.is_some() {
            continue;
        }
        let relative = shot
            .thumb_rel
            .as_deref()
            .with_context(|| format!("shot {} has no thumbnail", shot.id))?;
        let path = store.thumbnail_path(relative)?;
        ensure!(
            path.is_file(),
            "shot {} thumbnail is missing: {}",
            shot.id,
            path.display()
        );
        let image = image::open(&path)
            .with_context(|| format!("failed to decode thumbnail {}", path.display()))?;
        let tensor = preprocess::preprocess(&image);
        let vector = embedder.embed_image(&tensor)?;
        store.put_vector(owner_id, &shot.id, &vector)?;
        embedded += 1;
    }
    Ok(embedded)
}
