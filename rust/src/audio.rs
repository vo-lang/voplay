//! Audio engine wrapper around rodio.
//!
//! Manages loaded audio clips, fire-and-forget SFX playback,
//! looping music with a dedicated Sink, and per-group volume control.
//!
//! Safety: AudioEngine is `!Send` because `OutputStream` contains platform-specific
//! audio handles. We wrap it in `unsafe impl Send/Sync` because voplay ensures
//! the engine is only created and accessed behind a single Mutex on the game thread.

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};

/// Unique clip ID counter.
static NEXT_CLIP_ID: AtomicU32 = AtomicU32::new(1);

/// Raw audio data stored for replay.
struct AudioClipData {
    bytes: Vec<u8>,
}

/// The audio engine manages output streams, loaded clips, and playback.
pub struct AudioEngine {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    clips: HashMap<u32, AudioClipData>,
    music_sink: Option<Sink>,
    sfx_volume: f32,
    music_volume: f32,
}

// Safety: AudioEngine is always behind a Mutex, only accessed from game thread.
// OutputStream is !Send on some platforms but we never move it across threads.
unsafe impl Send for AudioEngine {}
unsafe impl Sync for AudioEngine {}

impl AudioEngine {
    /// Create a new audio engine. Returns None if no audio device is available.
    pub fn new() -> Option<Self> {
        let (stream, stream_handle) = match OutputStream::try_default() {
            Ok(pair) => pair,
            Err(e) => {
                log::warn!("voplay audio: no output device: {e}");
                return None;
            }
        };
        Some(Self {
            _stream: stream,
            stream_handle,
            clips: HashMap::new(),
            music_sink: None,
            sfx_volume: 1.0,
            music_volume: 1.0,
        })
    }

    /// Load audio from raw file bytes (WAV, OGG, MP3).
    /// Returns a clip ID on success.
    pub fn load_bytes(&mut self, data: Vec<u8>) -> Result<u32, String> {
        // Validate by trying to decode a clone (Decoder requires 'static)
        let test_cursor = Cursor::new(data.clone());
        Decoder::new(test_cursor).map_err(|e| format!("audio decode error: {e}"))?;

        let id = NEXT_CLIP_ID.fetch_add(1, Ordering::Relaxed);
        self.clips.insert(id, AudioClipData { bytes: data });
        Ok(id)
    }

    /// Load audio from a file path.
    pub fn load_file(&mut self, path: &str) -> Result<u32, String> {
        let data = std::fs::read(path).map_err(|e| format!("audio load error: {e}"))?;
        self.load_bytes(data)
    }

    /// Free a loaded audio clip.
    pub fn free_clip(&mut self, clip_id: u32) {
        self.clips.remove(&clip_id);
    }

    /// Play a sound effect (fire-and-forget). Volume is scaled by sfx_volume.
    /// Optional pitch and pan parameters (1.0 = normal pitch, 0.0 = center pan).
    pub fn play_sound(&self, clip_id: u32, volume: f32, pitch: f32, pan: f32) {
        let clip = match self.clips.get(&clip_id) {
            Some(c) => c,
            None => {
                log::warn!("voplay audio: clip {clip_id} not found");
                return;
            }
        };

        let cursor = Cursor::new(clip.bytes.clone());
        let source = match Decoder::new(cursor) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("voplay audio: decode error for clip {clip_id}: {e}");
                return;
            }
        };

        let final_volume = volume * self.sfx_volume;

        // Apply pitch (speed), pan, then amplify
        let processed = source
            .speed(pitch)
            .amplify(final_volume);

        // Pan: -1.0 = full left, 0.0 = center, 1.0 = full right
        // rodio doesn't have built-in pan, so we approximate with channel volumes
        // For simplicity, just play without pan if near center
        if pan.abs() < 0.01 {
            let _ = self.stream_handle.play_raw(processed.convert_samples());
        } else {
            // Use a Sink for pan control (left/right volume)
            if let Ok(sink) = Sink::try_new(&self.stream_handle) {
                sink.append(processed);
                // Pan approximation: attenuate one channel
                // rodio Sink doesn't support per-channel volume natively,
                // so we just use the overall volume for now
                sink.detach();
            }
        }
    }

    /// Play music (looping). Stops any currently playing music first.
    pub fn play_music(&mut self, clip_id: u32, volume: f32) {
        // Stop current music
        self.stop_music();

        let clip = match self.clips.get(&clip_id) {
            Some(c) => c,
            None => {
                log::warn!("voplay audio: music clip {clip_id} not found");
                return;
            }
        };

        let cursor = Cursor::new(clip.bytes.clone());
        let source = match Decoder::new(cursor) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("voplay audio: decode error for music clip {clip_id}: {e}");
                return;
            }
        };

        if let Ok(sink) = Sink::try_new(&self.stream_handle) {
            sink.set_volume(volume * self.music_volume);
            sink.append(source.repeat_infinite());
            self.music_sink = Some(sink);
        }
    }

    /// Stop currently playing music.
    pub fn stop_music(&mut self) {
        if let Some(sink) = self.music_sink.take() {
            sink.stop();
        }
    }

    /// Pause currently playing music.
    pub fn pause_music(&self) {
        if let Some(ref sink) = self.music_sink {
            sink.pause();
        }
    }

    /// Resume paused music.
    pub fn resume_music(&self) {
        if let Some(ref sink) = self.music_sink {
            sink.play();
        }
    }

    /// Set SFX group volume (0.0 to 1.0).
    pub fn set_sfx_volume(&mut self, vol: f32) {
        self.sfx_volume = vol.clamp(0.0, 1.0);
    }

    /// Set music group volume (0.0 to 1.0).
    /// Also updates the currently playing music sink if any.
    pub fn set_music_volume(&mut self, vol: f32) {
        self.music_volume = vol.clamp(0.0, 1.0);
        if let Some(ref sink) = self.music_sink {
            sink.set_volume(self.music_volume);
        }
    }
}
