use crate::backend::ext;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

/// Larger objects aren't downloaded for an in-app preview.
pub const MAX_PREVIEW: u64 = 100 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Image,
    Audio,
    Video,
}

/// Only formats the app can decode count as images and audio.
pub fn kind(key: &str) -> Option<Kind> {
    Some(match ext(key).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Kind::Image,
        "mp3" | "flac" | "wav" | "ogg" | "oga" | "m4a" => Kind::Audio,
        "mp4" | "m4v" | "webm" | "mov" | "mkv" | "ogv" => Kind::Video,
        _ => return None,
    })
}

fn decode(data: Arc<[u8]>, hint: &str) -> Result<Decoder<Cursor<Arc<[u8]>>>, String> {
    Decoder::builder()
        .with_byte_len(data.len() as u64)
        .with_data(Cursor::new(data))
        .with_hint(hint)
        .with_seekable(true)
        .build()
        .map_err(|e| e.to_string())
}

pub struct Audio {
    // the output device; dropping it ends playback
    _sink: MixerDeviceSink,
    player: Player,
    data: Arc<[u8]>,
    hint: String,
    pub total: Option<Duration>,
}

impl Audio {
    pub fn new(key: &str, data: Vec<u8>) -> Result<Self, String> {
        let mut sink = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        sink.log_on_drop(false);
        let player = Player::connect_new(sink.mixer());
        player.pause();
        let mut audio = Self {
            _sink: sink,
            player,
            data: data.into(),
            hint: ext(key),
            total: None,
        };
        audio.total = audio.load()?;
        Ok(audio)
    }

    fn load(&self) -> Result<Option<Duration>, String> {
        let source = decode(self.data.clone(), &self.hint)?;
        let total = source.total_duration();
        self.player.append(source);
        Ok(total)
    }

    pub fn playing(&self) -> bool {
        !self.player.empty() && !self.player.is_paused()
    }

    pub fn pos(&self) -> Duration {
        if self.player.empty() {
            Duration::ZERO
        } else {
            self.player.get_pos()
        }
    }

    pub fn toggle(&self) {
        if self.playing() {
            self.player.pause();
        } else {
            // a finished track has left the queue, so start it again
            if self.player.empty() {
                let _ = self.load();
            }
            self.player.play();
        }
    }

    pub fn seek(&self, to: Duration) {
        if self.player.empty() {
            self.player.pause();
            let _ = self.load();
        }
        let _ = self.player.try_seek(to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_extension() {
        assert!(kind("a/Photo.JPG") == Some(Kind::Image));
        assert!(kind("song.flac") == Some(Kind::Audio));
        assert!(kind("clip.webm") == Some(Kind::Video));
        assert!(kind("notes.txt").is_none());
        assert!(kind("no-extension").is_none());
        assert!(kind("dir.mp4/readme").is_none());
    }

    #[test]
    fn decodes_audio_and_images() {
        // one second of 8 kHz mono 16-bit silence
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + 16000u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes()); // fmt chunk size
        wav.extend(1u16.to_le_bytes()); // PCM
        wav.extend(1u16.to_le_bytes()); // channels
        wav.extend(8000u32.to_le_bytes()); // sample rate
        wav.extend(16000u32.to_le_bytes()); // bytes per second
        wav.extend(2u16.to_le_bytes()); // bytes per frame
        wav.extend(16u16.to_le_bytes()); // bits per sample
        wav.extend(b"data");
        wav.extend(16000u32.to_le_bytes());
        wav.resize(wav.len() + 16000, 0);
        let source = decode(wav.into(), "wav").unwrap();
        assert_eq!(source.total_duration(), Some(Duration::from_secs(1)));
        assert!(decode(vec![1, 2, 3].into(), "mp3").is_err());

        let png = image::load_from_memory(include_bytes!("../assets/icon.png")).unwrap();
        let mut jpeg = Cursor::new(Vec::new());
        png.to_rgb8()
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let back = image::load_from_memory(jpeg.get_ref()).unwrap();
        assert_eq!((back.width(), back.height()), (256, 256));
    }
}
