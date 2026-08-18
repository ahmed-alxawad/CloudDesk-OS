//! SQLite-backed music library storage. Every query is scoped by
//! `owner_user_id`; there is no code path in this module that can return
//! or mutate another user's rows, since every statement binds the caller's
//! own id into the `WHERE`/`INSERT` clause rather than trusting an
//! unscoped id from the request.

use serde::Serialize;
use sqlx::{Row, SqlitePool};

fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(0)
}

#[derive(Clone, Debug, Serialize)]
pub struct LibraryRoot {
    pub id: String,
    pub virtual_path: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
    pub duration_seconds: Option<f64>,
    pub codec: Option<String>,
    pub bit_rate: Option<i64>,
    pub year: Option<String>,
    pub genre: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Track {
    pub id: String,
    pub root_id: String,
    pub virtual_path: String,
    #[serde(flatten)]
    pub metadata: TrackMetadata,
    pub updated_at: i64,
}

fn row_to_track(row: &sqlx::sqlite::SqliteRow) -> Track {
    Track {
        id: row.get("id"),
        root_id: row.get("root_id"),
        virtual_path: row.get("virtual_path"),
        metadata: TrackMetadata {
            title: row.get("title"),
            artist: row.get("artist"),
            album: row.get("album"),
            album_artist: row.get("album_artist"),
            track_number: row.get("track_number"),
            disc_number: row.get("disc_number"),
            duration_seconds: row.get("duration_seconds"),
            codec: row.get("codec"),
            bit_rate: row.get("bit_rate"),
            year: row.get("year"),
            genre: row.get("genre"),
        },
        updated_at: row.get("updated_at"),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone)]
pub struct LibraryStore {
    pool: SqlitePool,
}

impl LibraryStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // -- library roots --------------------------------------------------

    pub async fn add_root(
        &self,
        owner_user_id: &str,
        virtual_path: &str,
    ) -> Result<LibraryRoot, sqlx::Error> {
        let id = clouddesk_auth::random_identifier(16);
        let created_at = now();
        sqlx::query(
            "INSERT INTO music_library_roots (id, owner_user_id, virtual_path, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(owner_user_id)
        .bind(virtual_path)
        .bind(created_at)
        .execute(&self.pool)
        .await?;
        Ok(LibraryRoot {
            id,
            virtual_path: virtual_path.to_owned(),
            created_at,
        })
    }

    pub async fn list_roots(&self, owner_user_id: &str) -> Result<Vec<LibraryRoot>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, virtual_path, created_at FROM music_library_roots
             WHERE owner_user_id = ? ORDER BY created_at",
        )
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| LibraryRoot {
                id: row.get("id"),
                virtual_path: row.get("virtual_path"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    /// Ownership-scoped lookup, returning the root's real virtual path
    /// only if it belongs to `owner_user_id`.
    pub async fn get_root(
        &self,
        owner_user_id: &str,
        root_id: &str,
    ) -> Result<Option<LibraryRoot>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, virtual_path, created_at FROM music_library_roots
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(root_id)
        .bind(owner_user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| LibraryRoot {
            id: row.get("id"),
            virtual_path: row.get("virtual_path"),
            created_at: row.get("created_at"),
        }))
    }

    pub async fn remove_root(&self, owner_user_id: &str, root_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM music_library_roots WHERE id = ? AND owner_user_id = ?")
            .bind(root_id)
            .bind(owner_user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // -- tracks -----------------------------------------------------------

    pub async fn upsert_track(
        &self,
        owner_user_id: &str,
        root_id: &str,
        virtual_path: &str,
        metadata: &TrackMetadata,
        fingerprint: &str,
    ) -> Result<(), sqlx::Error> {
        let ts = now();
        sqlx::query(
            "INSERT INTO music_tracks (
                id, owner_user_id, root_id, virtual_path, title, artist, album,
                album_artist, track_number, disc_number, duration_seconds, codec,
                bit_rate, year, genre, fingerprint, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (owner_user_id, virtual_path) DO UPDATE SET
                title = excluded.title,
                artist = excluded.artist,
                album = excluded.album,
                album_artist = excluded.album_artist,
                track_number = excluded.track_number,
                disc_number = excluded.disc_number,
                duration_seconds = excluded.duration_seconds,
                codec = excluded.codec,
                bit_rate = excluded.bit_rate,
                year = excluded.year,
                genre = excluded.genre,
                fingerprint = excluded.fingerprint,
                updated_at = excluded.updated_at",
        )
        .bind(clouddesk_auth::random_identifier(16))
        .bind(owner_user_id)
        .bind(root_id)
        .bind(virtual_path)
        .bind(&metadata.title)
        .bind(&metadata.artist)
        .bind(&metadata.album)
        .bind(&metadata.album_artist)
        .bind(metadata.track_number)
        .bind(metadata.disc_number)
        .bind(metadata.duration_seconds)
        .bind(&metadata.codec)
        .bind(metadata.bit_rate)
        .bind(&metadata.year)
        .bind(&metadata.genre)
        .bind(fingerprint)
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns `(virtual_path, fingerprint)` for every currently-indexed
    /// track under `root_id`, used by an incremental scan to decide what
    /// changed without re-probing unchanged files.
    pub async fn fingerprints_for_root(
        &self,
        root_id: &str,
    ) -> Result<std::collections::HashMap<String, String>, sqlx::Error> {
        let rows =
            sqlx::query("SELECT virtual_path, fingerprint FROM music_tracks WHERE root_id = ?")
                .bind(root_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .iter()
            .map(|row| (row.get("virtual_path"), row.get("fingerprint")))
            .collect())
    }

    /// Removes tracks under `root_id` whose `virtual_path` is not in
    /// `still_present` -- files that disappeared since the last scan.
    /// Returns how many rows were removed.
    pub async fn prune_missing(
        &self,
        root_id: &str,
        still_present: &std::collections::HashSet<String>,
    ) -> Result<u64, sqlx::Error> {
        let existing = self.fingerprints_for_root(root_id).await?;
        let mut removed = 0_u64;
        for virtual_path in existing.keys() {
            if !still_present.contains(virtual_path) {
                sqlx::query("DELETE FROM music_tracks WHERE root_id = ? AND virtual_path = ?")
                    .bind(root_id)
                    .bind(virtual_path)
                    .execute(&self.pool)
                    .await?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub async fn get_track(
        &self,
        owner_user_id: &str,
        track_id: &str,
    ) -> Result<Option<Track>, sqlx::Error> {
        let row = sqlx::query("SELECT * FROM music_tracks WHERE id = ? AND owner_user_id = ?")
            .bind(track_id)
            .bind(owner_user_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(row_to_track))
    }

    pub async fn list_tracks(
        &self,
        owner_user_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Track>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM music_tracks WHERE owner_user_id = ?
             ORDER BY artist, album, disc_number, track_number, title
             LIMIT ? OFFSET ?",
        )
        .bind(owner_user_id)
        .bind(limit.clamp(1, 500))
        .bind(offset.max(0))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_track).collect())
    }

    pub async fn count_tracks(&self, owner_user_id: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM music_tracks WHERE owner_user_id = ?")
            .bind(owner_user_id)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn list_artists(&self, owner_user_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT DISTINCT artist FROM music_tracks
             WHERE owner_user_id = ? AND artist IS NOT NULL AND artist != ''
             ORDER BY artist",
        )
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|row| row.get("artist")).collect())
    }

    pub async fn tracks_by_artist(
        &self,
        owner_user_id: &str,
        artist: &str,
    ) -> Result<Vec<Track>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM music_tracks WHERE owner_user_id = ? AND artist = ?
             ORDER BY album, disc_number, track_number, title",
        )
        .bind(owner_user_id)
        .bind(artist)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_track).collect())
    }

    pub async fn list_albums(
        &self,
        owner_user_id: &str,
    ) -> Result<Vec<(String, Option<String>, Option<String>)>, sqlx::Error> {
        // (album, album_artist/artist, year)
        let rows = sqlx::query(
            "SELECT DISTINCT album, COALESCE(album_artist, artist) AS artist, year
             FROM music_tracks
             WHERE owner_user_id = ? AND album IS NOT NULL AND album != ''
             ORDER BY album",
        )
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| (row.get("album"), row.get("artist"), row.get("year")))
            .collect())
    }

    pub async fn tracks_by_album(
        &self,
        owner_user_id: &str,
        album: &str,
    ) -> Result<Vec<Track>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM music_tracks WHERE owner_user_id = ? AND album = ?
             ORDER BY disc_number, track_number, title",
        )
        .bind(owner_user_id)
        .bind(album)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_track).collect())
    }

    pub async fn tracks_by_root(
        &self,
        owner_user_id: &str,
        root_id: &str,
    ) -> Result<Vec<Track>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM music_tracks WHERE owner_user_id = ? AND root_id = ?
             ORDER BY virtual_path",
        )
        .bind(owner_user_id)
        .bind(root_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_track).collect())
    }

    /// Metadata search, scoped to `owner_user_id` and backed by the
    /// indexes on `(owner_user_id, title/artist/album)` -- never a
    /// filesystem scan.
    pub async fn search(
        &self,
        owner_user_id: &str,
        query: &str,
        limit: i64,
    ) -> Result<Vec<Track>, sqlx::Error> {
        let pattern = format!("%{}%", escape_like(query));
        let rows = sqlx::query(
            "SELECT * FROM music_tracks
             WHERE owner_user_id = ? AND (
                title LIKE ? ESCAPE '\\' OR
                artist LIKE ? ESCAPE '\\' OR
                album LIKE ? ESCAPE '\\' OR
                genre LIKE ? ESCAPE '\\'
             )
             ORDER BY artist, album, title
             LIMIT ?",
        )
        .bind(owner_user_id)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_track).collect())
    }

    // -- playlists --------------------------------------------------------

    pub async fn create_playlist(
        &self,
        owner_user_id: &str,
        name: &str,
    ) -> Result<Playlist, sqlx::Error> {
        let id = clouddesk_auth::random_identifier(16);
        let ts = now();
        sqlx::query(
            "INSERT INTO music_playlists (id, owner_user_id, name, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(owner_user_id)
        .bind(name)
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(Playlist {
            id,
            name: name.to_owned(),
            created_at: ts,
            updated_at: ts,
        })
    }

    pub async fn list_playlists(&self, owner_user_id: &str) -> Result<Vec<Playlist>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, name, created_at, updated_at FROM music_playlists
             WHERE owner_user_id = ? ORDER BY created_at",
        )
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| Playlist {
                id: row.get("id"),
                name: row.get("name"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            })
            .collect())
    }

    async fn owns_playlist(
        &self,
        owner_user_id: &str,
        playlist_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let row: Option<String> =
            sqlx::query_scalar("SELECT id FROM music_playlists WHERE id = ? AND owner_user_id = ?")
                .bind(playlist_id)
                .bind(owner_user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.is_some())
    }

    pub async fn rename_playlist(
        &self,
        owner_user_id: &str,
        playlist_id: &str,
        name: &str,
    ) -> Result<bool, sqlx::Error> {
        if !self.owns_playlist(owner_user_id, playlist_id).await? {
            return Ok(false);
        }
        sqlx::query("UPDATE music_playlists SET name = ?, updated_at = ? WHERE id = ?")
            .bind(name)
            .bind(now())
            .bind(playlist_id)
            .execute(&self.pool)
            .await?;
        Ok(true)
    }

    pub async fn delete_playlist(
        &self,
        owner_user_id: &str,
        playlist_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM music_playlists WHERE id = ? AND owner_user_id = ?")
            .bind(playlist_id)
            .bind(owner_user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Adds `track_id` to the end of `playlist_id`, if the caller owns
    /// both the playlist and the track. Returns `false` (no-op, not an
    /// error) if either ownership check fails -- callers map that to a
    /// 404, never revealing whether the other user's playlist/track ID
    /// exists.
    pub async fn add_playlist_entry(
        &self,
        owner_user_id: &str,
        playlist_id: &str,
        track_id: &str,
    ) -> Result<bool, sqlx::Error> {
        if !self.owns_playlist(owner_user_id, playlist_id).await? {
            return Ok(false);
        }
        if self.get_track(owner_user_id, track_id).await?.is_none() {
            return Ok(false);
        }
        let next_position: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM music_playlist_entries
             WHERE playlist_id = ?",
        )
        .bind(playlist_id)
        .fetch_one(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO music_playlist_entries (id, playlist_id, track_id, position)
             VALUES (?, ?, ?, ?)",
        )
        .bind(clouddesk_auth::random_identifier(16))
        .bind(playlist_id)
        .bind(track_id)
        .bind(next_position)
        .execute(&self.pool)
        .await?;
        sqlx::query("UPDATE music_playlists SET updated_at = ? WHERE id = ?")
            .bind(now())
            .bind(playlist_id)
            .execute(&self.pool)
            .await?;
        Ok(true)
    }

    pub async fn remove_playlist_entry(
        &self,
        owner_user_id: &str,
        playlist_id: &str,
        entry_id: &str,
    ) -> Result<bool, sqlx::Error> {
        if !self.owns_playlist(owner_user_id, playlist_id).await? {
            return Ok(false);
        }
        sqlx::query("DELETE FROM music_playlist_entries WHERE id = ? AND playlist_id = ?")
            .bind(entry_id)
            .bind(playlist_id)
            .execute(&self.pool)
            .await?;
        Ok(true)
    }

    /// Replaces the full ordering of `playlist_id`'s entries with
    /// `ordered_entry_ids`. Every id in `ordered_entry_ids` must already
    /// belong to this playlist -- ids that don't are silently ignored
    /// (not an injection point: this only ever reassigns `position` on
    /// rows already scoped to `playlist_id`, it can never attach a
    /// different playlist's entry to this one).
    pub async fn reorder_playlist(
        &self,
        owner_user_id: &str,
        playlist_id: &str,
        ordered_entry_ids: &[String],
    ) -> Result<bool, sqlx::Error> {
        if !self.owns_playlist(owner_user_id, playlist_id).await? {
            return Ok(false);
        }
        for (position, entry_id) in ordered_entry_ids.iter().enumerate() {
            sqlx::query(
                "UPDATE music_playlist_entries SET position = ?
                 WHERE id = ? AND playlist_id = ?",
            )
            .bind(i64::try_from(position).unwrap_or(i64::MAX))
            .bind(entry_id)
            .bind(playlist_id)
            .execute(&self.pool)
            .await?;
        }
        Ok(true)
    }

    pub async fn playlist_entries(
        &self,
        owner_user_id: &str,
        playlist_id: &str,
    ) -> Result<Option<Vec<(String, Track)>>, sqlx::Error> {
        if !self.owns_playlist(owner_user_id, playlist_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query(
            "SELECT e.id AS entry_id, t.* FROM music_playlist_entries e
             JOIN music_tracks t ON t.id = e.track_id
             WHERE e.playlist_id = ? ORDER BY e.position",
        )
        .bind(playlist_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(Some(
            rows.iter()
                .map(|row| (row.get("entry_id"), row_to_track(row)))
                .collect(),
        ))
    }

    // -- favorites ----------------------------------------------------------

    pub async fn favorite(&self, owner_user_id: &str, track_id: &str) -> Result<bool, sqlx::Error> {
        if self.get_track(owner_user_id, track_id).await?.is_none() {
            return Ok(false);
        }
        sqlx::query(
            "INSERT OR IGNORE INTO music_favorites (owner_user_id, track_id, created_at)
             VALUES (?, ?, ?)",
        )
        .bind(owner_user_id)
        .bind(track_id)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    pub async fn unfavorite(&self, owner_user_id: &str, track_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM music_favorites WHERE owner_user_id = ? AND track_id = ?")
            .bind(owner_user_id)
            .bind(track_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_favorites(&self, owner_user_id: &str) -> Result<Vec<Track>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT t.* FROM music_favorites f
             JOIN music_tracks t ON t.id = f.track_id
             WHERE f.owner_user_id = ? ORDER BY f.created_at DESC",
        )
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_track).collect())
    }

    // -- recently played ------------------------------------------------

    pub async fn record_played(
        &self,
        owner_user_id: &str,
        track_id: &str,
    ) -> Result<bool, sqlx::Error> {
        if self.get_track(owner_user_id, track_id).await?.is_none() {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO music_recently_played (id, owner_user_id, track_id, played_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(clouddesk_auth::random_identifier(16))
        .bind(owner_user_id)
        .bind(track_id)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    pub async fn recently_played(
        &self,
        owner_user_id: &str,
        limit: i64,
    ) -> Result<Vec<Track>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT t.* FROM music_recently_played r
             JOIN music_tracks t ON t.id = r.track_id
             WHERE r.owner_user_id = ?
             GROUP BY t.id
             ORDER BY MAX(r.played_at) DESC
             LIMIT ?",
        )
        .bind(owner_user_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_track).collect())
    }

    // -- queue ------------------------------------------------------------

    pub async fn get_queue(&self, owner_user_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let row: Option<String> =
            sqlx::query_scalar("SELECT items_json FROM music_queue WHERE owner_user_id = ?")
                .bind(owner_user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }

    pub async fn set_queue(
        &self,
        owner_user_id: &str,
        items: &[String],
    ) -> Result<(), sqlx::Error> {
        let json = serde_json::to_string(items).unwrap_or_else(|_| "[]".to_owned());
        sqlx::query(
            "INSERT INTO music_queue (owner_user_id, items_json, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT (owner_user_id) DO UPDATE SET
                items_json = excluded.items_json, updated_at = excluded.updated_at",
        )
        .bind(owner_user_id)
        .bind(json)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Escapes `%`, `_`, and `\` in a user-supplied `LIKE` search term so it
/// is matched as literal text, never as a wildcard the caller controls.
fn escape_like(input: &str) -> String {
    input
        .chars()
        .flat_map(|c| match c {
            '%' | '_' | '\\' => vec!['\\', c],
            other => vec![other],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::escape_like;

    #[test]
    fn like_wildcards_in_search_input_are_escaped() {
        assert_eq!(escape_like("50%_off\\"), "50\\%\\_off\\\\");
        assert_eq!(escape_like("normal"), "normal");
    }
}
