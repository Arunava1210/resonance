/* cover_art_pixbuf_loader.rs
 *
 * SPDX-FileCopyrightText: 2023 nate-xyz
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use gtk::{gdk_pixbuf::Pixbuf, gdk_pixbuf, gdk, glib};
use gtk::{subclass::prelude::*, prelude::*};
use std::{cell::RefCell, rc::Rc};

use crate::util;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct CoverArtPixbufLoader {
        pub data: RefCell<Option<Vec<u8>>>,
        pub pixbuf: RefCell<Option<Rc<Pixbuf>>>,
        pub palette: RefCell<Option<Vec<gdk::RGBA>>>
        //pub data: RefCell<Option<Vec<u8>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverArtPixbufLoader {
        const NAME: &'static str = "CoverArtPixbufLoader";
        type Type = super::CoverArtPixbufLoader;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for CoverArtPixbufLoader {
        fn constructed(&self) {
            self.parent_constructed();
        }
    }

    impl CoverArtPixbufLoader {}
}

glib::wrapper! {
    pub struct CoverArtPixbufLoader(ObjectSubclass<imp::CoverArtPixbufLoader>);
}

impl Default for CoverArtPixbufLoader {
    fn default() -> Self {
        glib::Object::builder::<CoverArtPixbufLoader>().build()
    }
}

impl CoverArtPixbufLoader {
    pub fn new() -> CoverArtPixbufLoader {
        let capl: CoverArtPixbufLoader = glib::Object::builder::<CoverArtPixbufLoader>().build();
        capl
    }

    // Keep the compressed bytes during model population. Decoding and palette
    // extraction are deferred until the UI actually requests this artwork.
    pub fn load(&self, data: Vec<u8>) -> Result<(), glib::Error> {
        self.imp().data.replace(Some(data));
        Ok(())
    }

    fn ensure_decoded(&self) -> Result<(), String> {
        if self.imp().pixbuf.borrow().is_some() {
            return Ok(());
        }

        let data = self.imp().data.borrow()
            .as_ref()
            .cloned()
            .ok_or_else(|| "No artwork data available".to_string())?;

        let loader = gdk_pixbuf::PixbufLoader::new();
        loader.write(&data).map_err(|error| error.to_string())?;
        loader.close().map_err(|error| error.to_string())?;

        let finished_pixbuf = loader.pixbuf()
            .ok_or_else(|| "Unable to decode artwork".to_string())?;
        let palette = util::load_palette(&finished_pixbuf);

        self.imp().pixbuf.replace(Some(Rc::new(finished_pixbuf)));
        self.imp().palette.replace(palette);
        // The decoded pixbuf is now cached, so release the original compressed
        // image bytes to avoid retaining both representations.
        self.imp().data.replace(None);

        Ok(())
    }

    pub fn pixbuf(&self) -> Result<Rc<Pixbuf>, String> {
        self.ensure_decoded()?;
        self.imp().pixbuf.borrow().as_ref()
            .map(Rc::clone)
            .ok_or_else(|| "Unable to access pixbuf".to_string())
    }

    pub fn palette(&self) -> Option<Vec<gdk::RGBA>> {
        self.ensure_decoded().ok()?;
        self.imp().palette.borrow().as_ref().cloned()
    }
}