<img src="/data/icons/icon.svg" align="left" height="250px" vspace="10px">

Resonance
======

Harmonize your listening experience with Resonance. 

Resonance is an intuitive music player application written primarily in Rust, with a clean user interface built using GTK4 / Libadwaita and audio playback powered by GStreamer. 
Resonance lets you effortlessly manage and play your music collection, with support for all the common music file formats such as MP3, FLAC, AAC, and many more.

⚠️ RESONANCE IS EARLY STAGE ALPHA RELEASE SOFTWARE (THERE ARE BUGS, PLEASE OPEN ISSUES) ⚠️

<br><br>

<img src="/data/screenshots/resonance2.png" align="right" height="500px">

Features:

 - UI updates to reflect currently playing track's cover art colors
 - Playlist creation & modification
 - Control the player through [MPRIS](https://specifications.freedesktop.org/mpris-spec/latest/)
 - Discord Rich Presence integration
 - Last.fm scrobbling
 - Parallel native-Rust library scanning and tag extraction (Lofty + Rayon)
 - Native metadata and embedded artwork support for MP3, FLAC, Ogg Vorbis, Opus, MP4/M4A, AAC, AIFF, WAV, APE, Musepack and WavPack
 - Mutagen fallback for legacy ASF/WMA/WMV files
 - No tag editing (intentionally out of scope to keep Resonance a music player only)

<br><br>


Flatpak
--------------

You can install stable builds of Resonance from [Flathub](https://flathub.org)
by using this command:

    flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
    flatpak install flathub io.github.nate_xyz.Resonance

<a href="https://beta.flathub.org/apps/io.github.nate_xyz.Resonance"><img src="https://flathub.org/assets/badges/flathub-badge-en.png" width="200"/></a>

Building from source
----------------------

To build from source you need the GStreamer, GTK4 and Libadwaita development headers. Additionally you'll need Meson, Ninja, Cargo and Git. Python, Mutagen, tqdm and loguru are retained only for the legacy ASF/WMA/WMV metadata fallback.

On a recent enough Ubuntu or Debian unstable you can do the following:

```
sudo apt install build-essential rustc meson ninja-build git python3-loguru python3-mutagen python3-tqdm libgtk-4-dev libadwaita-1-dev \
libgstreamer-plugins-bad1.0-dev libgstreamer-plugins-base1.0-dev libgstreamer1.0-dev
git clone https://github.com/nate-xyz/resonance && cd resonance
ninja -C build && sudo ninja -C build install

```
To uninstall run ```sudo ninja -C build uninstall```.

## Arch Linux
Initial script provided for 0.1.3.

Available in the [AUR.](https://aur.archlinux.org/packages/resonance)

Or manually download `PKGBUILD` file and run:
```
$ makepkg -sic
```


Screenshots
--------------
![Screenshot 1](./data/screenshots/resonance3.png)
![Screenshot 2](./data/screenshots/resonance4.png)
![Screenshot 3](./data/screenshots/resonance5.png)
![Screenshot 4](./data/screenshots/resonance6.png)
![Screenshot 5](./data/screenshots/resonance7.png)
![Screenshot 6](./data/screenshots/resonance8.png)
![Screenshot 7](./data/screenshots/resonance9.png)





## Native importer

The main library scan is performed in Rust. Resonance walks the selected folder once, parses each supported file with Lofty in a bounded Rayon worker pool, and only uses embedded Python/Mutagen for legacy ASF/WMA/WMV files. Queue edits keep the shuffled and sequential lists consistent, and fuzzy matching reuses a matcher across filter/sort operations.

The Rust importer currently recognises MP3, AIFF, Ogg Vorbis, Opus, FLAC, MP4/M4A, AAC, APE, Musepack, WAV and WavPack. Embedded cover art and the core title/artist/album/genre/date/track/disc tags are imported where present.


## Playback processing

When the GStreamer Good Plug-ins ReplayGain filter (`rgvolume`) is installed, Resonance uses embedded ReplayGain/R128 tags to normalize track loudness. Album gain is preferred by default; the Playback preferences let users switch to track-level gain. The filter includes peak-aware clipping prevention. If the plugin is unavailable, normal playback continues without loudness normalization.
