/* track.rs
 *
 * SPDX-FileCopyrightText: 2023 nate-xyz
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use adw::subclass::prelude::*;
use gtk::{glib, gio};
use glib::prelude::ToVariant;

use std::{cell::Cell, cell::RefCell};

// Avoid invoking the regular-expression engine for every title, album and
// artist while constructing large track models.
pub(crate) fn sort_value(value: &str) -> String {
    let lowercase = value.to_lowercase();

    for article in ["the", "a", "an"] {
        if let Some(remainder) = lowercase.strip_prefix(article) {
            if remainder.chars().next().is_some_and(char::is_whitespace) {
                return remainder
                    .trim_start_matches(char::is_whitespace)
                    .to_owned();
            }
        }
    }

    lowercase
}

#[cfg(test)]
mod sort_value_tests {
    use super::sort_value;

    #[test]
    fn strips_leading_articles_case_insensitively() {
        assert_eq!(sort_value("The Beatles"), "beatles");
        assert_eq!(sort_value("A Tribe Called Quest"), "tribe called quest");
        assert_eq!(sort_value("An Example"), "example");
    }

    #[test]
    fn strips_all_leading_whitespace_after_article() {
        assert_eq!(sort_value("The  Beatles"), "beatles");
        assert_eq!(sort_value("An\tExample"), "example");
    }

    #[test]
    fn preserves_words_without_an_article_prefix() {
        assert_eq!(sort_value("Theatre"), "theatre");
        assert_eq!(sort_value("Anarchy"), "anarchy");
        assert_eq!(sort_value(" Beatles"), " beatles");
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TrackPriv {
        pub id: Cell<i64>,
        pub title: RefCell<String>,
        pub album_name: RefCell<String>,
        pub album_artist: RefCell<String>,
        pub search_string: RefCell<String>,
        pub sort_string: RefCell<String>,
        pub sort_title: RefCell<String>,
        pub sort_album: RefCell<String>,
        pub sort_artist: RefCell<String>,
        pub filetype: RefCell<String>,
        pub uri: RefCell<String>,
        pub date: RefCell<String>,
        pub genre: RefCell<String>,
        pub duration: Cell<f64>,
        pub track_number: Cell<i64>,
        pub disc_number: Cell<i64>,
        // artist_parent: RefCell<Option<Artist>>,
        // album_parent: RefCell<Option<Album>>,
        pub genre_id: Cell<Option<i64>>,
        pub album_id: Cell<i64>,
        pub artist_id: Cell<i64>,
        // img_data: RefCell<Option<Vec<u8>>>,
        pub cover_art_id: Cell<Option<i64>>,
        pub menu: gio::Menu,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackPriv {
        const NAME: &'static str = "Track";
        type Type = super::Track;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for TrackPriv {
        fn constructed(&self) {
            self.parent_constructed();
        }
    }

    impl TrackPriv {}
}

glib::wrapper! {
    pub struct Track(ObjectSubclass<imp::TrackPriv>);
}

impl Default for Track {
    fn default() -> Self {
        glib::Object::builder::<Track>().build()
    }
}

impl Track {
    pub fn new(id: i64, title: String, album_name: String, album_artist: String, filetype: String, uri: String, date: String, genre: String, duration: f64, track_number: i64, disc_number: i64) -> Track {
        let track: Track = Self::default();
        track.load_info(id, title, album_name, album_artist, filetype, uri, date, genre, duration, track_number, disc_number);
        track
    }

    pub fn load_info(&self, id: i64, title: String, album_name: String, album_artist: String, filetype: String, uri: String, date: String, genre: String, duration: f64, track_number: i64, disc_number: i64) {
        let imp = self.imp();
        
        let stripped_title = sort_value(&title);
        let stripped_artist = sort_value(&album_artist);
        let stripped_album = sort_value(&album_name);

        imp.sort_title.replace(stripped_title.clone());
        imp.sort_artist.replace(stripped_artist.clone());
        imp.sort_album.replace(stripped_album.clone());

        imp.sort_string.replace(format!(
            "{} {} {}",
            stripped_title,
            stripped_album,
            stripped_artist
        ));
        imp.search_string.replace(format!("{} {} {}", title, album_name, album_artist));

        imp.id.set(id);
        imp.title.replace(title);
        imp.album_name.replace(album_name);
        imp.album_artist.replace(album_artist);
        imp.filetype.replace(filetype);
        imp.uri.replace(uri);
        imp.date.replace(date);
        imp.genre.replace(genre);
        imp.duration.set(duration);
        imp.track_number.set(track_number);
        imp.disc_number.set(disc_number);
        //self.create_menu();
    }
    pub fn add_genre_id(&self, genre_id: i64) {
        self.imp().genre_id.set(Some(genre_id));
    }

    pub fn add_artist_id(&self, artist_id: i64) {
        self.imp().artist_id.set(artist_id);
    }

    pub fn add_album_id(&self, album_id: i64) {
        self.imp().album_id.set(album_id);
    }


    pub fn add_cover_art_option(&self, cover_art_option: Option<i64>) {
        self.imp().cover_art_id.set(cover_art_option);
    }

    pub fn cover_art_option(&self) -> Option<i64> {
        self.imp().cover_art_id.get()
    }

    pub fn id(&self) -> i64 {
        self.imp().id.get().clone()
    }

    pub fn album_id(&self) -> i64 {
        self.imp().album_id.get().clone()
    }

    pub fn artist_id(&self) -> i64 {
        self.imp().artist_id.get().clone()
    }

    pub fn disc_number(&self) -> i64 {
        self.imp().disc_number.get().clone()
    }

    pub fn track_number(&self) -> i64 {
        self.imp().track_number.get().clone()
    }

    pub fn title(&self) -> String {
        self.imp().title.borrow().clone()
    }

    pub fn title_ref(&self) -> std::cell::Ref<'_, str> {
        std::cell::Ref::map(self.imp().title.borrow(), |value| value.as_str())
    }

    pub fn sort_title(&self) -> String {
        self.imp().sort_title.borrow().clone()
    }

    pub fn sort_title_ref(&self) -> std::cell::Ref<'_, str> {
        std::cell::Ref::map(self.imp().sort_title.borrow(), |value| value.as_str())
    }

    pub fn uri(&self) -> String {
        self.imp().uri.borrow().clone()
    }

    pub fn date(&self) -> String {
        self.imp().date.borrow().clone()
    }

    pub fn date_ref(&self) -> std::cell::Ref<'_, str> {
        std::cell::Ref::map(self.imp().date.borrow(), |value| value.as_str())
    }

    pub fn genre(&self) -> String {
        self.imp().genre.borrow().clone()
    }

    pub fn genre_ref(&self) -> std::cell::Ref<'_, str> {
        std::cell::Ref::map(self.imp().genre.borrow(), |value| value.as_str())
    }

    pub fn artist(&self) -> String {
        self.imp().album_artist.borrow().clone()
    }

    pub fn artist_ref(&self) -> std::cell::Ref<'_, str> {
        std::cell::Ref::map(self.imp().album_artist.borrow(), |value| value.as_str())
    }

    pub fn sort_artist(&self) -> String {
        self.imp().sort_artist.borrow().clone()
    }

    pub fn sort_artist_ref(&self) -> std::cell::Ref<'_, str> {
        std::cell::Ref::map(self.imp().sort_artist.borrow(), |value| value.as_str())
    }

    pub fn album(&self) -> String {
        self.imp().album_name.borrow().clone()
    }

    pub fn album_ref(&self) -> std::cell::Ref<'_, str> {
        std::cell::Ref::map(self.imp().album_name.borrow(), |value| value.as_str())
    }

    pub fn sort_album(&self) -> String {
        self.imp().sort_album.borrow().clone()
    }

    pub fn sort_album_ref(&self) -> std::cell::Ref<'_, str> {
        std::cell::Ref::map(self.imp().sort_album.borrow(), |value| value.as_str())
    }

    pub fn duration(&self) -> f64 {
        self.imp().duration.get().clone()
    }

    pub fn search_string(&self) -> String {
        self.imp().search_string.borrow().clone()
    }

    pub fn search_string_ref(&self) -> std::cell::Ref<'_, str> {
        std::cell::Ref::map(self.imp().search_string.borrow(), |value| value.as_str())
    }

    pub fn sort_string(&self) -> String {
        self.imp().sort_string.borrow().clone()
    }

    pub fn sort_string_ref(&self) -> std::cell::Ref<'_, str> {
        std::cell::Ref::map(self.imp().sort_string.borrow(), |value| value.as_str())
    }
    
    #[allow(dead_code)]
    fn create_menu(&self) {
        let imp = self.imp();
    
        let menu = gio::Menu::new();
    
        let menu_item = gio::MenuItem::new(Some(&format!("Play «{}»", self.title())), None);
        menu_item.set_action_and_target_value(Some("win.play-track"), Some(&imp.id.get().to_variant()));
        menu.append_item(&menu_item);
    
        let menu_item = gio::MenuItem::new(Some(&format!("Add «{}» to Queue", self.title())), None);
        menu_item.set_action_and_target_value(Some("win.add-track"), Some(&imp.id.get().to_variant()));
        menu.append_item(&menu_item);
    
        // let menu_item = gio::MenuItem::new(Some(&format!("Play «{}» from «{}»", self.album(), self.title())), None);
        // menu_item.set_action_and_target_value(Some("win.play-album-from-track"), Some(&imp.id.get().to_variant()));
        // menu.append_item(&menu_item);

        imp.menu.append_section(Some("Play"), &menu);
    
        let menu = gio::Menu::new();
    
        let menu_item = gio::MenuItem::new(Some(&format!("Go to Album «{}» Detail", self.title())), None);
        menu_item.set_action_and_target_value(Some("win.go-to-album-detail"), Some(&imp.id.get().to_variant()));
        menu.append_item(&menu_item);
    
        let menu_item = gio::MenuItem::new(Some(&format!("Go to Artist {} Detail", self.artist())), None);
        menu_item.set_action_and_target_value(Some("win.go-to-artist-detail"), Some(&imp.artist_id.get().to_variant()));
        menu.append_item(&menu_item);
    
        imp.menu.append_section(Some("Navigate"), &menu);
    
        let menu = gio::Menu::new();
        
        let menu_item = gio::MenuItem::new(Some(&format!("Create Playlist from «{}»", self.title())), None);
        menu_item.set_action_and_target_value(Some("win.create-playlist-from-track"), Some(&imp.id.get().to_variant()));
        menu.append_item(&menu_item);
    
        let menu_item = gio::MenuItem::new(Some(&format!("Add «{}» to Playlist", self.title())), None);
        menu_item.set_action_and_target_value(Some("win.add-track-to-playlist"), Some(&imp.id.get().to_variant()));
        menu.append_item(&menu_item);
    
        imp.menu.append_section(Some("Playlist"), &menu);
    }
    
    pub fn menu_model(&self)-> &gio::Menu {
        &self.imp().menu
    }
}