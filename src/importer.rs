/* importer.rs
 *
 * SPDX-FileCopyrightText: 2023 nate-xyz
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use gtk::{gio, glib::Sender};
#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
use gtk_macros::send;

#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
use std::cell::RefCell;
use std::{collections::HashMap, error::Error, fs};
#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
use std::fmt;
use log::{debug, error};
use rusqlite::Transaction;
#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
use pyo3::prelude::*;
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use regex::Regex;

static TRACK_NUMBER_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^[^\d]*(\d+)").expect("valid track/disc number regex")
});

fn parse_number_tag(value: &str) -> Option<u32> {
    TRACK_NUMBER_REGEX
        .captures(value)
        .and_then(|captures| captures.get(1))
        .and_then(|digits| digits.as_str().parse::<u32>().ok())
}

#[cfg(test)]
mod metadata_number_tests {
    use super::parse_number_tag;

    #[test]
    fn parses_plain_and_slash_separated_numbers() {
        assert_eq!(parse_number_tag("7"), Some(7));
        assert_eq!(parse_number_tag("07/12"), Some(7));
    }

    #[test]
    fn parses_numbers_with_text_prefixes() {
        assert_eq!(parse_number_tag("Track 07/12"), Some(7));
        assert_eq!(parse_number_tag("Disc 2"), Some(2));
    }

    #[test]
    fn ignores_missing_or_invalid_numbers() {
        assert_eq!(parse_number_tag(""), None);
        assert_eq!(parse_number_tag("no number"), None);
    }
}

use super::database::{Database, DatabaseAction};
use super::util;
#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
use std::time::{Duration, Instant};

#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
#[derive(Debug)]
struct ImportError(String);
#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Import error: {}", self.0)
        
    }
}
#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
impl Error for ImportError {}

#[derive(Debug, Clone)]
#[cfg_attr(all(feature = "python-importer", not(feature = "native-importer")), derive(FromPyObject))]
pub enum MapVal {
    Str(String),
    List(Vec<String>),
    Float(f32),
    Int(i32),
    Bytes(Vec<u8>),
}

#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
#[pyclass]
struct LoggingStdout {
    last_update_time: RefCell<Option<Instant>>,
    sender: Sender<DatabaseAction>,
}

#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
impl LoggingStdout {
    // Construct this Python-backed object from Rust so the GLib sender never
    // crosses the Python constructor boundary.
    fn new(sender: Sender<DatabaseAction>) -> Self {
        Self {
            last_update_time: RefCell::new(None),
            sender,
        }
    }
}

#[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
#[pymethods]
impl LoggingStdout {
    fn write(&self, data: &str) {
        if data.contains("ERROR") || data.contains("Exception") {
            error!("{}", data);
        } else if data.contains("DEBUG") {
            debug!("{}", data);
        } else if data.contains('%') {
            let should_update = match self.last_update_time.borrow().as_ref() {
                Some(last_update) => last_update.elapsed() > Duration::from_secs(3),
                None => true,
            };

            if should_update {
                let _ = self.sender.send(DatabaseAction::SetImportPercentage(data.to_string()));
                self.last_update_time.replace(Some(Instant::now()));
            }
        }
    }
}
#[derive(Debug)]
pub struct Importer {
    pub settings: gio::Settings,
}

impl Importer {
    pub fn new() -> Self {
        Self {
            settings: util::settings_manager(),
        }
    }

    #[cfg(feature = "native-importer")]
    pub fn extract_folder(&self, folder_path: String, sender: Sender<DatabaseAction>) {
        std::thread::spawn(move || {
            let _ = sender.send(DatabaseAction::SetImportMessage(
                "Extracting tags from music files.".to_string()
            ));

            match crate::native_metadata::scan_folder(
                std::path::Path::new(&folder_path),
                sender.clone(),
            ) {
                Ok((tags, cover_art)) => {
                    let _ = sender.send(DatabaseAction::ConstructFromTags((
                        folder_path,
                        tags,
                        cover_art,
                    )));
                }
                Err(error) => {
                    error!("Native metadata extraction failed: {}", error);
                    let _ = sender.send(DatabaseAction::ImportFailed(error));
                }
            }
        });
    }

    // Transitional fallback. The default build uses the Rust-native scanner;
    // the old PyO3/Mutagen importer remains available only with --no-default-features.
    #[cfg(all(feature = "python-importer", not(feature = "native-importer")))]
    pub fn extract_folder(&self, folder_path: String, sender: Sender<DatabaseAction>) {
        std::thread::spawn(move || {
            let _ = sender.send(DatabaseAction::SetImportMessage(
                "Extracting tags from music files.".to_string()
            ));

            let from_python = Python::with_gil(|py| -> Result<(), Box<dyn Error>> {
                let sys = py.import("sys")?;
                sys.setattr("stderr", LoggingStdout::new(sender.clone()).into_py(py))?;
                py.run("print('importing')", None, None)?;

                let code_main = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/python/main.py"));
                let code_extracting = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/python/extracting.py"));
                let code_importer = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/python/importer.py"));
                let code_translate_dicts = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/python/translate_dicts.py"));
                let code_util = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/python/util.py"));

                PyModule::from_code(py, code_util, "util", "util")?;
                PyModule::from_code(py, code_translate_dicts, "translate_dicts", "translate_dicts")?;
                PyModule::from_code(py, code_extracting, "extracting", "extracting")?;
                PyModule::from_code(py, code_importer, "importer", "importer")?;

                let module = PyModule::from_code(py, code_main, "main", "")?;
                let object: Py<PyAny> = module
                    .getattr("tags_and_cover_art")?
                    .call1((&folder_path,))?
                    .into();

                let all: (
                    HashMap<String, HashMap<String, HashMap<String, MapVal>>>,
                    HashMap<String, &[u8]>,
                ) = object.extract(py)?;
                let (tags, bytes_map) = all;

                if tags.is_empty() {
                    return Err(Box::new(ImportError("Tag Map Empty".into())));
                }

                let mut vec_byte_map = HashMap::with_capacity(bytes_map.len());
                for (uri, bytes) in bytes_map.iter() {
                    vec_byte_map.insert(uri.clone(), bytes.to_vec());
                }

                send!(sender, DatabaseAction::ConstructFromTags((
                    folder_path,
                    tags,
                    vec_byte_map,
                )));
                Ok(())
            });

            if let Err(err) = from_python {
                error!("Music metadata extraction failed: {}", err);
                let _ = sender.send(DatabaseAction::ImportFailed(err.to_string()));
            }
        });
    }

    pub fn build_database_from_tags(
        tx: &Transaction,
        folder_uri: String,
        tags: HashMap<String, HashMap<String, HashMap<String, MapVal>>>,
        cover_art_map: HashMap<String, Vec<u8>>,
        sender: Sender<DatabaseAction>,
    ) -> Result<(), Box<dyn Error>> {
        debug!("Building database from tags");
        let _ = sender.send(DatabaseAction::SetImportMessage(
            "Constructing database from tags.".to_string(),
        ));


        let mut added_genres: HashMap<String, i64> = HashMap::new();
        let mut added_artists: HashMap<String, i64> = HashMap::new();
        let mut added_albums: HashMap<String, i64> = HashMap::new();
        let mut added_tracks: HashMap<String, i64> = HashMap::new();
        let mut added_art: HashMap<&[u8], i64> = HashMap::new();
        let mut album_art: HashMap<i64, AlbumArtCounter> = HashMap::new();

        // Count existing unknown-album tracks once; use an in-memory counter as
        // new tracks are inserted instead of running COUNT(*) for every orphan.
        let existing_orphan_tracks = Database::query_orphan_tracks(tx)? as u32;
        let mut added_orphan_tracks = 0u32;
        let current_folder_id = Database::add_folder(tx, folder_uri)?;

        // Metadata files are extracted in parallel, but the SQLite import
        // should remain deterministic across runs. HashMap iteration order is
        // intentionally unspecified, so sort paths before assigning database IDs.
        let mut ordered_tags: Vec<_> = tags.iter().collect();
        ordered_tags.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

        for (uri, track_map) in ordered_tags {
            debug!("getting tags from:\n\t -> {:?}", uri);

            let mut title_tag = String::new();
            let mut artist_tag = String::new();
            let mut album_tag = String::new();
            let mut albumartist_tag = String::new();
            let mut date_tag = String::new();
            let mut genre_tag = String::new();
            let cover_art = cover_art_map.get(uri);
            let mut duration_tag = None;
            let mut disc_number_tag = None;
            let mut track_number_tag = None;
            let mut filetype_tag = String::new();


            for (type_key, type_map) in track_map {

                match type_key.as_str() {
                    "str_list" => {
                        for (tag, tag_val) in type_map {
                            if let MapVal::List(value) = tag_val {
                                //debug!("\t\ttagging {} {:?} ", tag, value);

                                match tag.as_str() {
                                    "genre" => {
                                        genre_tag = value.first().cloned().unwrap_or_default();
                                    },
                                    "album" => {
                                        album_tag = value.first().cloned().unwrap_or_default();
                                    },
                                    "title" => {
                                        title_tag = value.first().cloned().unwrap_or_default();
                                    },
                                    "albumartist" => {
                                        albumartist_tag = value.first().cloned().unwrap_or_default();
                                    },
                                    "artist" => {
                                        artist_tag = value.first().cloned().unwrap_or_default();
                                    },
                                    "date" => {
                                        date_tag = value.first().cloned().unwrap_or_default();
                                    },
                                    "tracknumber" => {
                                        track_number_tag = value.first().and_then(|value| parse_number_tag(value));
                                    },
                                    "discnumber" => {
                                        disc_number_tag = value.first().and_then(|value| parse_number_tag(value));
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
                                        track_number_tag = parse_number_tag(value);
                                    },
                                    "discnumber" => {
                                        disc_number_tag = parse_number_tag(value);
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
            let is_orphan_album = album_key == "Unknown AlbumUnknown Artist";
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
                    if is_orphan_album {
                        existing_orphan_tracks + added_orphan_tracks + 1
                    } else {
                        error!("No track number for {}", song_key);
                        1
                    }
                },
            };
   
            // add genre to the database table
            let genre_id = if !genre_tag.is_empty() {
                if !added_genres.contains_key(&genre_tag) {
                    Some(Database::add_genre(tx, genre_tag.clone())?)
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
                    Database::add_artist(tx, albumartist_tag.clone())?
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
                    Some(Database::add_cover_art(tx, bytes)?)
                }
            } else {
                None
            };
            
            // add albums to the database table
            let album_id = if !added_albums.contains_key(&album_key) {
                Database::add_album_full(
                    tx,
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

            

            let metadata = fs::metadata(uri)?;
            let modification_time = metadata.modified()?;
            let modification_time_dt = DateTime::<Utc>::from(modification_time);

            // if let Some(w) = window {
            //     w.set_import_message(&format!("Adding: {}", uri.clone()));
            // }

            let track_id =  Database::add_track_full(
                tx,
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

                    // Update an album's selected cover in expected O(1)
                    // time per track, instead of rescanning every previous
                    // artwork ID whenever a track is imported.
                    if let Some(selected_art_id) = album_art
                        .entry(album_id)
                        .or_default()
                        .record(art_id)
                    {
                        Database::update_album_art(tx, album_id, selected_art_id)?;
                    }
                }
            }

            added_albums.insert(album_key, album_id);
            added_tracks.insert(song_key, track_id);
            if is_orphan_album {
                added_orphan_tracks += 1;
            }

        }

        Ok(())
    }
}

#[derive(Default)]
struct AlbumArtCounter {
    // (count, first position) for every artwork ID seen on this album.
    frequencies: HashMap<i64, (usize, usize)>,
    // (ID, count, first position) of the current winner.
    best: Option<(i64, usize, usize)>,
    next_position: usize,
}

impl AlbumArtCounter {
    /// Record one artwork occurrence. Returns the selected ID only when the
    /// winner changes, preserving first-seen tie-breaking from the old logic.
    fn record(&mut self, id: i64) -> Option<i64> {
        let position = self.next_position;
        self.next_position += 1;

        let (count, first_position) = {
            let entry = self.frequencies.entry(id).or_insert((0, position));
            entry.0 += 1;
            *entry
        };

        let becomes_best = match self.best {
            None => true,
            Some((best_id, _, _)) if best_id == id => {
                // The current winner remains the winner as its own frequency
                // increases. Keep its count current without asking SQLite to
                // rewrite the same album artwork on every subsequent track.
                self.best = Some((id, count, first_position));
                false
            }
            Some((_, best_count, best_position)) => {
                count > best_count || (count == best_count && first_position < best_position)
            }
        };

        if becomes_best {
            self.best = Some((id, count, first_position));
            Some(id)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AlbumArtCounter;

    #[test]
    fn first_artwork_becomes_the_initial_selection() {
        let mut counter = AlbumArtCounter::default();
        assert_eq!(counter.record(7), Some(7));
    }

    #[test]
    fn keeps_current_selection_while_other_id_is_tied() {
        let mut counter = AlbumArtCounter::default();
        assert_eq!(counter.record(4), Some(4));
        assert_eq!(counter.record(9), None);
    }

    #[test]
    fn changes_selection_when_another_id_becomes_more_frequent() {
        let mut counter = AlbumArtCounter::default();
        assert_eq!(counter.record(4), Some(4));
        assert_eq!(counter.record(9), None);
        assert_eq!(counter.record(9), Some(9));
        assert_eq!(counter.record(9), None);
    }

    #[test]
    fn restores_first_seen_id_when_frequencies_tie_again() {
        let mut counter = AlbumArtCounter::default();
        assert_eq!(counter.record(9), Some(9));
        assert_eq!(counter.record(4), None);
        assert_eq!(counter.record(4), Some(4));
        assert_eq!(counter.record(9), Some(9));
    }

    #[test]
    fn switches_winner_when_new_id_becomes_more_frequent() {
        let mut counter = AlbumArtCounter::default();
        assert_eq!(counter.record(9), Some(9));
        assert_eq!(counter.record(4), None);
        assert_eq!(counter.record(4), Some(4));
    }
}
