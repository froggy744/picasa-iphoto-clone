pub const HOME_PREVIEW_LIMIT: i64 = 6;

pub struct LibraryHomeData {
    pub added: Vec<Photo>,
    pub edited: Vec<Photo>,
    pub favorites: Vec<Photo>,
    pub albums: Vec<(Album, Option<Photo>)>,
}

/// Bounded, database-only queries. No source availability probes or scans.
pub fn library_home_data(connection: &Connection) -> Result<LibraryHomeData> {
    let added = {
        let mut query = connection.prepare(
            "SELECT p.id,p.path,p.folder_id,p.taken_at,p.camera,p.width,p.height,p.size_bytes,p.mtime,p.added_at,p.rotation,p.edit_recipe,p.favorite,p.trashed,f.path
             FROM photos p LEFT JOIN folders f ON f.id=p.folder_id
             WHERE p.trashed=0 ORDER BY p.added_at DESC,p.id DESC LIMIT ?1",
        )?;
        let rows = query.query_map([HOME_PREVIEW_LIMIT], photo_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut query = connection.prepare(
        "WITH preview AS (SELECT * FROM albums ORDER BY name COLLATE NOCASE,id LIMIT ?1)
         SELECT a.id,a.name,a.created_at,COUNT(p.id),a.cover_frame,a.cover_photo_id
         FROM preview a LEFT JOIN album_photos ap ON ap.album_id=a.id
         LEFT JOIN photos p ON p.id=ap.photo_id AND p.trashed=0
         GROUP BY a.id ORDER BY a.name COLLATE NOCASE,a.id",
    )?;
    let albums = query
        .query_map([HOME_PREVIEW_LIMIT], |row| {
            Ok(Album {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                photo_count: row.get(3)?,
                cover_frame: row.get(4)?,
                cover_photo_id: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let albums =
        albums
            .into_iter()
            .map(|album| -> Result<_> {
                let chosen = album
                    .cover_photo_id
                    .map(|id| photo(connection, id))
                    .transpose()?
                    .flatten()
                    .filter(|photo| !photo.trashed);
                let cover =
                    match chosen {
                        Some(photo) => Some(photo),
                        None => {
                            let id = connection.query_row(
                    "SELECT p.id FROM album_photos ap JOIN photos p ON p.id=ap.photo_id
                     WHERE ap.album_id=?1 AND p.trashed=0
                     ORDER BY p.taken_at IS NULL,p.taken_at DESC,p.path COLLATE NOCASE LIMIT 1",
                    [album.id], |row| row.get::<_,i64>(0),
                ).optional()?;
                            id.map(|id| photo(connection, id)).transpose()?.flatten()
                        }
                    };
                Ok((album, cover))
            })
            .collect::<Result<Vec<_>>>()?;
    Ok(LibraryHomeData {
        added,
        edited: history_photos_limited(connection, HOME_PREVIEW_LIMIT)?,
        favorites: photos_limited(connection, None, true, None, HOME_PREVIEW_LIMIT)?,
        albums,
    })
}

#[cfg(test)]
mod home_tests {
    use super::*;

    #[test]
    fn home_previews_are_bounded_ordered_and_follow_favorite_changes() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        for id in 1..=100 {
            c.execute(
                "INSERT INTO photos(id,path,added_at,favorite,taken_at) VALUES (?1,?2,?1,1,?3)",
                params![
                    id,
                    format!("smb://offline/{id:03}.jpg"),
                    format!("2026-01-{:02}", id % 28 + 1)
                ],
            )
            .unwrap();
            set_edit_recipe(&c, id, "edited").unwrap();
        }
        c.execute("UPDATE photos SET trashed=1 WHERE id=100", [])
            .unwrap();
        let data = library_home_data(&c).unwrap();
        assert_eq!(
            data.added.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![99, 98, 97, 96, 95, 94]
        );
        assert_eq!(
            data.edited.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![99, 98, 97, 96, 95, 94]
        );
        let expected = photos(&c, None, true, None)
            .unwrap()
            .into_iter()
            .take(6)
            .map(|p| p.id)
            .collect::<Vec<_>>();
        assert_eq!(
            data.favorites.iter().map(|p| p.id).collect::<Vec<_>>(),
            expected
        );
        let removed = data.favorites[0].id;
        set_favorite(&c, removed, false).unwrap();
        let refreshed = library_home_data(&c).unwrap();
        assert_eq!(refreshed.favorites.len(), 6);
        assert!(!refreshed.favorites.iter().any(|p| p.id == removed));
        assert_eq!(
            c.query_row("SELECT count(*) FROM editing_events", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            100
        );
    }

    #[test]
    fn home_album_previews_keep_covers_counts_and_empty_albums() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c.execute("INSERT INTO photos(id,path,trashed) VALUES (1,'/one.jpg',0),(2,'/two.jpg',0),(3,'/trash.jpg',1)",[]).unwrap();
        for index in 0..10 {
            create_album(&c, &format!("Album {index}")).unwrap();
        }
        add_photos_to_album(&c, 1, &[1, 2, 3]).unwrap();
        set_album_cover_photo(&c, 1, 2).unwrap();
        let data = library_home_data(&c).unwrap();
        assert_eq!(data.albums.len(), 6);
        assert_eq!(data.albums[0].0.photo_count, 2);
        assert_eq!(data.albums[0].1.as_ref().unwrap().id, 2);
        assert_eq!(data.albums[1].0.photo_count, 0);
        assert!(data.albums[1].1.is_none());
    }
}
