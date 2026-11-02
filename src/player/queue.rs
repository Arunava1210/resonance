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

// Keep queue indices valid when a track is removed or two entries are swapped.
fn position_after_remove(current: usize, removed: usize, new_len: usize) -> usize {
    if new_len == 0 {
        return 0;
    }

    let adjusted = if removed < current {
        current.saturating_sub(1)
    } else {
        current
    };

    adjusted.min(new_len - 1)
}

fn position_after_swap(current: usize, first: usize, second: usize) -> usize {
    if current == first {
        second
    } else if current == second {
        first
    } else {
        current
    }
}

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
        let track = {
            let queue = self.queue.borrow();
            queue.get(self.current_position.get() as usize).cloned()
        };

        self.current_track.replace(track);
        send!(self.sender, QueueAction::QueuePositionUpdate(self.position()));
        self.calculate_time_remaining();
    }

    pub fn set_album(&self, tracks: Vec<Rc<Track>>) {
        self.clear_queue();
        if tracks.is_empty() {
            self.end_queue();
            return;
        }

        self.add_tracks(tracks);
        send!(self.sender, QueueAction::QueuePositionUpdate(0));
    }

    pub fn add_tracks(&self, tracks: Vec<Rc<Track>>) {
        if tracks.is_empty() {
            return;
        }

        let queue_was_empty = self.queue.borrow().is_empty();
        self.sequential_queue
            .borrow_mut()
            .extend(tracks.iter().cloned());
        self.queue.borrow_mut().extend(tracks);

        if queue_was_empty {
            self.current_position.set(0);
        }

        self.current_song_update();
        if self.repeat_mode.get() == RepeatMode::Shuffle {
            self.shuffle_tracks();
        }
        send!(self.sender, QueueAction::QueueNonEmpty);
        send!(self.sender, QueueAction::QueueUpdate);
    }

    pub fn set_position(&self, position: u64) {
        let queue_length = self.queue.borrow().len() as u64;
        if position >= queue_length {
            return;
        }

        self.current_position.set(position);
        self.current_song_update();
    }

    /// Remove an entry from the visible queue.
    ///
    /// Returns true when the removed entry was the current track. Callers
    /// should then invoke Player::play() to move playback to the new selection.
    pub fn remove_track(&self, position_to_remove: usize) -> bool {
        let current_position = self.position() as usize;
        let (removed_track, next_position, was_current, queue_is_empty) = {
            let mut queue = self.queue.borrow_mut();
            if position_to_remove >= queue.len() {
                return false;
            }

            let removed_track = queue.remove(position_to_remove);
            let next_position =
                position_after_remove(current_position, position_to_remove, queue.len());
            (
                removed_track,
                next_position,
                current_position == position_to_remove,
                queue.is_empty(),
            )
        };

        // The sequential queue is the source of truth when leaving Shuffle.
        // Remove the same track there as well so deleted songs cannot return.
        let mut sequential_queue = self.sequential_queue.borrow_mut();
        if let Some(index) = sequential_queue
            .iter()
            .position(|track| Rc::ptr_eq(track, &removed_track))
        {
            sequential_queue.remove(index);
        }
        drop(sequential_queue);

        if queue_is_empty {
            self.end_queue();
            return was_current;
        }

        self.current_position.set(next_position as u64);
        send!(self.sender, QueueAction::QueueUpdate);
        self.current_song_update();
        was_current
    }

    /// Swap two queue entries without changing which track is playing.
    pub fn reorder_track(&self, old_position: usize, new_position: usize) {
        if old_position == new_position {
            return;
        }

        let queue_len = self.queue.borrow().len();
        if old_position >= queue_len || new_position >= queue_len {
            return;
        }

        let current_position = self.position() as usize;
        self.queue.borrow_mut().swap(old_position, new_position);

        // Outside Shuffle, the visible queue and its restore order are the
        // same sequence. During Shuffle, keep the original sequence intact.
        if self.repeat_mode.get() != RepeatMode::Shuffle {
            let mut sequential_queue = self.sequential_queue.borrow_mut();
            if old_position < sequential_queue.len() && new_position < sequential_queue.len() {
                sequential_queue.swap(old_position, new_position);
            }
        }

        self.current_position.set(
            position_after_swap(current_position, old_position, new_position) as u64,
        );
        send!(self.sender, QueueAction::QueueUpdate);
        self.current_song_update();
    }

    pub fn on_repeat_change(&self, mode: RepeatMode) {
        let previous_mode = self.repeat_mode.get();
        let mode = if mode == previous_mode {
            RepeatMode::Normal
        } else {
            mode
        };

        match mode {
            RepeatMode::Shuffle => self.shuffle_tracks(),
            _ if previous_mode == RepeatMode::Shuffle => {
                debug!("Restoring queue from sequential");
                let current_track = self.current_track.borrow().clone();
                let restored = self.sequential_queue.borrow().clone();
                let current_position = current_track
                    .as_ref()
                    .and_then(|current| {
                        restored
                            .iter()
                            .position(|track| Rc::ptr_eq(track, current))
                    })
                    .unwrap_or_else(|| self.current_position.get() as usize)
                    .min(restored.len().saturating_sub(1));

                self.queue.replace(restored);
                self.current_position.set(current_position as u64);
                self.current_song_update();
            }
            _ => (),
        }

        self.repeat_mode.set(mode);

        let mode_int = match mode {
            RepeatMode::Normal => 0,
            RepeatMode::Loop => 1,
            RepeatMode::LoopSong => 2,
            RepeatMode::Shuffle => 3,
        };
        _ = self.settings.set_int("repeat-mode", mode_int);

        if mode == RepeatMode::Shuffle || previous_mode == RepeatMode::Shuffle {
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
        let pos = (self.position() as usize).min(queue.len());
        let total_duration = queue[pos..]
            .iter()
            .map(|track| track.duration())
            .sum();
        send!(self.sender, QueueAction::QueueDuration(total_duration));
    }

    pub fn tracks(&self) -> Vec<Rc<Track>> {
        self.queue.borrow().clone()
    }

    pub fn track_ids(&self) -> Vec<i64> {
        self.queue.borrow().iter().map(|track| track.id()).collect()
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
    use super::{position_after_remove, position_after_swap};

    #[test]
    fn removing_a_track_before_the_current_track_shifts_the_index() {
        assert_eq!(position_after_remove(3, 1, 3), 2);
    }

    #[test]
    fn removing_a_track_after_the_current_track_keeps_the_index() {
        assert_eq!(position_after_remove(1, 3, 3), 1);
    }

    #[test]
    fn removing_the_current_track_selects_the_next_or_previous_at_the_end() {
        assert_eq!(position_after_remove(2, 2, 3), 2);
        assert_eq!(position_after_remove(3, 3, 3), 2);
    }

    #[test]
    fn removing_the_only_track_resets_the_index() {
        assert_eq!(position_after_remove(0, 0, 0), 0);
    }

    #[test]
    fn swapping_a_non_current_track_preserves_the_current_index() {
        assert_eq!(position_after_swap(1, 2, 3), 1);
    }

    #[test]
    fn swapping_the_current_track_moves_its_index() {
        assert_eq!(position_after_swap(2, 2, 4), 4);
        assert_eq!(position_after_swap(4, 2, 4), 2);
    }
}
