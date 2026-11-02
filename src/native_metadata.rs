/* native_metadata.rs
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Native Rust audio metadata scanner. The existing database importer consumes
 * the same intermediate map shape as the previous Mutagen-backed importer.
 */

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
        Arc,
    },
    thread,
};

use gtk::glib::Sender;
use log::{debug, warn};

use audex::{
    File,
    StandardField,
    StreamInfo,
};

use crate::{
    database::DatabaseAction,
    importer::MapVal,
};

type TrackTags = HashMap<String, HashMap<String, HashMap<String, MapVal>>>;
type Artwork = HashMap<String, Vec<u8>>;

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "mp3", "aif", "aiff", "ogg", "opus", "flac", "mp4", "m4a", "asf", "wma", "wmv",
];

/// Scan a music folder in parallel and return metadata in the legacy database
/// importer's data shape. No GTK objects are touched by worker threads.
pub fn scan_folder(
    folder: &Path,
    progress_sender: Sender<DatabaseAction>,
) -> Result<(TrackTags, Artwork), String> {
    let files = collect_audio_files(folder)?;
    if files.is_empty() {
        return Err(format!("No supported audio files found in {}", folder.display()));
    }

    let total = files.len();
    let files = Arc::new(files);
    let next_index = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let last_percentage = Arc::new(AtomicUsize::new(0));
    let worker_count = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(2)
        .min(total)
        .max(1);

    debug!(
        "Scanning {} audio files with {} Rust workers",
        total, worker_count
    );

    let (result_sender, result_receiver) = mpsc::channel();

    let mut tags = TrackTags::with_capacity(total);
    let mut artwork = Artwork::new();

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let files = Arc::clone(&files);
            let next_index = Arc::clone(&next_index);
            let completed = Arc::clone(&completed);
            let last_percentage = Arc::clone(&last_percentage);
            let result_sender = result_sender.clone();
            let progress_sender = progress_sender.clone();

            scope.spawn(move || loop {
                let index = next_index.fetch_add(1, Ordering::Relaxed);
                let Some(path) = files.get(index) else {
                    break;
                };

                let result = extract_audio_file(path);
                let _ = result_sender.send((path.clone(), result));

                let finished = completed.fetch_add(1, Ordering::Relaxed) + 1;
                let percentage = finished.saturating_mul(100) / total;
                let previous = last_percentage.fetch_max(percentage, Ordering::Relaxed);

                if percentage > previous {
                    let _ = progress_sender.send(DatabaseAction::SetImportPercentage(
                        format!("{}%", percentage),
                    ));
                }
            });
        }

        drop(result_sender);

        for (path, result) in result_receiver {
            match result {
                Ok((uri, track_tags, cover_art)) => {
                    tags.insert(uri.clone(), track_tags);
                    if let Some(cover_art) = cover_art {
                        artwork.insert(uri, cover_art);
                    }
                }
                Err(error) => {
                    warn!("Skipping audio file {}: {}", path.display(), error);
                }
            }
        }
    });

    if tags.is_empty() {
        return Err(format!(
            "No readable audio metadata found in {}",
            folder.display()
        ));
    }

    debug!(
        "Native metadata scan completed: {} tracks, {} embedded covers",
        tags.len(),
        artwork.len()
    );

    Ok((tags, artwork))
}

fn collect_audio_files(folder: &Path) -> Result<Vec<PathBuf>, String> {
    if !folder.is_dir() {
        return Err(format!("Music folder does not exist: {}", folder.display()));
    }

    let mut pending = vec![folder.to_path_buf()];
    let mut files = Vec::new();

    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("Unable to read {}: {}", directory.display(), error))?;

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warn!("Unable to read an entry in {}: {}", directory.display(), error);
                    continue;
                }
            };

            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }

            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    warn!("Unable to inspect {}: {}", entry.path().display(), error);
                    continue;
                }
            };

            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() && is_supported_audio_file(&entry.path()) {
                files.push(entry.path());
            }
        }
    }

    // Deterministic input order makes imports reproducible and keeps the
    // database's auto-assigned IDs stable between scans of the same folder.
    files.sort();
    Ok(files)
}

fn is_supported_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
        .unwrap_or(false)
}

fn extract_audio_file(
    path: &Path,
) -> Result<(String, HashMap<String, HashMap<String, MapVal>>, Option<Vec<u8>>), String> {
    let audio = File::load(path).map_err(|error| error.to_string())?;
    let normalized = audio.to_tag_map();

    let duration = audio
        .info()
        .length()
        .map(|duration| duration.as_secs_f64() as f32)
        .ok_or_else(|| "audio duration is unavailable".to_string())?;

    let mut string_list_tags = HashMap::new();

    insert_standard_tag(&normalized, StandardField::Title, "title", &mut string_list_tags);
    insert_standard_tag(&normalized, StandardField::Artist, "artist", &mut string_list_tags);
    insert_standard_tag(&normalized, StandardField::Album, "album", &mut string_list_tags);
    insert_standard_tag(
        &normalized,
        StandardField::AlbumArtist,
        "albumartist",
        &mut string_list_tags,
    );
    insert_standard_tag(&normalized, StandardField::Date, "date", &mut string_list_tags);
    insert_standard_tag(&normalized, StandardField::Genre, "genre", &mut string_list_tags);
    insert_standard_tag(
        &normalized,
        StandardField::TrackNumber,
        "tracknumber",
        &mut string_list_tags,
    );
    insert_standard_tag(
        &normalized,
        StandardField::DiscNumber,
        "discnumber",
        &mut string_list_tags,
    );

    // Keep the same fallbacks as the previous Mutagen importer.
    if !string_list_tags.contains_key("title") {
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        string_list_tags.insert("title".to_string(), MapVal::List(vec![title]));
    }

    if !string_list_tags.contains_key("album") {
        string_list_tags.insert(
            "album".to_string(),
            MapVal::List(vec!["Unknown Album".to_string()]),
        );
    }

    if !string_list_tags.contains_key("albumartist") {
        let fallback_artist = match string_list_tags.get("artist") {
            Some(MapVal::List(values)) => values.first().cloned(),
            _ => None,
        }
        .filter(|artist| !artist.trim().is_empty())
        .unwrap_or_else(|| "Unknown Artist".to_string());

        string_list_tags.insert(
            "albumartist".to_string(),
            MapVal::List(vec![fallback_artist]),
        );
    }

    let filetype = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
        .unwrap_or_default();

    let mut str_tags = HashMap::new();
    str_tags.insert("filetype_".to_string(), MapVal::Str(filetype));

    let mut float_tags = HashMap::new();
    float_tags.insert("duration".to_string(), MapVal::Float(duration));

    let mut grouped_tags = HashMap::new();
    grouped_tags.insert("str_list".to_string(), string_list_tags);
    grouped_tags.insert("str".to_string(), str_tags);
    grouped_tags.insert("float".to_string(), float_tags);

    let uri = path.to_string_lossy().into_owned();
    let cover_art = extract_cover_art(&audio);

    Ok((uri, grouped_tags, cover_art))
}

fn insert_standard_tag(
    map: &audex::TagMap,
    field: StandardField,
    target_key: &str,
    destination: &mut HashMap<String, MapVal>,
) {
    if let Some(values) = map.get(&field) {
        let values: Vec<String> = values
            .iter()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .collect();

        if !values.is_empty() {
            destination.insert(target_key.to_string(), MapVal::List(values));
        }
    }
}

fn extract_cover_art(audio: &audex::DynamicFileType) -> Option<Vec<u8>> {
    // FLAC stores pictures as dedicated metadata blocks.
    if let Some(flac) = audio.downcast_ref::<audex::flac::FLAC>() {
        if let Some(picture) = flac.pictures.first() {
            return Some(picture.data.clone());
        }
    }

    // MP4/M4A stores cover art in the covr atom.
    if let Some(mp4) = audio.downcast_ref::<audex::mp4::MP4>() {
        if let Some(cover) = mp4.tags.as_ref().and_then(|tags| tags.covers.first()) {
            return Some(cover.data.clone());
        }
    }

    // Ogg Vorbis and Opus use base64-encoded FLAC Picture blocks. Audex
    // exposes a parser for those blocks, so no custom base64 code is needed.
    if let Some(vorbis) = audio.downcast_ref::<audex::oggvorbis::OggVorbis>() {
        if let Some(picture) = vorbis.tags.as_ref().and_then(|tags| tags.get_pictures().into_iter().next()) {
            return Some(picture.data);
        }
    }

    if let Some(opus) = audio.downcast_ref::<audex::oggopus::OggOpus>() {
        if let Some(picture) = opus.tags.as_ref().and_then(|tags| tags.get_pictures().into_iter().next()) {
            return Some(picture.data);
        }
    }

    // ASF/WMA embeds images in WM/Picture attributes.
    if let Some(asf) = audio.downcast_ref::<audex::asf::ASF>() {
        if let Some(picture) = asf
            .tags
            .get_first("WM/Picture")
            .and_then(|attribute| attribute.as_picture())
        {
            return Some(picture.data);
        }
    }

    // MP3 and AIFF carry embedded images in ID3 APIC frames.
    if let Some(mp3) = audio.downcast_ref::<audex::mp3::MP3>() {
        if let Some(data) = first_id3_picture(mp3.tags.as_ref()) {
            return Some(data);
        }
    }

    if let Some(aiff) = audio.downcast_ref::<audex::aiff::AIFF>() {
        if let Some(data) = first_id3_picture(aiff.tags.as_ref()) {
            return Some(data);
        }
    }

    None
}

fn first_id3_picture(tags: Option<&audex::id3::ID3Tags>) -> Option<Vec<u8>> {
    let tags = tags?;
    tags.dict.values().find_map(|frame| {
        frame
            .as_any()
            .downcast_ref::<audex::id3::APIC>()
            .map(|picture| picture.data.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::is_supported_audio_file;
    use std::path::Path;

    #[test]
    fn recognizes_existing_mutagen_extensions_case_insensitively() {
        for extension in [
            "mp3", "aif", "ogg", "opus", "flac", "mp4", "m4a", "asf", "wma", "wmv",
        ] {
            assert!(is_supported_audio_file(Path::new(&format!("track.{}", extension))));
            assert!(is_supported_audio_file(Path::new(&format!("track.{}", extension.to_uppercase()))));
        }
    }

    #[test]
    fn parses_real_audio_format_fixtures_when_configured() {
        let fixture_root = match std::env::var_os("RESONANCE_AUDIO_FIXTURES") {
            Some(path) => PathBuf::from(path),
            None => return,
        };

        let fixtures = [
            "silence-44-s.mp3",
            "silence-44-s.flac",
            "multipagecomment.ogg",
            "example.opus",
            "with-id3.aif",
            "with-id3.aiff",
            "alac.m4a",
            "silence-1.wma",
            "sample.wmv",
        ];

        for filename in fixtures {
            let path = fixture_root.join(filename);
            let (uri, grouped_tags, _) = extract_audio_file(&path)
                .unwrap_or_else(|error| panic!("Failed to parse {}: {}", filename, error));

            assert_eq!(uri, path.to_string_lossy());
            assert!(grouped_tags.contains_key("str_list"), "{} has no string tags", filename);
            assert!(grouped_tags.contains_key("float"), "{} has no stream duration", filename);

            match grouped_tags.get("str_list").and_then(|values| values.get("title")) {
                Some(MapVal::List(values)) if !values.is_empty() => (),
                _ => panic!("{} did not receive a title or fallback title", filename),
            }

            match grouped_tags.get("float").and_then(|values| values.get("duration")) {
                Some(MapVal::Float(duration)) if *duration > 0.0 => (),
                _ => panic!("{} has an invalid duration", filename),
            }
        }

        let flac = extract_audio_file(&fixture_root.join("silence-44-s.flac"))
            .expect("FLAC fixture should parse");
        assert!(flac.2.is_some(), "FLAC embedded artwork should be extracted");

        let mp4 = extract_audio_file(&fixture_root.join("covr-with-name.m4a"))
            .expect("MP4 fixture should parse");
        assert!(mp4.2.is_some(), "MP4 embedded artwork should be extracted");
    }

    #[test]
    fn ignores_unrecognized_extensions() {
        assert!(!is_supported_audio_file(Path::new("cover.jpg")));
        assert!(!is_supported_audio_file(Path::new("notes.txt")));
    }
}
