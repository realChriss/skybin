use crate::backend::ext;
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use filmcraft_media::{FrameRequest, SharedSource};
use filmcraft_time::Tick;
use rodio::source::SeekError;
use rodio::{
    ChannelCount, Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate, Source,
};
use std::io::Cursor;
use std::num::NonZero;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::{Duration, Instant};

pub const MAX_PREVIEW: u64 = 100 * 1024 * 1024;

/// Frames above this rate are still decoded (each builds on the previous ones), just not shown.
const MAX_FPS: f64 = 30.0;

/// Frames are shrunk to at most this width before display: the panel is far narrower, and a
/// full 4K frame would be 33 MB to upload 30 times a second.
const MAX_FRAME_WIDTH: u32 = 1280;

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

/// Parses the file; FilmCraft is new, so a crash on a damaged file becomes an error instead.
pub fn open_video(key: &str, data: Vec<u8>) -> Result<SharedSource, String> {
    let open = || filmcraft_media::open_bytes(key, data.into(), &filmcraft_codecs::openers());
    match catch_unwind(AssertUnwindSafe(open)) {
        Ok(source) => source.map_err(|e| e.to_string()),
        Err(_) => Err("the video decoder crashed on this file".into()),
    }
}

pub trait Playback {
    fn playing(&self) -> bool;
    fn pos(&self) -> Duration;
    fn total(&self) -> Option<Duration>;
    fn toggle(&mut self);
    fn seek(&mut self, to: Duration);
}

/// Dropping the returned device ends playback, so it must live as long as the player.
fn output() -> Result<(MixerDeviceSink, Player), String> {
    let mut sink = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
    sink.log_on_drop(false);
    let player = Player::connect_new(sink.mixer());
    player.pause();
    Ok((sink, player))
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
    _sink: MixerDeviceSink,
    player: Player,
    data: Arc<[u8]>,
    hint: String,
    total: Option<Duration>,
}

impl Audio {
    pub fn new(key: &str, data: Vec<u8>) -> Result<Self, String> {
        let (sink, player) = output()?;
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
}

impl Playback for Audio {
    fn playing(&self) -> bool {
        !self.player.empty() && !self.player.is_paused()
    }

    fn pos(&self) -> Duration {
        if self.player.empty() {
            Duration::ZERO
        } else {
            self.player.get_pos()
        }
    }

    fn total(&self) -> Option<Duration> {
        self.total
    }

    fn toggle(&mut self) {
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

    fn seek(&mut self, to: Duration) {
        if self.player.empty() {
            self.player.pause();
            let _ = self.load();
        }
        let _ = self.player.try_seek(to);
    }
}

/// Wall-clock playback position; video frames and the sound track both follow it.
#[derive(Default)]
struct Clock {
    base: Duration,
    since: Option<Instant>,
}

impl Clock {
    fn now(&self) -> Duration {
        self.base + self.since.map_or(Duration::ZERO, |s| s.elapsed())
    }

    fn play(&mut self) {
        self.since.get_or_insert_with(Instant::now);
    }

    fn pause(&mut self) {
        self.base = self.now();
        self.since = None;
    }

    fn set(&mut self, to: Duration) {
        self.base = to;
        if self.since.is_some() {
            self.since = Some(Instant::now());
        }
    }
}

/// A video's sound track, decoded a few thousand frames at a time as rodio pulls samples.
struct Track {
    source: SharedSource,
    rate: u32,
    channels: u16,
    next: i64,
    end: i64,
    buf: Vec<f32>,
    i: usize,
}

impl Track {
    fn new(
        source: SharedSource,
        rate: u32,
        channels: u16,
        from: Duration,
        total: Duration,
    ) -> Self {
        Self {
            next: (from.as_secs_f64() * rate as f64) as i64,
            end: (total.as_secs_f64() * rate as f64).ceil() as i64,
            source,
            rate,
            channels,
            buf: Vec::new(),
            i: 0,
        }
    }
}

impl Iterator for Track {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.i == self.buf.len() {
            let n = (self.end - self.next).min(4096);
            if n <= 0 {
                return None;
            }
            // this runs on the audio device's thread, where a panic can abort the whole app
            let source = &self.source;
            let (next, rate) = (self.next, self.rate);
            let decode = AssertUnwindSafe(|| source.audio(next, n as usize, rate));
            let chunk = catch_unwind(decode).ok()?.ok()?;
            self.next += n;
            self.buf.clear();
            self.i = 0;
            for f in 0..chunk.frames() {
                for c in 0..self.channels as usize {
                    // a stream with fewer channels than announced repeats its first one
                    let ch = chunk.channels.get(c).or(chunk.channels.first());
                    self.buf.push(ch.map_or(0.0, |ch| ch[f]));
                }
            }
            if self.buf.is_empty() {
                return None;
            }
        }
        self.i += 1;
        Some(self.buf[self.i - 1])
    }
}

impl Source for Track {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        NonZero::new(self.channels).unwrap_or(NonZero::<u16>::MIN)
    }

    fn sample_rate(&self) -> SampleRate {
        NonZero::new(self.rate).unwrap_or(NonZero::<u32>::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.next = (pos.as_secs_f64() * self.rate as f64) as i64;
        self.buf.clear();
        self.i = 0;
        Ok(())
    }
}

struct Sound {
    _sink: MixerDeviceSink,
    player: Player,
    rate: u32,
    channels: u16,
}

pub struct Video {
    source: SharedSource,
    fps: f64,
    total: Duration,
    sound: Option<Sound>,
    clock: Clock,
    frames: Receiver<(Duration, ColorImage)>,
    next: Option<(Duration, ColorImage)>,
    texture: Option<TextureHandle>,
    ctx: egui::Context,
}

impl Video {
    /// Takes a source from [`open_video`], so no file parsing happens on the UI thread.
    pub fn new(source: SharedSource, ctx: &egui::Context) -> Result<Self, String> {
        let info = source.info();
        let v = info.video.as_ref().ok_or("the file has no video track")?;
        let fps = v.frame_rate.num as f64 / v.frame_rate.den.max(1) as f64;
        let fps = if fps > 0.0 { fps.min(MAX_FPS) } else { MAX_FPS };
        let total = Duration::from_secs_f64(info.duration.seconds().max(0.0));
        // no sound card, or no sound track, just means a silent video
        let sound = info.audio_streams.first().and_then(|a| {
            let channels = u16::try_from(a.channels).ok().filter(|&c| c > 0)?;
            let (sink, player) = output().ok()?;
            let rate = a.sample_rate;
            player.append(Track::new(
                source.clone(),
                rate,
                channels,
                Duration::ZERO,
                total,
            ));
            Some(Sound {
                _sink: sink,
                player,
                rate,
                channels,
            })
        });
        Ok(Self {
            frames: decode_frames(source.clone(), Duration::ZERO, fps, total, ctx.clone()),
            source,
            fps,
            total,
            sound,
            clock: Clock::default(),
            next: None,
            texture: None,
            ctx: ctx.clone(),
        })
    }

    /// Shows the newest decoded frame that's due; call once per UI frame.
    pub fn update(&mut self) {
        if self.playing() && self.clock.now() >= self.total {
            self.pause();
        }
        let now = self.pos();
        let mut due = None;
        loop {
            let frame = match self.next.take() {
                Some(f) => f,
                None => match self.frames.try_recv() {
                    Ok(f) => f,
                    Err(_) => break,
                },
            };
            if frame.0 > now {
                self.next = Some(frame);
                break;
            }
            due = Some(frame.1);
        }
        if let Some(image) = due {
            match &mut self.texture {
                Some(t) => t.set(image, TextureOptions::LINEAR),
                None => {
                    let t = self
                        .ctx
                        .load_texture("video", image, TextureOptions::LINEAR);
                    self.texture = Some(t);
                }
            }
        }
        if self.playing() {
            self.ctx
                .request_repaint_after(Duration::from_secs_f64(1.0 / self.fps));
        }
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        self.texture.as_ref()
    }

    fn pause(&mut self) {
        self.clock.pause();
        if let Some(s) = &self.sound {
            s.player.pause();
        }
    }

    fn sync_sound(&self, at: Duration) {
        let Some(s) = &self.sound else { return };
        if s.player.empty() {
            let track = Track::new(self.source.clone(), s.rate, s.channels, at, self.total);
            s.player.append(track);
        } else {
            let _ = s.player.try_seek(at);
        }
    }
}

impl Playback for Video {
    fn playing(&self) -> bool {
        self.clock.since.is_some()
    }

    fn pos(&self) -> Duration {
        self.clock.now().min(self.total)
    }

    fn total(&self) -> Option<Duration> {
        Some(self.total)
    }

    fn toggle(&mut self) {
        if self.playing() {
            self.pause();
            return;
        }
        if self.pos() >= self.total {
            self.seek(Duration::ZERO);
        }
        self.sync_sound(self.pos());
        self.clock.play();
        if let Some(s) = &self.sound {
            s.player.play();
        }
    }

    fn seek(&mut self, to: Duration) {
        let to = to.min(self.total);
        self.clock.set(to);
        // the old decoder thread stops once its receiver is gone
        self.frames = decode_frames(
            self.source.clone(),
            to,
            self.fps,
            self.total,
            self.ctx.clone(),
        );
        self.next = None;
        self.sync_sound(to);
    }
}

fn decode_frames(
    source: SharedSource,
    from: Duration,
    fps: f64,
    total: Duration,
    ctx: egui::Context,
) -> Receiver<(Duration, ColorImage)> {
    let (tx, rx) = sync_channel(4);
    std::thread::spawn(move || {
        let first = (from.as_secs_f64() * fps).floor() as i64;
        for i in first.. {
            let t = i as f64 / fps;
            if t > total.as_secs_f64() {
                break;
            }
            let Ok(frame) = source.video_frame(FrameRequest::full(Tick::from_seconds_f64(t)))
            else {
                break;
            };
            let factor = (frame.width / MAX_FRAME_WIDTH + 1).next_power_of_two() as usize;
            let small = frame.box_decimated(factor);
            let frame = small.as_ref().unwrap_or(&frame);
            let size = [frame.width as usize, frame.height as usize];
            let image = ColorImage::from_rgba_unmultiplied(size, &frame.to_rgba8());
            if tx.send((Duration::from_secs_f64(t), image)).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    rx
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

    #[test]
    fn broken_video_is_an_error() {
        assert!(open_video("clip.mp4", b"not a video".to_vec()).is_err());
        let mut truncated = b"\0\0\0\x20ftypisom".to_vec();
        truncated.resize(64, 0xff);
        assert!(open_video("clip.mp4", truncated).is_err());
    }

    #[test]
    fn clock_pauses_and_seeks() {
        let mut c = Clock::default();
        assert_eq!(c.now(), Duration::ZERO);
        c.set(Duration::from_secs(5));
        assert_eq!(c.now(), Duration::from_secs(5));
        c.play();
        std::thread::sleep(Duration::from_millis(20));
        c.pause();
        let paused = c.now();
        assert!(paused >= Duration::from_millis(5020));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(c.now(), paused);
    }
}
