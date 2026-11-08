/* importer.rs
 *
 * SPDX-FileCopyrightText: 2023 nate-xyz
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use gtk::{gio, glib::Sender};
use gtk_macros::send;

use std::{
    collections::HashMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    thread,
};
use chrono::{DateTime, Utc};
use log::{debug, error, warn};
use lofty::{
    file::{AudioFile, TaggedFileExt},
    read_from_path,
    tag::{Accessor, ItemKey},
};
use pyo3::prelude::*;
use rayon::prelude::*;
use regex::Regex;
use rusqlite::Transaction;
use walkdir::WalkDir;

use super::database::DatabaseAction;
use super::util;

#[derive(Debug, FromPyObject, Clone)]
pub enum MapVal {
    Str(String),
    List(Vec<String>),
    Float(f32),
    Int(i32),
    Bytes(Vec<u8>),
}

#[derive(Debug)]
pub struct Importer {
    pub settings: gio::Settings,
}

type TrackTags = HashMap<String, HashMap<String, HashMap<String, MapVal>>>;
type CoverArt = HashMap<String, Vec<u8>>;

const RUST_METADATA_EXTENSIONS: &[&str] = &[
    "mp3", "aif", "aiff", "ogg", "opus", "flac", "mp4", "m4a",
    "aac", "ape", "mpc", "wav", "wv",
];
const PYTHON_FALLBACK_EXTENSIONS: &[&str] = &["asf", "wma", "wmv"];
const IMPORT_BATCH_SIZE: usize = 96;

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
}

fn is_hidden(path: &Path) -> bool {
    path.components().any(|component| match component {
        std::path::Component::Normal(name) => name.to_string_lossy().starts_with('.'),
        _ => false,
    })
}

fn list_audio_files(folder: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut rust_files = Vec::new();
    let mut legacy_files = Vec::new();

    for entry in WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        // Prune hidden directories before WalkDir descends into them.
        .filter_entry(|entry| {
            entry.depth() == 0
                || entry
                    .path()
                    .strip_prefix(folder)
                    .map(|relative_path| !is_hidden(relative_path))
                    .unwrap_or(true)
        })
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                warn!("Unable to read a music-library entry: {}", err);
                continue;
            }
        };

        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }

        match extension(path).as_deref() {
            Some(ext) if RUST_METADATA_EXTENSIONS.contains(&ext) => rust_files.push(path.to_path_buf()),
            Some(ext) if PYTHON_FALLBACK_EXTENSIONS.contains(&ext) => legacy_files.push(path.to_path_buf()),
            _ => {}
        }
    }

    rust_files.sort();
    legacy_files.sort();
    (rust_files, legacy_files)
}

fn insert_list(target: &mut HashMap<String, MapVal>, key: &str, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        target.insert(key.to_owned(), MapVal::List(vec![value]));
    }
}

fn extract_with_lofty(path: &Path) -> Option<(String, HashMap<String, HashMap<String, MapVal>>, Option<Vec<u8>>)> {
    let tagged_file = match read_from_path(path) {
        Ok(file) => file,
        Err(err) => {
            debug!("Skipping unreadable audio file {}: {}", path.display(), err);
            return None;
        }
    };

    let tag = tagged_file.primary_tag().or_else(|| tagged_file.first_tag());
    let fallback_title = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unknown Track".to_owned());

    let (title, artist, album_artist, album, genre, date, track_number, disc_number, picture) =
        if let Some(tag) = tag {
            let artist = tag.artist().map(|value| value.into_owned());
            let album_artist = tag
                .get_string(ItemKey::AlbumArtist)
                .map(str::to_owned)
                .or_else(|| artist.clone())
                .unwrap_or_else(|| "Unknown Artist".to_owned());

            (
                tag.title().map(|value| value.into_owned()).unwrap_or(fallback_title),
                artist.unwrap_or_default(),
                album_artist,
                tag.album().map(|value| value.into_owned()).unwrap_or_else(|| "Unknown Album".to_owned()),
                tag.genre().map(|value| value.into_owned()),
                tag.date().map(|value| value.to_string()),
                tag.track(),
                tag.disk(),
                tag.pictures().first().map(|picture| picture.data().to_vec()),
            )
        } else {
            (
                fallback_title,
                String::new(),
                "Unknown Artist".to_owned(),
                "Unknown Album".to_owned(),
                None,
                None,
                None,
                None,
                None,
            )
        };

    let mut strings = HashMap::new();
    insert_list(&mut strings, "title", Some(title));
    insert_list(&mut strings, "artist", Some(artist));
    insert_list(&mut strings, "albumartist", Some(album_artist));
    insert_list(&mut strings, "album", Some(album));
    insert_list(&mut strings, "genre", genre);
    insert_list(&mut strings, "date", date);
    insert_list(&mut strings, "tracknumber", track_number.map(|number| number.to_string()));
    insert_list(&mut strings, "discnumber", disc_number.map(|number| number.to_string()));

    let mut string_values = HashMap::new();
    let file_type = extension(path).map(|value| format!(".{value}")).unwrap_or_default();
    string_values.insert("filetype_".to_owned(), MapVal::Str(file_type));

    let mut float_values = HashMap::new();
    float_values.insert(
        "duration".to_owned(),
        MapVal::Float(tagged_file.properties().duration().as_secs_f32()),
    );

    let mut grouped = HashMap::new();
    grouped.insert("str_list".to_owned(), strings);
    grouped.insert("str".to_owned(), string_values);
    grouped.insert("float".to_owned(), float_values);

    Some((path.to_string_lossy().into_owned(), grouped, picture))
}

fn merge_legacy_python(paths: &[PathBuf]) -> Result<(TrackTags, CoverArt), Box<dyn Error>> {
    if paths.is_empty() {
        return Ok((HashMap::new(), HashMap::new()));
    }

    let path_strings = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    let result = Python::with_gil(|py| -> PyResult<(TrackTags, CoverArt)> {
        let code_main = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/python/main.py"));
        let code_extracting = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/python/extracting.py"));
        let code_importer = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/python/importer.py"));
        let code_translate_dicts = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/python/translate_dicts.py"));
        let code_util = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/python/util.py"));

        PyModule::from_code(py, code_util, "util", "util")?;
        PyModule::from_code(py, code_translate_dicts, "translate_dicts", "translate_dicts")?;
        PyModule::from_code(py, code_extracting, "extracting", "extracting")?;
        PyModule::from_code(py, code_importer, "importer", "importer")?;

        let module = PyModule::from_code(py, code_main, "main", "main")?;
        let object: Py<PyAny> = module
            .getattr("tags_and_cover_art_from_files")?
            .call1((path_strings,))?
            .into();
        let all = object.extract(py)?;
        Ok(all)
    });

    result.map_err(|err| Box::new(err) as Box<dyn Error>)
}

impl Importer {
    pub fn new() -> Self {
        Self {
            settings: util::settings_manager(),
        }
    }

    pub fn extract_folder(&self, folder_path: String, sender: Sender<DatabaseAction>) {
        thread::spawn(move || {
            send!(
                sender,
                DatabaseAction::ImportProgress("Finding audio files…".to_owned())
            );

            let (rust_files, legacy_files) = list_audio_files(Path::new(&folder_path));
            let total = rust_files.len() + legacy_files.len();
            if total == 0 {
                send!(
                    sender,
                    DatabaseAction::ImportFailed(format!("No supported audio files were found in {folder_path}"))
                );
                return;
            }

            let worker_count = thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(4)
                .clamp(1, 8);
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(worker_count)
                .build();

            let mut tags: TrackTags = HashMap::with_capacity(total);
            let mut cover_art: CoverArt = HashMap::new();
            let mut processed = 0usize;

            for chunk in rust_files.chunks(IMPORT_BATCH_SIZE) {
                let parsed = match &pool {
                    Ok(pool) => pool.install(|| {
                        chunk.par_iter().filter_map(|path| extract_with_lofty(path)).collect::<Vec<_>>()
                    }),
                    Err(_) => chunk.iter().filter_map(|path| extract_with_lofty(path)).collect::<Vec<_>>(),
                };

                for (uri, metadata, picture) in parsed {
                    tags.insert(uri.clone(), metadata);
                    if let Some(picture) = picture {
                        cover_art.insert(uri, picture);
                    }
                }

                processed += chunk.len();
                let percentage = processed.saturating_mul(100) / total;
                send!(
                    sender,
                    DatabaseAction::ImportProgress(format!(
                        "Reading metadata: {processed}/{total} ({percentage}%)"
                    ))
                );
            }

            if !legacy_files.is_empty() {
                send!(
                    sender,
                    DatabaseAction::ImportProgress(format!(
                        "Reading metadata for {} legacy ASF/WMA files…",
                        legacy_files.len()
                    ))
                );

                match merge_legacy_python(&legacy_files) {
                    Ok((legacy_tags, legacy_art)) => {
                        tags.extend(legacy_tags);
                        cover_art.extend(legacy_art);
                    }
                    Err(err) => {
                        error!("Legacy metadata fallback failed: {}", err);
                    }
                }
            }

            if tags.is_empty() {
                send!(
                    sender,
                    DatabaseAction::ImportFailed(format!(
                        "No readable audio metadata was found in {folder_path}"
                    ))
                );
                return;
            }

            debug!(
                "Extracted metadata for {} tracks ({} artwork files) from {}",
                tags.len(),
                cover_art.len(),
                folder_path
            );

            send!(
                sender,
                DatabaseAction::ConstructFromTags((folder_path, tags, cover_art))
            );
        });
    }

    pub fn build_database_from_tags(&self, tx: &Transaction, folder_uri: String, tags: HashMap<String, HashMap<String, HashMap<String, MapVal>>>, cover_art_map: HashMap<String, Vec<u8>>) -> Result<(), Box<dyn Error>> {
        debug!("Building database from tags");
        let window = util::window();
        let database = util::database();

        if let Some(w) = window {
            w.set_import_message("Constructing database from tags.");
    
        }


        let mut added_genres: HashMap<String, i64> = HashMap::new();
        let mut added_artists: HashMap<String, i64> = HashMap::new();
        let mut added_albums: HashMap<String, i64> = HashMap::new();
        let mut added_tracks: HashMap<String, i64> = HashMap::new();
        let mut added_art: HashMap<&[u8], i64> = HashMap::new();
        let mut album_art_counts: HashMap<i64, HashMap<i64, usize>> = HashMap::new();
        let mut album_art_selection: HashMap<i64, Option<(i64, usize)>> = HashMap::new();

        let mut orphan_tracks = 0;
        // This count is constant during this import; avoid querying it once per orphan track.
        let existing_orphan_tracks = database.query_orphan_tracks(&tx)? as u32;
        let current_folder_id = database.add_folder(&tx, folder_uri)?;

        let re = Regex::new(r"^[^\d]*(\d+)").unwrap();

        for (uri, track_map) in &tags {
            debug!("getting tags from:\n\t -> {:?}", uri);

            let mut title_tag = String::new();
            let mut artist_tag = String::new();
            let mut album_tag = String::new();
            let mut albumartist_tag = String::new();
            let mut date_tag = String::new();
            let mut genre_tag = String::new();
            let mut cover_art = None;
            let mut duration_tag = None;
            let mut disc_number_tag = None;
            let mut track_number_tag = None;
            let mut filetype_tag = String::new();

            cover_art = cover_art_map.get(uri);
            
            for (type_key, type_map) in track_map {

                match type_key.as_str() {
                    "str_list" => {
                        for (tag, tag_val) in type_map {
                            if let MapVal::List(value) = tag_val {
                                //debug!("\t\ttagging {} {:?} ", tag, value);

                                match tag.as_str() {
                                    "genre" => {
                                        genre_tag = value[0].clone();
                                    },
                                    "album" => {
                                        album_tag = value[0].clone();
                                    },
                                    "title" => {
                                        title_tag = value[0].clone();
                                    },
                                    "albumartist" => {
                                        albumartist_tag = value[0].clone();
                                    },
                                    "artist" => {
                                        artist_tag = value[0].clone();
                                    },
                                    "date" => {
                                        date_tag = value[0].clone();
                                    },
                                    "tracknumber" => {
                                        let tag = re.captures(&value[0]).and_then(|cap| {
                                            cap.get(0).map(|s| s.as_str())
                                        });
                                        track_number_tag = if !tag.is_none() {
                                            match tag.unwrap().parse::<u32>() {
                                                Ok(num) => Some(num),
                                                Err(e) => {
                                                    error!("{}: {}", tag.unwrap(), e);
                                                    None
                                                },
                                            }
                                        } else {
                                            None
                                        };
                                    },
                                    "discnumber" => {
                                        let tag = re.captures(&value[0]).and_then(|cap| {
                                            cap.get(0).map(|s| s.as_str())
                                        });
                                        disc_number_tag = if !tag.is_none() {
                                            match tag.unwrap().parse::<u32>() {
                                                Ok(num) => Some(num),
                                                Err(e) => {
                                                    error!("{}: {}", tag.unwrap(), e);
                                                    None
                                                },
                                            }
                                        } else {
                                            None
                                        };
                                    },
                                    _ => (),
                                }
                            }
                        }
                    }
                    "str" => {
                        for (tag, tag_val) in type_map {
                            if let MapVal::Str(value) = tag_val {
                                match tag.as_str() {
                                    "genre" => {
                                        genre_tag = value.clone();
                                    }
                                    "album" => {
                                        album_tag = value.clone();
                                    }
                                    "title" => {
                                        title_tag = value.clone();
                                    }
                                    "albumartist" => {
                                        albumartist_tag = value.clone();
                                    }
                                    "artist" => {
                                        artist_tag = value.clone();
                                    }
                                    "filetype_" => {
                                        filetype_tag = value.clone();
                                    },
                                    "tracknumber" => {
                                        let tag = re.captures(&value).and_then(|cap| {
                                            cap.get(0).map(|s| s.as_str())
                                        });
                                        debug!("{:?}", tag);
                                        track_number_tag = if !tag.is_none() {
                                            
                                            match tag.unwrap().parse::<u32>() {
                                                Ok(num) => Some(num),
                                                Err(e) => {
                                                    error!("{}: {}", tag.unwrap(), e);
                                                    None
                                                },
                                            }
                                        } else {
                                            None
                                        };
                                    },
                                    "discnumber" => {
                                        let tag = re.captures(&value).and_then(|cap| {
                                            cap.get(0).map(|s| s.as_str())
                                        });
                                        disc_number_tag = if !tag.is_none() {
                                            match tag.unwrap().parse::<u32>() {
                                                Ok(num) => Some(num),
                                                Err(e) => {
                                                    error!("{}: {}", tag.unwrap(), e);
                                                    None
                                                },
                                            }
                                        } else {
                                            None
                                        };
                                    },
                                    _ => (),
                                }
                            }
                        }
                    }
                    "float" => {
                        for (tag, tag_val) in type_map {
                            if let MapVal::Float(value) = tag_val {
                                match tag.as_str() {
                                    "duration" => {
                                        duration_tag = Some(value);
                                    }
                                    _ => (),
                                }
                            }
                        }
                    }
                    _ => error!("unknown key type"),
                }
            }

            let album_key = format!("{}{}", album_tag, albumartist_tag);
            let song_key = format!("{}{}{}{}", title_tag, album_tag, albumartist_tag, uri);

            if added_tracks.contains_key(&song_key) {
                //debug!("{} already added.", title_tag);
                continue;
            }

            let discnumber = match disc_number_tag {
                Some(number) => number,
                None => 1,
            };

            let tracknumber = match track_number_tag {
                Some(number) => number,
                None => {
                    if album_key == "Unknown AlbumUnknown Artist" {
                        orphan_tracks += 1;
                        orphan_tracks + existing_orphan_tracks
                    } else {
                        error!("No track number for {}", song_key);
                        1
                    }
                },
            };
   
            // add genre to the database table
            let genre_id = if !genre_tag.is_empty() {
                if !added_genres.contains_key(&genre_tag) {
                    Some(database.add_genre(&tx, genre_tag.clone())?)
                } else {
                    //debug!("Already added genre {}, retrieving from map.", genre_tag);
                    Some(added_genres[&genre_tag])
                }
            } else {
                None
            };

            // add artists to the database table
            let artist_id = if !albumartist_tag.is_empty() {
                if !added_artists.contains_key(&albumartist_tag) {
                    database.add_artist(&tx, albumartist_tag.clone())?
                } else {
                    //debug!("Already added artist {}, retrieving from map.", albumartist_tag);
                    added_artists[&albumartist_tag]
                }
            } else {
                error!("No artist found for track {}", title_tag);
                continue;
            };

            // add cover art to the database table
            let cover_art_id = if let Some(bytes) = cover_art {
                let bytes = bytes.as_slice();
                if added_art.contains_key(&bytes) {
                    //debug!("Already added cover_art, retrieving from map.");
                    added_art.get(&bytes).cloned()
                } else {
                    Some(database.add_cover_art(&tx, bytes)?)
                }
            } else {
                None
            };
            
            // add albums to the database table
            let album_id = if !added_albums.contains_key(&album_key) {
                database.add_album_full(
                    &tx,
                    album_tag.clone(),
                    artist_tag,
                    date_tag.clone(),
                    genre_tag.clone(),
                    cover_art_id,
                    artist_id,
                    genre_id,
                )?
            } else {
                //debug!("Already added {}, retrieving from map.", album_key);
                added_albums[&album_key]
            };
        
            let duration = match duration_tag {
                Some(d) => *d,
                None => {
                    error!("No track duration for {}", song_key);
                    continue;
                },
            };

            

            let metadata = match fs::metadata(uri) {
                Ok(metadata) => metadata,
                Err(err) => {
                    error!("Skipping {} because file metadata could not be read: {}", uri, err);
                    continue;
                }
            };
            let modification_time = metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let modification_time_dt = DateTime::<Utc>::from(modification_time);

            // if let Some(w) = window {
            //     w.set_import_message(&format!("Adding: {}", uri.clone()));
            // }

            let track_id =  database.add_track_full(
                &tx,
                title_tag,
                filetype_tag,
                album_tag,
                albumartist_tag.clone(),
                date_tag,
                duration,
                tracknumber,
                discnumber,
                album_id,
                artist_id,
                cover_art_id,
                uri.to_string(),
                modification_time_dt,
                current_folder_id,
            )?;


            if !genre_tag.is_empty() && !genre_id.is_none(){
                added_genres.insert(genre_tag, genre_id.unwrap());
            }
            added_artists.insert(albumartist_tag, artist_id);


            if let Some(data) = cover_art {
                if let Some(art_id) = cover_art_id {
                    added_art.insert(data, art_id);

                    let frequencies = album_art_counts.entry(album_id).or_default();
                    let selection = album_art_selection.entry(album_id).or_insert(None);
                    if record_album_art_frequency(frequencies, selection, art_id) {
                        if let Some((selected_art_id, _)) = *selection {
                            database.update_album_art(&tx, album_id, selected_art_id)?;
                        }
                    }
                }
            }

            added_albums.insert(album_key, album_id);
            added_tracks.insert(song_key, track_id);

        }

        Ok(())
    }
}

// Update the selected album artwork without cloning and rescanning the album's
// entire artwork history for every imported track. Ties keep the first selected image.
fn record_album_art_frequency(
    frequencies: &mut HashMap<i64, usize>,
    selection: &mut Option<(i64, usize)>,
    art_id: i64,
) -> bool {
    let count = {
        let count = frequencies.entry(art_id).or_insert(0);
        *count += 1;
        *count
    };

    match *selection {
        None => {
            *selection = Some((art_id, count));
            true
        }
        Some((selected_id, _)) if selected_id == art_id => {
            *selection = Some((selected_id, count));
            false
        }
        Some((_, selected_count)) if count > selected_count => {
            *selection = Some((art_id, count));
            true
        }
        _ => false,
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_reader_accepts_common_lossless_and_lossy_formats() {
        for file in ["track.MP3", "album.flac", "disc.aiff", "track.ogg", "track.opus", "song.m4a", "song.wav", "song.ape", "song.mpc", "song.wv"] {
            let ext = extension(Path::new(file)).unwrap();
            assert!(RUST_METADATA_EXTENSIONS.contains(&ext.as_str()), "unexpected native format: {file}");
        }
    }

    #[test]
    fn legacy_reader_is_limited_to_windows_media_formats() {
        for file in ["song.asf", "song.wma", "song.wmv"] {
            let ext = extension(Path::new(file)).unwrap();
            assert!(PYTHON_FALLBACK_EXTENSIONS.contains(&ext.as_str()), "unexpected fallback format: {file}");
        }
        assert!(!PYTHON_FALLBACK_EXTENSIONS.contains(&"mp3"));
    }

    #[test]
    fn album_art_frequency_updates_incrementally_and_keeps_stable_ties() {
        let mut frequencies = HashMap::new();
        let mut selection = None;

        for id in [4, 5, 4, 6, 4, 5] {
            record_album_art_frequency(&mut frequencies, &mut selection, id);
        }

        assert_eq!(selection, Some((4, 3)));

        // A tie does not displace the artwork that reached the count first.
        assert!(!record_album_art_frequency(&mut frequencies, &mut selection, 5));
        assert_eq!(selection, Some((4, 3)));

        // The challenger is selected only after it exceeds the current count.
        assert!(record_album_art_frequency(&mut frequencies, &mut selection, 5));
        assert_eq!(selection, Some((5, 4)));
    }

    #[test]
    fn hidden_ancestor_directories_are_detected() {
        assert!(is_hidden(Path::new("/music/.private/song.flac")));
        assert!(is_hidden(Path::new("/music/.Trash/album/track.flac")));
    }

    #[test]
    fn visible_paths_are_not_hidden() {
        assert!(!is_hidden(Path::new("/music/Albums/track.flac")));
        assert!(!is_hidden(Path::new(".")));
    }
}
