/* queue.rs
 *
 * SPDX-FileCopyrightText: 2023 nate-xyz
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use gtk::{prelude::SettingsExt, glib, glib::Sender, gio};
use gtk_macros::send;

use std::{cell::Cell, cell::RefCell, rc::Rc};
use rand::{seq::SliceRandom, thread_rng};
use log::{error, debug};

use crate::model::track::Track;
use crate::util::settings_manager;

#[derive(Clone, Debug)]
pub enum QueueAction {
    QueueUpdate,
    QueuePositionUpdate(u64),
    QueueTimeRemaining(u64),
    QueueEmpty,
    QueueNonEmpty,
    QueueRepeatMode(RepeatMode),
    QueueDuration(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, glib::Enum)]
#[enum_type(name = "RepeatMode")]
pub enum RepeatMode {
    Normal,
    Loop,
    LoopSong,
    Shuffle
}

impl Default for RepeatMode {
    fn default() -> Self {
        Self::Normal
    }
}

impl RepeatMode {
    fn new_from_settings(mode: i32) -> Self {
        let repeat_mode = match mode {
            0 => RepeatMode::Normal,
            1 => RepeatMode::Loop,
            2 => RepeatMode::LoopSong,
            3 => RepeatMode::Shuffle,
            _ => RepeatMode::Normal,
        };
        repeat_mode
    }
}

#[derive(Debug)]
pub struct Queue {
    pub sender: Sender<QueueAction>,
    pub queue: RefCell<Vec<Rc<Track>>>,
    pub sequential_queue: RefCell<Vec<Rc<Track>>>,
    pub current_position: Cell<u64>,
    pub current_track: RefCell<Option<Rc<Track>>>,
    pub repeat_mode: Cell<RepeatMode>,
    pub shuffle_loop: Cell<bool>,
    pub settings: gio::Settings,
}

fn position_after_remove(current: usize, removed: usize, new_len: usize) -> usize {
    if new_len == 0 {
        0
    } else if removed < current {
        current.saturating_sub(1).min(new_len - 1)
    } else if removed == current {
        removed.min(new_len - 1)
    } else {
        current.min(new_len - 1)
    }
}

fn position_after_move(current: usize, from: usize, to: usize) -> usize {
    if current == from {
        to
    } else if from < current && to >= current {
        current.saturating_sub(1)
    } else if from > current && to <= current {
        current + 1
    } else {
        current
    }
}

impl Queue {
    pub fn new(queue_sender: Sender<QueueAction>) -> Queue {
        let settings = settings_manager();
        let shuffle_loop = settings.boolean("shuffle-mode-loop");
        let repeat_mode = settings.int("repeat-mode");

        let queue = Self {
            sender: queue_sender,
            queue: RefCell::new(Vec::new()),
            sequential_queue: RefCell::new(Vec::new()),
            current_position: Cell::new(0),
            current_track: RefCell::new(None),
            repeat_mode: Cell::new(RepeatMode::new_from_settings(repeat_mode)),
            shuffle_loop: Cell::new(shuffle_loop),
            settings,
        };
        queue
    }

    pub fn set_shuffle_mode(&self, mode: bool) {
        self.shuffle_loop.set(mode);
    }

    pub fn current_track(&self) -> Option<Rc<Track>> {
        self.current_track.borrow().clone()
    }

    pub fn set_song(&self, track: Rc<Track>) {
        self.clear_queue();
        self.add_track(track.clone());
        send!(self.sender, QueueAction::QueuePositionUpdate(0));
        //self.current_track.replace(Some(track));
    }

    fn clear_queue(&self) {
        self.queue.replace(Vec::new());
        self.sequential_queue.replace(Vec::new());
        self.current_position.set(0);
        self.current_track.replace(None);
    }

    pub fn add_track(&self, track: Rc<Track>) {
        self.queue.borrow_mut().push(track.clone());
        self.sequential_queue.borrow_mut().push(track);
        send!(self.sender, QueueAction::QueueNonEmpty);
        send!(self.sender, QueueAction::QueueUpdate);
        self.current_song_update();

    }

    pub fn update_from_first(&self) {
        if self.queue.borrow().len() > 0 {
            self.current_position.set(0);
            self.current_song_update();
            self.calculate_time_remaining();
        }
    }

    fn current_song_update(&self) {
        let track = self
            .queue
            .borrow()
            .get(self.current_position.get() as usize)
            .cloned();

        match track {
            Some(track) => {
                self.current_track.replace(Some(track));
                send!(self.sender, QueueAction::QueuePositionUpdate(self.position()));
            }
            None => {
                self.current_track.replace(None);
                send!(self.sender, QueueAction::QueueEmpty);
            }
        }
        self.calculate_time_remaining();
    }

    pub fn set_album(&self, tracks: Vec<Rc<Track>>) {
        self.clear_queue();
        self.add_tracks(tracks);
        send!(self.sender, QueueAction::QueuePositionUpdate(0));
    }

    pub fn add_tracks(&self, tracks: Vec<Rc<Track>>) {
        self.sequential_queue.borrow_mut().extend(tracks.iter().cloned());
        self.queue.borrow_mut().extend(tracks);
        self.current_song_update();
        if self.repeat_mode.get() == RepeatMode::Shuffle {
            self.shuffle_tracks();
        }
        send!(self.sender, QueueAction::QueueNonEmpty);
        send!(self.sender, QueueAction::QueueUpdate);
    }

    pub fn set_position(&self, position: u64) {
        let queue_length = self.queue.borrow().len() as u64;
        if queue_length == 0 {
            self.current_track.replace(None);
            return;
        }
        if position >= queue_length {
            return;
        }

        self.current_position.set(position);
        self.current_song_update();

    }

    pub fn remove_track(&self, position_to_remove: usize) {
        let queue_len = self.queue.borrow().len();
        if position_to_remove >= queue_len {
            return;
        }

        if queue_len == 1 {
            self.end_queue();
            return;
        }

        let current_position = self.position() as usize;
        let removed_track = self.queue.borrow()[position_to_remove].clone();
        self.queue.borrow_mut().remove(position_to_remove);

        // Keep the non-shuffled order in sync, including duplicate queue entries.
        let sequential_index = {
            self.sequential_queue
                .borrow()
                .iter()
                .position(|track| Rc::ptr_eq(track, &removed_track))
        };
        if let Some(sequential_index) = sequential_index {
            self.sequential_queue.borrow_mut().remove(sequential_index);
        }

        let next_position = position_after_remove(
            current_position,
            position_to_remove,
            queue_len - 1,
        );

        self.current_position.set(next_position as u64);
        send!(self.sender, QueueAction::QueueUpdate);
        self.current_song_update();
    }

    pub fn reorder_track(&self, old_position: usize, new_position: usize) {
        let queue_len = self.queue.borrow().len();
        if old_position >= queue_len || new_position >= queue_len || old_position == new_position {
            return;
        }

        let current_position = self.position() as usize;
        let next_position = position_after_move(
            current_position,
            old_position,
            new_position,
        );

        {
            let mut queue = self.queue.borrow_mut();
            let track = queue.remove(old_position);
            queue.insert(new_position, track);
        }

        // The sequential copy is authoritative when shuffle is disabled.
        if self.repeat_mode.get() != RepeatMode::Shuffle {
            let mut sequential = self.sequential_queue.borrow_mut();
            if old_position < sequential.len() && new_position < sequential.len() {
                let track = sequential.remove(old_position);
                sequential.insert(new_position, track);
            }
        }

        self.current_position.set(next_position as u64);
        send!(self.sender, QueueAction::QueueUpdate);
        self.current_song_update();
    }

    pub fn on_repeat_change(&self, mode: RepeatMode) {
        let mode = if mode == self.repeat_mode.get() {
            RepeatMode::Normal
        } else {
            mode
        };
        let current_track = self.current_track.borrow().clone();
        match mode {
            RepeatMode::Shuffle => {
                self.shuffle_tracks();
            },
            _ => {
                debug!("Restoring queue from sequential");
                self.queue.replace(self.sequential_queue.borrow().clone());
                if let Some(current_track) = current_track {
                    let restored_position = self.queue.borrow()
                        .iter()
                        .position(|track| Rc::ptr_eq(track, &current_track))
                        .unwrap_or(0);
                    self.current_position.set(restored_position as u64);
                    self.current_song_update();
                } else if self.queue.borrow().is_empty() {
                    self.current_position.set(0);
                    self.current_track.replace(None);
                }
            },
        }

        self.repeat_mode.set(mode);

        let mode_int = match mode {
            RepeatMode::Normal => 0,
            RepeatMode::Loop => 1,
            RepeatMode::LoopSong => 2,
            RepeatMode::Shuffle => 3,
        };
        _ = self.settings.set_int("repeat-mode", mode_int);

        if mode == RepeatMode::Shuffle {
            send!(self.sender, QueueAction::QueueUpdate);
        }
        send!(self.sender, QueueAction::QueueRepeatMode(mode));
    }

    fn shuffle_tracks(&self) {
        debug!("Shuffling songs");
        let remaining_songs = self.queue_len() as i64 - self.current_position.get() as i64 - 1;
        if remaining_songs <= 0 {
            debug!("Nothing left to shuffle");
            return;
        }
        let mut remaining_songs: Vec<Rc<Track>> = self.queue.borrow_mut().drain((self.current_position.get()+1) as usize..).collect();
        let mut rng = thread_rng();
        remaining_songs.shuffle(&mut rng);
        self.queue.borrow_mut().append(&mut remaining_songs);
    }

    pub fn get_previous(&self) {
        if self.is_empty() {
            self.current_track.replace(None);
            send!(self.sender, QueueAction::QueueEmpty);
            return;
        }
        let queue_length = self.queue.borrow().len() as u64;
        let position = self.current_position.get();
        match self.repeat_mode.get() {
            RepeatMode::Normal => {
                if position <= 0 {
                    self.current_position.set(0);
                } else {
                    self.current_position.set(position-1);
                }
                self.current_song_update();

            },
            RepeatMode::Loop => {
                if position <= 0 {
                    self.current_position.set(queue_length-1);
                } else {
                    self.current_position.set(position-1);
                }
                self.current_song_update();
            },
            RepeatMode::LoopSong => {
                send!(self.sender, QueueAction::QueuePositionUpdate(position));
                self.calculate_time_remaining();
            },
            RepeatMode::Shuffle => {
                if position <= 0 {
                    self.current_position.set(queue_length-1);
                } else {
                    self.current_position.set(position-1);
                }
                self.current_song_update();
            },
        }
    }

    pub fn get_next(&self) {
        if self.is_empty() {
            self.current_track.replace(None);
            send!(self.sender, QueueAction::QueueEmpty);
            return;
        }
        let queue_length = self.queue.borrow().len() as u64;
        let position = self.current_position.get()+1;
        
        match self.repeat_mode.get() {
            RepeatMode::Normal => {
                if position >= queue_length {
                    self.current_track.replace(None);
                    self.end_queue();
                    // send!(self.sender, QueueAction::QueueEmpty);
                    return;
                } else {
                    self.current_position.set(position);
                    self.current_song_update();
                }
            },
            RepeatMode::Loop => {
                if position >= queue_length {
                    self.current_position.set(0);
                } else {
                    self.current_position.set(position);
                }
                self.current_song_update();
            },
            RepeatMode::LoopSong => {
                send!(self.sender, QueueAction::QueuePositionUpdate(position-1));
                self.calculate_time_remaining();
            },
            RepeatMode::Shuffle => {
                if self.shuffle_loop.get() {
                    if position >= queue_length {
                        self.current_position.set(0);
                    } else {
                        self.current_position.set(position);
                    }
                } else {
                    if position >= queue_length {
                        self.current_track.replace(None);
                        self.end_queue();
                        
                        return;
                    } else {
                        self.current_position.set(position);
                    }
                }
                self.current_song_update();
            },
        }
    }

    pub fn end_queue(&self) {
        self.clear_queue();
        send!(self.sender, QueueAction::QueueEmpty);
    }

    fn calculate_time_remaining(&self) {
        let queue = self.queue.borrow();
        let position = (self.position() as usize).min(queue.len());
        let total_duration = queue
            .iter()
            .skip(position)
            .map(|track| track.duration())
            .sum();
        send!(self.sender, QueueAction::QueueDuration(total_duration));
    }

    pub fn tracks(&self) -> Vec<Rc<Track>> {
        self.queue.borrow().clone()
    }

    pub fn track_ids(&self) -> Vec<i64> {
        let mut ret = Vec::new();
        for track in self.tracks() {
            ret.push(track.id());
        }
        ret
    }

    fn position(&self) -> u64 {
        self.current_position.get()
    }

    pub fn repeat_mode(&self) -> RepeatMode {
        self.repeat_mode.get()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.borrow().len() == 0
    }

    fn queue_len(&self) -> usize {
        self.queue.borrow().len()
    }
}
    

#[cfg(test)]
mod tests {
    use super::{position_after_move, position_after_remove};

    #[test]
    fn removing_tracks_preserves_or_advances_the_current_position() {
        assert_eq!(position_after_remove(4, 1, 4), 3);
        assert_eq!(position_after_remove(2, 4, 4), 2);
        assert_eq!(position_after_remove(2, 2, 4), 2);
        assert_eq!(position_after_remove(0, 0, 0), 0);
        assert_eq!(position_after_remove(9, 1, 4), 3);
    }

    #[test]
    fn moving_queue_entries_keeps_the_current_track_selected() {
        assert_eq!(position_after_move(2, 2, 5), 5);
        assert_eq!(position_after_move(4, 1, 5), 3);
        assert_eq!(position_after_move(1, 5, 0), 2);
        assert_eq!(position_after_move(2, 0, 1), 2);
        assert_eq!(position_after_move(2, 4, 5), 2);
    }
}
