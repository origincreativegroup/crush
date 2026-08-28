//! SQLite persistence for Crush.
//!
//! This crate is the only place in the product that contains SQL. It owns migrations, typed
//! records, vector serialization, FTS synchronization, job history, and deep integrity checks.

use std::{
    convert::TryFrom,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, ensure, Context};
use chrono::{DateTime, Utc};
use crush_core::{job::JobRecord, job::JobStatus, job::Stage};
use rusqlite::{params, types::Type, Connection, OptionalExtension, Row, TransactionBehavior};

const CURRENT_SCHEMA_VERSION: i64 = 1;
const MIGRATIONS: &[(i64, &str)] = &[(1, include_str!("../migrations/0001_init.sql"))];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoStatus {
    Pending,
    Split,
    Embedded,
    Transcribed,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Video {
    pub id: String,
    pub owner_id: String,
    pub path: String,
    pub sha256: String,
    pub duration_s: Option<f64>,
    pub fps: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub has_audio: bool,
    pub status: VideoStatus,
    pub indexed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub id: String,
    pub video_id: String,
    pub owner_id: String,
    pub idx: i64,
    pub start_s: f64,
    pub end_s: f64,
    pub rep_frame_s: f64,
    /// Path relative to `<data_dir>/thumbs`.
    pub thumb_rel: Option<String>,
    pub scene_score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSegment {
    pub id: String,
    pub video_id: String,
    pub owner_id: String,
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
    pub confidence: Option<f64>,
}

/// Store-owned projection used to hydrate a ranked in-memory vector match.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchShotContext {
    pub shot_id: String,
    pub video_id: String,
    pub video_path: String,
    pub start_s: f64,
    pub end_s: f64,
    pub thumb_rel: Option<String>,
}

/// A matching transcript segment joined to every shot whose interval it overlaps.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptShotHit {
    pub shot_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingMeta {
    pub owner_id: String,
    pub model_name: String,
    pub model_sha256: String,
    pub dim: usize,
    pub preprocess_version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    pub id: String,
    pub video_id: String,
    pub stage: Stage,
    pub started_at: DateTime<Utc>,
    pub debug_dir: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobFilter {
    pub video_id: Option<String>,
    pub stage: Option<Stage>,
    pub status: Option<JobStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    MissingVector,
    MissingThumbnail,
    UnsafeThumbnailPath,
    OrphanVector,
    InvalidVectorBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub kind: ProblemKind,
    pub entity_id: String,
    pub detail: String,
}

pub struct Store {
    connection: Connection,
    data_dir: PathBuf,
    db_path: PathBuf,
}

impl Store {
    /// Open `<data_dir>/library.db`, applying pending migrations before returning.
    pub fn open(data_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("failed to create data directory {}", data_dir.display()))?;
        std::fs::create_dir_all(data_dir.join("thumbs"))?;
        let db_path = data_dir.join("library.db");
        let mut connection = Connection::open(&db_path)
            .with_context(|| format!("failed to open SQLite database {}", db_path.display()))?;

        configure_connection(&connection)?;
        apply_migrations(&mut connection)?;

        Ok(Self {
            connection,
            data_dir,
            db_path,
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn thumbnail_path(&self, relative: &str) -> anyhow::Result<PathBuf> {
        let relative = Path::new(relative);
        ensure!(
            safe_relative_path(relative),
            "thumbnail path must be a safe path relative to the thumbs directory"
        );
        Ok(self.data_dir.join("thumbs").join(relative))
    }

    pub fn schema_version(&self) -> anyhow::Result<i64> {
        self.connection
            .query_row(
                "SELECT version FROM schema_version WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .context("failed to read schema version")
    }

    pub fn upsert_video(&self, owner_id: &str, video: &Video) -> anyhow::Result<Video> {
        ensure_owner_matches(owner_id, &video.owner_id, "video")?;
        self.connection.execute(
            "INSERT INTO videos (
                id, owner_id, path, sha256, duration_s, fps, width, height, has_audio, status,
                indexed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(owner_id, sha256) DO UPDATE SET
                path = excluded.path,
                duration_s = excluded.duration_s,
                fps = excluded.fps,
                width = excluded.width,
                height = excluded.height,
                has_audio = excluded.has_audio,
                status = excluded.status,
                indexed_at = excluded.indexed_at",
            params![
                video.id,
                owner_id,
                video.path,
                video.sha256,
                video.duration_s,
                video.fps,
                video.width,
                video.height,
                video.has_audio,
                video_status_to_str(video.status),
                video.indexed_at.map(|value| value.to_rfc3339()),
            ],
        )?;

        self.video_by_sha(owner_id, &video.sha256)?
            .context("upserted video could not be read back")
    }

    pub fn video_by_sha(&self, owner_id: &str, sha256: &str) -> anyhow::Result<Option<Video>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, path, sha256, duration_s, fps, width, height, has_audio,
                        status, indexed_at
                 FROM videos
                 WHERE owner_id = ?1 AND sha256 = ?2",
                params![owner_id, sha256],
                video_from_row,
            )
            .optional()
            .context("failed to query video by sha256")
    }

    pub fn video_by_id(&self, owner_id: &str, video_id: &str) -> anyhow::Result<Option<Video>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, path, sha256, duration_s, fps, width, height, has_audio,
                        status, indexed_at
                 FROM videos
                 WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, video_id],
                video_from_row,
            )
            .optional()
            .context("failed to query video by id")
    }

    pub fn video_by_path(&self, owner_id: &str, path: &str) -> anyhow::Result<Option<Video>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, path, sha256, duration_s, fps, width, height, has_audio,
                        status, indexed_at
                 FROM videos
                 WHERE owner_id = ?1 AND path = ?2",
                params![owner_id, path],
                video_from_row,
            )
            .optional()
            .context("failed to query video by path")
    }

    pub fn videos(&self, owner_id: &str) -> anyhow::Result<Vec<Video>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, path, sha256, duration_s, fps, width, height, has_audio,
                    status, indexed_at
             FROM videos
             WHERE owner_id = ?1
             ORDER BY path, id",
        )?;
        let rows = statement.query_map(params![owner_id], video_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list videos")
    }

    pub fn set_video_status(
        &self,
        owner_id: &str,
        video_id: &str,
        status: VideoStatus,
    ) -> anyhow::Result<()> {
        let status_text = video_status_to_str(status);
        let indexed_at = (status == VideoStatus::Done).then(|| Utc::now().to_rfc3339());
        let changed = self.connection.execute(
            "UPDATE videos
             SET status = ?3,
                 indexed_at = CASE WHEN ?3 = 'done' THEN COALESCE(indexed_at, ?4)
                                   ELSE indexed_at END
             WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, video_id, status_text, indexed_at],
        )?;
        ensure_changed(changed, "video", video_id)
    }

    pub fn insert_shots(&mut self, owner_id: &str, shots: &[Shot]) -> anyhow::Result<()> {
        for shot in shots {
            ensure_owner_matches(owner_id, &shot.owner_id, "shot")?;
            validate_shot(shot)?;
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO shots (
                    id, video_id, owner_id, idx, start_s, end_s, rep_frame_s, thumb_rel,
                    scene_score
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for shot in shots {
                statement.execute(params![
                    shot.id,
                    shot.video_id,
                    owner_id,
                    shot.idx,
                    shot.start_s,
                    shot.end_s,
                    shot.rep_frame_s,
                    shot.thumb_rel,
                    shot.scene_score,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Replace a video's shot graph and advance it to `split` in one transaction.
    pub fn replace_shots(
        &mut self,
        owner_id: &str,
        video_id: &str,
        shots: &[Shot],
    ) -> anyhow::Result<()> {
        ensure!(!shots.is_empty(), "cannot replace with an empty shot list");
        for shot in shots {
            ensure_owner_matches(owner_id, &shot.owner_id, "shot")?;
            ensure!(
                shot.video_id == video_id,
                "shot belongs to a different video"
            );
            validate_shot(shot)?;
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "DELETE FROM shots WHERE owner_id = ?1 AND video_id = ?2",
            params![owner_id, video_id],
        )?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO shots (
                    id, video_id, owner_id, idx, start_s, end_s, rep_frame_s, thumb_rel,
                    scene_score
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for shot in shots {
                statement.execute(params![
                    shot.id,
                    video_id,
                    owner_id,
                    shot.idx,
                    shot.start_s,
                    shot.end_s,
                    shot.rep_frame_s,
                    shot.thumb_rel,
                    shot.scene_score,
                ])?;
            }
        }
        let changed = transaction.execute(
            "UPDATE videos SET status = 'split' WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, video_id],
        )?;
        ensure_changed(changed, "video", video_id)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn delete_vectors_for_video(
        &self,
        owner_id: &str,
        video_id: &str,
    ) -> anyhow::Result<usize> {
        self.connection
            .execute(
                "DELETE FROM shot_vectors
                 WHERE owner_id = ?1 AND shot_id IN (
                     SELECT id FROM shots WHERE owner_id = ?1 AND video_id = ?2
                 )",
                params![owner_id, video_id],
            )
            .context("failed to delete video vectors")
    }

    pub fn shots_for_video(&self, owner_id: &str, video_id: &str) -> anyhow::Result<Vec<Shot>> {
        let mut statement = self.connection.prepare(
            "SELECT id, video_id, owner_id, idx, start_s, end_s, rep_frame_s, thumb_rel,
                    scene_score
             FROM shots
             WHERE owner_id = ?1 AND video_id = ?2
             ORDER BY idx",
        )?;
        let rows = statement.query_map(params![owner_id, video_id], shot_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to load shots for video")
    }

    pub fn shot_by_id(&self, owner_id: &str, shot_id: &str) -> anyhow::Result<Option<Shot>> {
        self.connection
            .query_row(
                "SELECT id, video_id, owner_id, idx, start_s, end_s, rep_frame_s, thumb_rel,
                        scene_score
                 FROM shots
                 WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, shot_id],
                shot_from_row,
            )
            .optional()
            .context("failed to query shot by id")
    }

    pub fn put_vector(&self, owner_id: &str, shot_id: &str, values: &[f32]) -> anyhow::Result<()> {
        ensure!(!values.is_empty(), "vector must not be empty");
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }

        self.connection.execute(
            "INSERT INTO shot_vectors (shot_id, owner_id, dim, vec)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(shot_id) DO UPDATE SET
                owner_id = excluded.owner_id,
                dim = excluded.dim,
                vec = excluded.vec",
            params![shot_id, owner_id, values.len() as i64, bytes],
        )?;
        Ok(())
    }

    /// Load one vector, preserving its exact little-endian f32 representation.
    pub fn vector_for_shot(
        &self,
        owner_id: &str,
        shot_id: &str,
    ) -> anyhow::Result<Option<Vec<f32>>> {
        let row = self
            .connection
            .query_row(
                "SELECT dim, vec FROM shot_vectors WHERE owner_id = ?1 AND shot_id = ?2",
                params![owner_id, shot_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?;
        let Some((dim_i64, bytes)) = row else {
            return Ok(None);
        };
        let dim = usize::try_from(dim_i64).context("vector dimension was negative")?;
        ensure!(
            bytes.len() == dim * size_of::<f32>(),
            "vector {shot_id} contains {} bytes; expected {}",
            bytes.len(),
            dim * size_of::<f32>()
        );
        Ok(Some(
            bytes
                .as_chunks::<{ size_of::<f32>() }>()
                .0
                .iter()
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect(),
        ))
    }

    /// Return shot ids plus one contiguous row-major vector matrix.
    pub fn load_all_vectors(&self, owner_id: &str) -> anyhow::Result<(Vec<String>, Vec<f32>)> {
        let mut statement = self.connection.prepare(
            "SELECT shot_id, dim, vec
             FROM shot_vectors
             WHERE owner_id = ?1
             ORDER BY shot_id",
        )?;
        let mut rows = statement.query(params![owner_id])?;
        let mut ids = Vec::new();
        let mut matrix = Vec::new();
        let mut expected_dim = None;

        while let Some(row) = rows.next()? {
            let shot_id: String = row.get(0)?;
            let dim_i64: i64 = row.get(1)?;
            let dim = usize::try_from(dim_i64).context("vector dimension was negative")?;
            let bytes: Vec<u8> = row.get(2)?;
            let expected_bytes = dim
                .checked_mul(size_of::<f32>())
                .context("vector byte length overflowed usize")?;
            ensure!(
                bytes.len() == expected_bytes,
                "vector {shot_id} contains {} bytes; expected {} for dim {dim}",
                bytes.len(),
                expected_bytes
            );
            if let Some(previous) = expected_dim {
                ensure!(
                    previous == dim,
                    "vector {shot_id} has dim {dim}; expected uniform dim {previous}"
                );
            } else {
                expected_dim = Some(dim);
                matrix
                    .try_reserve(dim)
                    .context("failed to reserve the vector matrix")?;
            }

            // Length was validated above, so every chunk contains exactly one little-endian f32.
            for chunk in bytes.chunks(size_of::<f32>()) {
                matrix.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
            ids.push(shot_id);
        }

        Ok((ids, matrix))
    }

    pub fn insert_transcript_segments(
        &mut self,
        owner_id: &str,
        segments: &[TranscriptSegment],
    ) -> anyhow::Result<()> {
        for segment in segments {
            ensure_owner_matches(owner_id, &segment.owner_id, "transcript segment")?;
            ensure!(
                segment.end_s > segment.start_s,
                "segment end must exceed start"
            );
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for segment in segments {
            transaction.execute(
                "INSERT INTO transcripts (
                    id, video_id, owner_id, start_s, end_s, text, confidence
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    segment.id,
                    segment.video_id,
                    owner_id,
                    segment.start_s,
                    segment.end_s,
                    segment.text,
                    segment.confidence,
                ],
            )?;
            let rowid = transaction.last_insert_rowid();
            transaction.execute(
                "INSERT INTO transcripts_fts(rowid, text) VALUES (?1, ?2)",
                params![rowid, segment.text],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Atomically replace one video's transcript and its external-content FTS rows.
    pub fn replace_transcript_segments(
        &mut self,
        owner_id: &str,
        video_id: &str,
        segments: &[TranscriptSegment],
    ) -> anyhow::Result<()> {
        for segment in segments {
            ensure_owner_matches(owner_id, &segment.owner_id, "transcript segment")?;
            ensure!(
                segment.video_id == video_id,
                "transcript segment belongs to a different video"
            );
            ensure!(
                segment.end_s > segment.start_s,
                "segment end must exceed start"
            );
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "DELETE FROM transcripts_fts
             WHERE rowid IN (
                 SELECT rowid FROM transcripts WHERE owner_id = ?1 AND video_id = ?2
             )",
            params![owner_id, video_id],
        )?;
        transaction.execute(
            "DELETE FROM transcripts WHERE owner_id = ?1 AND video_id = ?2",
            params![owner_id, video_id],
        )?;
        for segment in segments {
            transaction.execute(
                "INSERT INTO transcripts (
                    id, video_id, owner_id, start_s, end_s, text, confidence
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    segment.id,
                    video_id,
                    owner_id,
                    segment.start_s,
                    segment.end_s,
                    segment.text,
                    segment.confidence,
                ],
            )?;
            let rowid = transaction.last_insert_rowid();
            transaction.execute(
                "INSERT INTO transcripts_fts(rowid, text) VALUES (?1, ?2)",
                params![rowid, segment.text],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn transcript_count_for_video(
        &self,
        owner_id: &str,
        video_id: &str,
    ) -> anyhow::Result<usize> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM transcripts WHERE owner_id = ?1 AND video_id = ?2",
            params![owner_id, video_id],
            |row| row.get(0),
        )?;
        usize::try_from(count).context("transcript count was negative")
    }

    pub fn segments_overlapping(
        &self,
        owner_id: &str,
        video_id: &str,
        start_s: f64,
        end_s: f64,
    ) -> anyhow::Result<Vec<TranscriptSegment>> {
        ensure!(end_s > start_s, "query end must exceed start");
        let mut statement = self.connection.prepare(
            "SELECT id, video_id, owner_id, start_s, end_s, text, confidence
             FROM transcripts
             WHERE owner_id = ?1 AND video_id = ?2 AND start_s < ?4 AND end_s > ?3
             ORDER BY start_s, id",
        )?;
        let rows = statement.query_map(
            params![owner_id, video_id, start_s, end_s],
            transcript_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to load overlapping transcript segments")
    }

    pub fn search_transcripts(
        &self,
        owner_id: &str,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<TranscriptSegment>> {
        ensure!(!query.trim().is_empty(), "FTS query must not be empty");
        let mut statement = self.connection.prepare(
            "SELECT t.id, t.video_id, t.owner_id, t.start_s, t.end_s, t.text, t.confidence
             FROM transcripts_fts
             JOIN transcripts AS t ON t.rowid = transcripts_fts.rowid
             WHERE transcripts_fts MATCH ?2 AND t.owner_id = ?1
             ORDER BY bm25(transcripts_fts), t.start_s
             LIMIT ?3",
        )?;
        let rows =
            statement.query_map(params![owner_id, query, limit as i64], transcript_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to search transcripts")
    }

    pub fn search_shot_context(
        &self,
        owner_id: &str,
        shot_id: &str,
    ) -> anyhow::Result<Option<SearchShotContext>> {
        self.connection
            .query_row(
                "SELECT s.id, s.video_id, v.path, s.start_s, s.end_s, s.thumb_rel
                 FROM shots AS s
                 JOIN videos AS v ON v.id = s.video_id AND v.owner_id = s.owner_id
                 WHERE s.owner_id = ?1 AND s.id = ?2",
                params![owner_id, shot_id],
                |row| {
                    Ok(SearchShotContext {
                        shot_id: row.get(0)?,
                        video_id: row.get(1)?,
                        video_path: row.get(2)?,
                        start_s: row.get(3)?,
                        end_s: row.get(4)?,
                        thumb_rel: row.get(5)?,
                    })
                },
            )
            .optional()
            .context("failed to load search shot context")
    }

    pub fn transcript_shot_hits(
        &self,
        owner_id: &str,
        fts_query: &str,
    ) -> anyhow::Result<Vec<TranscriptShotHit>> {
        ensure!(!fts_query.trim().is_empty(), "FTS query must not be empty");
        let mut statement = self.connection.prepare(
            "SELECT s.id, t.text
             FROM transcripts_fts
             JOIN transcripts AS t ON t.rowid = transcripts_fts.rowid
             JOIN shots AS s
               ON s.video_id = t.video_id
              AND s.owner_id = t.owner_id
              AND t.start_s < s.end_s
              AND t.end_s > s.start_s
             WHERE transcripts_fts MATCH ?2 AND t.owner_id = ?1
             ORDER BY bm25(transcripts_fts), t.start_s, s.id",
        )?;
        let rows = statement.query_map(params![owner_id, fts_query], |row| {
            Ok(TranscriptShotHit {
                shot_id: row.get(0)?,
                text: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to join transcript matches to shots")
    }

    pub fn job_start(&self, owner_id: &str, job: &NewJob) -> anyhow::Result<JobRecord> {
        self.connection.execute(
            "INSERT INTO jobs (
                id, owner_id, video_id, stage, status, started_at, debug_dir
             ) VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6)",
            params![
                job.id,
                owner_id,
                job.video_id,
                stage_to_str(job.stage),
                job.started_at.to_rfc3339(),
                job.debug_dir,
            ],
        )?;
        self.job_by_id(owner_id, &job.id)?
            .context("started job could not be read back")
    }

    pub fn job_finish(
        &self,
        owner_id: &str,
        job_id: &str,
        finished_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.complete_job(owner_id, job_id, JobStatus::Done, finished_at, None)
    }

    pub fn job_fail(
        &self,
        owner_id: &str,
        job_id: &str,
        finished_at: DateTime<Utc>,
        error: &str,
    ) -> anyhow::Result<()> {
        ensure!(!error.trim().is_empty(), "failed job must include an error");
        self.complete_job(
            owner_id,
            job_id,
            JobStatus::Failed,
            finished_at,
            Some(error),
        )
    }

    pub fn job_cancel(
        &self,
        owner_id: &str,
        job_id: &str,
        finished_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.complete_job(owner_id, job_id, JobStatus::Cancelled, finished_at, None)
    }

    pub fn jobs(&self, owner_id: &str, filter: &JobFilter) -> anyhow::Result<Vec<JobRecord>> {
        let stage = filter.stage.map(stage_to_str);
        let status = filter.status.map(job_status_to_str);
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, video_id, stage, status, started_at, finished_at, duration_ms,
                    error, debug_dir
             FROM jobs
             WHERE owner_id = ?1
               AND (?2 IS NULL OR video_id = ?2)
               AND (?3 IS NULL OR stage = ?3)
               AND (?4 IS NULL OR status = ?4)
             ORDER BY started_at DESC, id DESC",
        )?;
        let rows = statement.query_map(
            params![owner_id, filter.video_id, stage, status],
            job_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list jobs")
    }

    pub fn fail_running_jobs_as_interrupted(&self, owner_id: &str) -> anyhow::Result<usize> {
        let now = Utc::now();
        let jobs = self.jobs(
            owner_id,
            &JobFilter {
                status: Some(JobStatus::Running),
                ..JobFilter::default()
            },
        )?;
        for job in &jobs {
            self.job_fail(owner_id, &job.id, now, "interrupted")?;
        }
        Ok(jobs.len())
    }

    /// Recover the last completed stage after a process or stage marked the video failed.
    pub fn restore_failed_video_status(
        &self,
        owner_id: &str,
        video_id: &str,
    ) -> anyhow::Result<VideoStatus> {
        let video = self
            .video_by_id(owner_id, video_id)?
            .with_context(|| format!("video {video_id} was not found"))?;
        if video.status != VideoStatus::Failed {
            return Ok(video.status);
        }
        let failed = self.jobs(
            owner_id,
            &JobFilter {
                video_id: Some(video_id.to_owned()),
                status: Some(JobStatus::Failed),
                ..JobFilter::default()
            },
        )?;
        let status = match failed.first().map(|job| job.stage) {
            Some(Stage::Split) | None => VideoStatus::Pending,
            Some(Stage::Embed) => VideoStatus::Split,
            Some(Stage::Transcribe) => VideoStatus::Embedded,
        };
        self.set_video_status(owner_id, video_id, status)?;
        Ok(status)
    }

    pub fn embedding_meta_get(&self, owner_id: &str) -> anyhow::Result<Option<EmbeddingMeta>> {
        self.connection
            .query_row(
                "SELECT owner_id, model_name, model_sha256, dim, preprocess_version
                 FROM embedding_meta
                 WHERE owner_id = ?1",
                params![owner_id],
                |row| {
                    let dim: i64 = row.get(3)?;
                    Ok(EmbeddingMeta {
                        owner_id: row.get(0)?,
                        model_name: row.get(1)?,
                        model_sha256: row.get(2)?,
                        dim: usize::try_from(dim).map_err(|error| conversion_error(3, error))?,
                        preprocess_version: row.get(4)?,
                    })
                },
            )
            .optional()
            .context("failed to read embedding metadata")
    }

    pub fn embedding_meta_set(
        &self,
        owner_id: &str,
        metadata: &EmbeddingMeta,
    ) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &metadata.owner_id, "embedding metadata")?;
        ensure!(metadata.dim > 0, "embedding dimension must be positive");
        ensure!(
            metadata.preprocess_version > 0,
            "preprocess version must be positive"
        );
        self.connection.execute(
            "INSERT INTO embedding_meta (
                owner_id, model_name, model_sha256, dim, preprocess_version
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(owner_id) DO UPDATE SET
                model_name = excluded.model_name,
                model_sha256 = excluded.model_sha256,
                dim = excluded.dim,
                preprocess_version = excluded.preprocess_version",
            params![
                owner_id,
                metadata.model_name,
                metadata.model_sha256,
                metadata.dim as i64,
                metadata.preprocess_version,
            ],
        )?;
        Ok(())
    }

    pub fn delete_video_cascade(&mut self, owner_id: &str, video_id: &str) -> anyhow::Result<bool> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let fts_rows = {
            let mut statement = transaction.prepare(
                "SELECT rowid, text FROM transcripts WHERE owner_id = ?1 AND video_id = ?2",
            )?;
            let rows = statement.query_map(params![owner_id, video_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (rowid, text) in fts_rows {
            transaction.execute(
                "INSERT INTO transcripts_fts(transcripts_fts, rowid, text)
                 VALUES ('delete', ?1, ?2)",
                params![rowid, text],
            )?;
        }
        let changed = transaction.execute(
            "DELETE FROM videos WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, video_id],
        )?;
        transaction.commit()?;
        Ok(changed == 1)
    }

    /// Scan all owners because `doctor --deep` validates the complete local database.
    pub fn integrity(&self) -> anyhow::Result<Vec<Problem>> {
        let mut problems = Vec::new();

        collect_string_pairs(
            &self.connection,
            "SELECT s.id, s.video_id
             FROM shots AS s
             JOIN videos AS v ON v.id = s.video_id AND v.owner_id = s.owner_id
             LEFT JOIN shot_vectors AS sv ON sv.shot_id = s.id AND sv.owner_id = s.owner_id
             WHERE v.status IN ('embedded', 'transcribed', 'done') AND sv.shot_id IS NULL
             ORDER BY s.id",
            |shot_id, video_id| Problem {
                kind: ProblemKind::MissingVector,
                entity_id: shot_id,
                detail: format!("embedded video {video_id} has a shot without a vector"),
            },
            &mut problems,
        )?;

        let mut statement = self
            .connection
            .prepare("SELECT id, thumb_rel FROM shots WHERE thumb_rel IS NOT NULL ORDER BY id")?;
        let thumbs = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for thumb in thumbs {
            let (shot_id, relative) = thumb?;
            let relative_path = Path::new(&relative);
            if !safe_relative_path(relative_path) {
                problems.push(Problem {
                    kind: ProblemKind::UnsafeThumbnailPath,
                    entity_id: shot_id,
                    detail: format!("thumbnail path is not a safe relative path: {relative}"),
                });
            } else {
                let path = self.data_dir.join("thumbs").join(relative_path);
                if !path.is_file() {
                    problems.push(Problem {
                        kind: ProblemKind::MissingThumbnail,
                        entity_id: shot_id,
                        detail: format!("thumbnail does not exist: {}", path.display()),
                    });
                }
            }
        }
        drop(statement);

        collect_string_pairs(
            &self.connection,
            "SELECT sv.shot_id, sv.owner_id
             FROM shot_vectors AS sv
             LEFT JOIN shots AS s ON s.id = sv.shot_id AND s.owner_id = sv.owner_id
             WHERE s.id IS NULL
             ORDER BY sv.shot_id",
            |shot_id, owner_id| Problem {
                kind: ProblemKind::OrphanVector,
                entity_id: shot_id,
                detail: format!("vector for owner {owner_id} has no matching shot"),
            },
            &mut problems,
        )?;

        let mut statement = self.connection.prepare(
            "SELECT shot_id, dim, length(vec)
             FROM shot_vectors
             WHERE length(vec) != dim * 4
             ORDER BY shot_id",
        )?;
        let invalid = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for item in invalid {
            let (shot_id, dim, byte_len) = item?;
            problems.push(Problem {
                kind: ProblemKind::InvalidVectorBytes,
                entity_id: shot_id,
                detail: format!("dim {dim} requires {} bytes, found {byte_len}", dim * 4),
            });
        }

        Ok(problems)
    }

    fn job_by_id(&self, owner_id: &str, job_id: &str) -> anyhow::Result<Option<JobRecord>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, video_id, stage, status, started_at, finished_at,
                        duration_ms, error, debug_dir
                 FROM jobs
                 WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, job_id],
                job_from_row,
            )
            .optional()
            .context("failed to query job by id")
    }

    fn complete_job(
        &self,
        owner_id: &str,
        job_id: &str,
        status: JobStatus,
        finished_at: DateTime<Utc>,
        error: Option<&str>,
    ) -> anyhow::Result<()> {
        let job = self
            .job_by_id(owner_id, job_id)?
            .with_context(|| format!("job {job_id} was not found for owner {owner_id}"))?;
        ensure!(
            job.status == JobStatus::Running,
            "job {job_id} is {:?}, not running",
            job.status
        );
        let duration_ms = finished_at
            .signed_duration_since(job.started_at)
            .num_milliseconds();
        ensure!(duration_ms >= 0, "job finish time precedes its start time");

        let changed = self.connection.execute(
            "UPDATE jobs
             SET status = ?3, finished_at = ?4, duration_ms = ?5, error = ?6
             WHERE owner_id = ?1 AND id = ?2 AND status = 'running'",
            params![
                owner_id,
                job_id,
                job_status_to_str(status),
                finished_at.to_rfc3339(),
                duration_ms,
                error,
            ],
        )?;
        ensure_changed(changed, "running job", job_id)
    }
}

fn configure_connection(connection: &Connection) -> anyhow::Result<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    ensure!(
        journal_mode.eq_ignore_ascii_case("wal"),
        "SQLite refused WAL mode; active mode is {journal_mode}"
    );
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    ensure!(
        foreign_keys == 1,
        "SQLite foreign_keys pragma is not enabled"
    );
    Ok(())
}

fn apply_migrations(connection: &mut Connection) -> anyhow::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            version INTEGER NOT NULL CHECK (version >= 0)
         ) STRICT;
         INSERT OR IGNORE INTO schema_version (singleton, version) VALUES (1, 0);",
    )?;
    let mut current: i64 = connection.query_row(
        "SELECT version FROM schema_version WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        current <= CURRENT_SCHEMA_VERSION,
        "database schema version {current} is newer than supported version {CURRENT_SCHEMA_VERSION}"
    );

    for &(version, migration) in MIGRATIONS {
        if version <= current {
            continue;
        }
        ensure!(
            version == current + 1,
            "migration sequence jumps from version {current} to {version}"
        );
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction
            .execute_batch(migration)
            .with_context(|| format!("failed to apply schema migration {version}"))?;
        transaction.execute(
            "UPDATE schema_version SET version = ?1 WHERE singleton = 1",
            params![version],
        )?;
        transaction.commit()?;
        tracing::info!(schema_version = version, "applied SQLite migration");
        current = version;
    }

    ensure!(
        current == CURRENT_SCHEMA_VERSION,
        "database stopped at schema version {current}; expected {CURRENT_SCHEMA_VERSION}"
    );
    Ok(())
}

fn video_from_row(row: &Row<'_>) -> rusqlite::Result<Video> {
    let status: String = row.get(9)?;
    let indexed_at: Option<String> = row.get(10)?;
    Ok(Video {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        path: row.get(2)?,
        sha256: row.get(3)?,
        duration_s: row.get(4)?,
        fps: row.get(5)?,
        width: row.get(6)?,
        height: row.get(7)?,
        has_audio: row.get(8)?,
        status: video_status_from_str(&status)
            .map_err(|error| conversion_message(9, error.to_string()))?,
        indexed_at: indexed_at
            .map(|value| timestamp_from_str(&value, 10))
            .transpose()?,
    })
}

fn shot_from_row(row: &Row<'_>) -> rusqlite::Result<Shot> {
    Ok(Shot {
        id: row.get(0)?,
        video_id: row.get(1)?,
        owner_id: row.get(2)?,
        idx: row.get(3)?,
        start_s: row.get(4)?,
        end_s: row.get(5)?,
        rep_frame_s: row.get(6)?,
        thumb_rel: row.get(7)?,
        scene_score: row.get(8)?,
    })
}

fn transcript_from_row(row: &Row<'_>) -> rusqlite::Result<TranscriptSegment> {
    Ok(TranscriptSegment {
        id: row.get(0)?,
        video_id: row.get(1)?,
        owner_id: row.get(2)?,
        start_s: row.get(3)?,
        end_s: row.get(4)?,
        text: row.get(5)?,
        confidence: row.get(6)?,
    })
}

fn job_from_row(row: &Row<'_>) -> rusqlite::Result<JobRecord> {
    let stage: String = row.get(3)?;
    let status: String = row.get(4)?;
    let started_at: String = row.get(5)?;
    let finished_at: Option<String> = row.get(6)?;
    Ok(JobRecord {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        video_id: row.get(2)?,
        stage: stage_from_str(&stage).map_err(|error| conversion_message(3, error.to_string()))?,
        status: job_status_from_str(&status)
            .map_err(|error| conversion_message(4, error.to_string()))?,
        started_at: timestamp_from_str(&started_at, 5)?,
        finished_at: finished_at
            .map(|value| timestamp_from_str(&value, 6))
            .transpose()?,
        duration_ms: row.get(7)?,
        error: row.get(8)?,
        debug_dir: row.get(9)?,
    })
}

fn video_status_to_str(status: VideoStatus) -> &'static str {
    match status {
        VideoStatus::Pending => "pending",
        VideoStatus::Split => "split",
        VideoStatus::Embedded => "embedded",
        VideoStatus::Transcribed => "transcribed",
        VideoStatus::Done => "done",
        VideoStatus::Failed => "failed",
    }
}

fn video_status_from_str(value: &str) -> anyhow::Result<VideoStatus> {
    match value {
        "pending" => Ok(VideoStatus::Pending),
        "split" => Ok(VideoStatus::Split),
        "embedded" => Ok(VideoStatus::Embedded),
        "transcribed" => Ok(VideoStatus::Transcribed),
        "done" => Ok(VideoStatus::Done),
        "failed" => Ok(VideoStatus::Failed),
        _ => bail!("unknown video status {value:?}"),
    }
}

fn stage_to_str(stage: Stage) -> &'static str {
    match stage {
        Stage::Split => "split",
        Stage::Embed => "embed",
        Stage::Transcribe => "transcribe",
    }
}

fn stage_from_str(value: &str) -> anyhow::Result<Stage> {
    match value {
        "split" => Ok(Stage::Split),
        "embed" => Ok(Stage::Embed),
        "transcribe" => Ok(Stage::Transcribe),
        _ => bail!("unknown job stage {value:?}"),
    }
}

fn job_status_to_str(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Done => "done",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
    }
}

fn job_status_from_str(value: &str) -> anyhow::Result<JobStatus> {
    match value {
        "queued" => Ok(JobStatus::Queued),
        "running" => Ok(JobStatus::Running),
        "done" => Ok(JobStatus::Done),
        "failed" => Ok(JobStatus::Failed),
        "cancelled" => Ok(JobStatus::Cancelled),
        _ => bail!("unknown job status {value:?}"),
    }
}

fn timestamp_from_str(value: &str, column: usize) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| conversion_error(column, error))
}

fn conversion_error(
    column: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
}

fn conversion_message(column: usize, message: String) -> rusqlite::Error {
    conversion_error(
        column,
        std::io::Error::new(std::io::ErrorKind::InvalidData, message),
    )
}

fn ensure_owner_matches(owner_id: &str, record_owner: &str, kind: &str) -> anyhow::Result<()> {
    ensure!(
        owner_id == record_owner,
        "{kind} owner {record_owner:?} does not match requested owner {owner_id:?}"
    );
    Ok(())
}

fn ensure_changed(changed: usize, kind: &str, id: &str) -> anyhow::Result<()> {
    ensure!(changed == 1, "{kind} {id} was not found");
    Ok(())
}

fn validate_shot(shot: &Shot) -> anyhow::Result<()> {
    ensure!(shot.idx >= 0, "shot index must be non-negative");
    ensure!(shot.start_s >= 0.0, "shot start must be non-negative");
    ensure!(shot.end_s > shot.start_s, "shot end must exceed start");
    ensure!(
        shot.rep_frame_s >= shot.start_s && shot.rep_frame_s <= shot.end_s,
        "representative frame must be within the shot"
    );
    if let Some(relative) = &shot.thumb_rel {
        ensure!(
            safe_relative_path(Path::new(relative)),
            "thumbnail path must be a safe path relative to the thumbs directory"
        );
    }
    Ok(())
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn collect_string_pairs(
    connection: &Connection,
    sql: &str,
    problem: impl Fn(String, String) -> Problem,
    output: &mut Vec<Problem>,
) -> anyhow::Result<()> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (first, second) = row?;
        output.push(problem(first, second));
    }
    Ok(())
}
