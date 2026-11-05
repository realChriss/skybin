use crate::backend::ext;
use crate::stream::{self, Fill, Hold, S3Cursor, S3Reader};
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use filmcraft_media::{ByteReader, FrameRequest, SharedReader, SharedSource, cancel};
use filmcraft_time::Tick;
use rodio::source::SeekError;
use rodio::{
    ChannelCount, Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate, Source,
};
use std::io::{Read, Seek};
use std::num::NonZero;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const MAX_PREVIEW: u64 = 100 * 1024 * 1024;

const MAX_FPS: f64 = 60.0;

const MAX_FRAME_WIDTH: u32 = 1280;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Image,
    Audio,
    Video,
}

pub fn kind(key: &str) -> Option<Kind> {
    Some(match ext(key).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Kind::Image,
        "mp3" | "flac" | "wav" | "ogg" | "oga" | "m4a" => Kind::Audio,
        "mp4" | "m4v" | "webm" | "mov" | "mkv" | "ogv" => Kind::Video,
        _ => return None,
    })
}

pub fn use_hardware_decoders() {
    #[cfg(target_os = "macos")]
    filmcraft_codecs::register_video_decoder(filmcraft_platform::videotoolbox_factory);
    #[cfg(target_os = "windows")]
    filmcraft_codecs::register_video_decoder(filmcraft_platform::media_foundation_factory);
    // driver probing can be slow
    #[cfg(target_os = "linux")]
    std::thread::spawn(|| {
        if filmcraft_platform::vaapi::va::probe().is_ok() {
            filmcraft_codecs::register_video_decoder(filmcraft_platform::vaapi_factory);
        }
        // registered last = tried first
        #[cfg(target_pointer_width = "64")]
        if filmcraft_platform::nvdec::cuvid::probe().is_ok() {
            filmcraft_codecs::register_video_decoder(filmcraft_platform::nvdec_factory);
        }
    });
}

// formats without a streaming demuxer are read whole, so they keep the size limit
pub fn open_video(key: &str, reader: SharedReader) -> Result<SharedSource, String> {
    let open = || {
        filmcraft_media::reader::open_reader_within(
            key,
            reader,
            &filmcraft_codecs::reader_openers(),
            &filmcraft_codecs::openers(),
            MAX_PREVIEW,
        )
    };
    match catch_unwind(AssertUnwindSafe(open)) {
        Ok(source) => source.map_err(|e| e.to_string()),
        Err(_) => Err("the video decoder crashed on this file".into()),
    }
}

pub struct Buffering {
    pub progress: f32,
    pub trouble: Option<String>,
}

pub trait Playback {
    fn playing(&self) -> bool;
    fn pos(&self) -> Duration;
    fn total(&self) -> Option<Duration>;
    fn toggle(&mut self);
    fn seek(&mut self, to: Duration);
    fn buffering(&self) -> Option<Buffering> {
        None
    }
}

// decides how much to keep downloaded and whether playback has to wait for it
struct Buffer {
    reader: Arc<S3Reader>,
    bitrate: Option<f64>,
    // a quick look at a clip stays cheap until it is first played
    engaged: bool,
    starting: bool,
    hold: Hold,
    // times playback ran out of data, which raises what it waits for before resuming
    stalls: u32,
    target: u64,
    ready: u64,
    fill: Fill,
}

impl Buffer {
    fn new(reader: Arc<S3Reader>, total: Option<Duration>) -> Self {
        let bitrate = total
            .filter(|t| !t.is_zero())
            .map(|t| reader.len() as f64 / t.as_secs_f64());
        Self {
            reader,
            bitrate,
            engaged: false,
            starting: false,
            hold: Hold::Running,
            stalls: 0,
            target: 0,
            ready: 0,
            fill: Fill::default(),
        }
    }

    // play was pressed, or a seek landed while playing
    fn start(&mut self) {
        self.engaged = true;
        self.starting = true;
    }

    fn update(&mut self, wants_play: bool, remaining: Duration) -> Hold {
        if self.engaged {
            self.target = stream::ahead_bytes(self.bitrate, self.reader.rate(), remaining);
            self.reader.set_ahead(self.target);
        }
        self.fill = self.reader.fill();
        self.ready = stream::ready_bytes(self.bitrate, self.stalls, self.target, self.fill.to_end);
        let starting = std::mem::take(&mut self.starting);
        let next = if wants_play {
            stream::hold(self.hold, starting, &self.fill, self.ready)
        } else {
            Hold::Running
        };
        if self.hold == Hold::Running && next == Hold::Buffering && !starting {
            self.stalls += 1;
        }
        self.hold = next;
        self.hold
    }

    fn status(&self) -> Option<Buffering> {
        (self.hold == Hold::Buffering).then(|| Buffering {
            progress: stream::progress(&self.fill, self.ready),
            trouble: self.fill.trouble.clone(),
        })
    }
}

// samples handed to the audio device in one go
const CHUNK: usize = 4096;
// how far sound may drift from the picture before it is fed again
const DRIFT: Duration = Duration::from_millis(250);
// how long to wait before retrying a read the network failed
const RETRY: Duration = Duration::from_millis(100);

fn sink() -> Result<MixerDeviceSink, String> {
    let mut sink = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
    sink.log_on_drop(false);
    Ok(sink)
}

// decoded on its own thread, so the audio device never waits on the network or a decoder
struct Feed {
    rx: Receiver<Vec<f32>>,
    cancel: Arc<AtomicBool>,
    played: Arc<AtomicU64>,
    rate: u32,
    channels: u16,
    buf: Vec<f32>,
    i: usize,
}

impl Feed {
    fn spawn(
        rate: u32,
        channels: u16,
        mut produce: impl FnMut() -> Option<Vec<f32>> + Send + 'static,
    ) -> Self {
        let (tx, rx) = sync_channel(8);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let _ = std::thread::Builder::new()
            .name("skybin-audio".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    let next = AssertUnwindSafe(&mut produce);
                    let chunk = cancel::with_cancel(&flag, || catch_unwind(next));
                    let Ok(Some(chunk)) = chunk else { break };
                    if tx.send(chunk).is_err() {
                        break;
                    }
                }
            });
        Self {
            rx,
            cancel,
            played: Arc::default(),
            rate,
            channels,
            buf: Vec::new(),
            i: 0,
        }
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Iterator for Feed {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.i == self.buf.len() {
            match self.rx.try_recv() {
                Ok(chunk) => {
                    self.buf = chunk;
                    self.i = 0;
                }
                // not decoded yet: silence that doesn't count as played
                Err(TryRecvError::Empty) => return Some(0.0),
                Err(TryRecvError::Disconnected) => return None,
            }
            if self.buf.is_empty() {
                return Some(0.0);
            }
        }
        let sample = self.buf[self.i];
        self.i += 1;
        if self.i.is_multiple_of(self.channels.max(1) as usize) {
            self.played.fetch_add(1, Ordering::Relaxed);
        }
        Some(sample)
    }
}

impl Source for Feed {
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

    // seeking replaces the feed
    fn try_seek(&mut self, _: Duration) -> Result<(), SeekError> {
        Err(SeekError::NotSupported {
            underlying_source: "skybin feed",
        })
    }
}

// a feed through its own player: replacing one drops the old player, which stops it without
// waiting on the audio thread (`Player::clear` does wait)
struct Voice {
    player: Player,
    cancel: Arc<AtomicBool>,
    played: Arc<AtomicU64>,
    rate: u32,
    from: Duration,
}

impl Voice {
    fn new(sink: &MixerDeviceSink, from: Duration, feed: Feed) -> Self {
        let player = Player::connect_new(sink.mixer());
        player.pause();
        let (cancel, played, rate) = (feed.cancel.clone(), feed.played.clone(), feed.rate);
        player.append(feed);
        Self {
            player,
            cancel,
            played,
            rate,
            from,
        }
    }

    fn pos(&self) -> Duration {
        let played = self.played.load(Ordering::Relaxed) as f64;
        self.from + Duration::from_secs_f64(played / self.rate.max(1) as f64)
    }

    fn started(&self) -> bool {
        self.played.load(Ordering::Relaxed) > 0
    }

    fn finished(&self) -> bool {
        self.player.empty()
    }

    fn run(&self, on: bool) {
        if on {
            self.player.play();
        } else {
            self.player.pause();
        }
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn decode<R: Read + Seek + Send + Sync + 'static>(
    data: R,
    len: u64,
    hint: &str,
) -> Result<Decoder<R>, String> {
    Decoder::builder()
        .with_byte_len(len)
        .with_data(data)
        .with_hint(hint)
        .with_seekable(true)
        .build()
        .map_err(|e| e.to_string())
}

fn decode_stream(reader: &Arc<S3Reader>, hint: &str) -> Result<Decoder<S3Cursor>, String> {
    decode(S3Cursor::patient(reader.clone()), reader.len(), hint)
}

// probing reads from the network, so it happens before the UI thread gets it; a failed read
// fails the open rather than waiting
pub struct OpenAudio {
    reader: Arc<S3Reader>,
    hint: String,
    rate: u32,
    channels: u16,
    total: Option<Duration>,
}

#[cfg(test)]
impl OpenAudio {
    pub fn total(&self) -> Option<Duration> {
        self.total
    }
}

pub fn open_audio(key: &str, reader: Arc<S3Reader>) -> Result<OpenAudio, String> {
    let hint = ext(key);
    let probe = decode(S3Cursor::new(reader.clone()), reader.len(), &hint)?;
    Ok(OpenAudio {
        rate: probe.sample_rate().get(),
        channels: probe.channels().get(),
        total: probe.total_duration(),
        reader,
        hint,
    })
}

// the decoder is made on the feed's own thread, so the UI never waits on the network
fn decoder_feed(
    reader: Arc<S3Reader>,
    hint: String,
    from: Duration,
    rate: u32,
    channels: u16,
) -> Feed {
    let mut decoder = None;
    Feed::spawn(rate, channels, move || {
        if decoder.is_none() {
            let mut d = decode_stream(&reader, &hint).ok()?;
            if !from.is_zero() {
                d.try_seek(from).ok()?;
            }
            decoder = Some(d);
        }
        let d = decoder.as_mut()?;
        let chunk: Vec<f32> = d.by_ref().take(CHUNK * channels as usize).collect();
        (!chunk.is_empty()).then_some(chunk)
    })
}

pub struct Audio {
    sink: MixerDeviceSink,
    reader: Arc<S3Reader>,
    hint: String,
    rate: u32,
    channels: u16,
    total: Option<Duration>,
    voice: Voice,
    wants_play: bool,
    buffer: Buffer,
}

impl Audio {
    pub fn new(open: OpenAudio) -> Result<Self, String> {
        let sink = sink()?;
        let OpenAudio {
            reader,
            hint,
            rate,
            channels,
            total,
        } = open;
        let feed = decoder_feed(reader.clone(), hint.clone(), Duration::ZERO, rate, channels);
        Ok(Self {
            voice: Voice::new(&sink, Duration::ZERO, feed),
            sink,
            buffer: Buffer::new(reader.clone(), total),
            reader,
            hint,
            rate,
            channels,
            total,
            wants_play: false,
        })
    }

    pub fn update(&mut self, ctx: &egui::Context) {
        if self.wants_play && self.voice.finished() {
            self.wants_play = false;
        }
        let remaining = self
            .total
            .map_or(Duration::ZERO, |t| t.saturating_sub(self.pos()));
        let hold = self.buffer.update(self.wants_play, remaining);
        self.voice.run(self.wants_play && hold == Hold::Running);
        if self.wants_play {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

impl Playback for Audio {
    fn playing(&self) -> bool {
        self.wants_play
    }

    fn pos(&self) -> Duration {
        let pos = self.voice.pos();
        self.total.map_or(pos, |t| pos.min(t))
    }

    fn total(&self) -> Option<Duration> {
        self.total
    }

    fn toggle(&mut self) {
        if self.wants_play {
            self.wants_play = false;
            self.voice.run(false);
            return;
        }
        if self.voice.finished() {
            self.seek(Duration::ZERO);
        }
        self.wants_play = true;
        self.buffer.start();
    }

    fn seek(&mut self, to: Duration) {
        let to = self.total.map_or(to, |t| to.min(t));
        self.reader.jumped();
        let feed = decoder_feed(
            self.reader.clone(),
            self.hint.clone(),
            to,
            self.rate,
            self.channels,
        );
        self.voice = Voice::new(&self.sink, to, feed);
        if self.wants_play {
            self.buffer.start();
        }
    }

    fn buffering(&self) -> Option<Buffering> {
        self.buffer.status()
    }
}

#[derive(Clone, Default)]
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

type SharedClock = Arc<Mutex<Clock>>;

fn read(clock: &SharedClock) -> Clock {
    clock.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn interleave(chunk: &filmcraft_frame::AudioBuffer, channels: u16) -> Vec<f32> {
    let mut out = Vec::with_capacity(chunk.frames() * channels as usize);
    for f in 0..chunk.frames() {
        for c in 0..channels as usize {
            let ch = chunk.channels.get(c).or(chunk.channels.first());
            out.push(ch.map_or(0.0, |ch| ch[f]));
        }
    }
    out
}

struct Sound {
    sink: MixerDeviceSink,
    rate: u32,
    channels: u16,
    voice: Option<Voice>,
}

static OPEN_VIDEOS: AtomicUsize = AtomicUsize::new(0);

pub struct Video {
    source: SharedSource,
    reader: Arc<S3Reader>,
    fps: f64,
    total: Duration,
    sound: Option<Sound>,
    clock: SharedClock,
    frames: Frames,
    next: Option<(Duration, ColorImage)>,
    texture: Option<TextureHandle>,
    ctx: egui::Context,
    wants_play: bool,
    buffer: Buffer,
}

impl Video {
    pub fn new(
        source: SharedSource,
        reader: Arc<S3Reader>,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        let info = source.info();
        let v = info.video.as_ref().ok_or("the file has no video track")?;
        let fps = v.frame_rate.num as f64 / v.frame_rate.den.max(1) as f64;
        let fps = if fps > 0.0 { fps.min(MAX_FPS) } else { MAX_FPS };
        let total = Duration::from_secs_f64(info.duration.seconds().max(0.0));
        let sound = info.audio_streams.first().and_then(|a| {
            let channels = u16::try_from(a.channels).ok().filter(|&c| c > 0)?;
            let rate = Some(a.sample_rate).filter(|&r| r > 0)?;
            Some(Sound {
                sink: sink().ok()?,
                rate,
                channels,
                voice: None,
            })
        });
        let clock = SharedClock::default();
        OPEN_VIDEOS.fetch_add(1, Ordering::Relaxed);
        let mut video = Self {
            frames: Frames::idle(),
            buffer: Buffer::new(reader.clone(), Some(total)),
            source,
            reader,
            fps,
            total,
            sound,
            clock,
            next: None,
            texture: None,
            ctx: ctx.clone(),
            wants_play: false,
        };
        video.frames = video.decode_from(Duration::ZERO);
        Ok(video)
    }

    pub fn update(&mut self) {
        if self.wants_play && self.now() >= self.total {
            self.wants_play = false;
        }
        let remaining = self.total.saturating_sub(self.pos());
        let hold = self.buffer.update(self.wants_play, remaining);
        let run = self.wants_play && hold == Hold::Running;
        match (run, self.running()) {
            (true, false) => self.run(),
            (false, true) => self.halt(),
            (true, true) => self.keep_in_sync(),
            (false, false) => {}
        }
        self.show_due_frame();
        if self.wants_play {
            let frame = Duration::from_secs_f64(1.0 / self.fps);
            self.ctx
                .request_repaint_after(frame.min(Duration::from_millis(100)));
        }
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        self.texture.as_ref()
    }

    fn show_due_frame(&mut self) {
        let now = self.pos();
        let mut due = None;
        loop {
            let frame = match self.next.take() {
                Some(f) => f,
                None => match self.frames.rx.try_recv() {
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
    }

    fn now(&self) -> Duration {
        read(&self.clock).now()
    }

    fn running(&self) -> bool {
        read(&self.clock).since.is_some()
    }

    fn clock(&self) -> std::sync::MutexGuard<'_, Clock> {
        self.clock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // sound restarts at the clock, which also undoes any drift from a stall
    fn run(&mut self) {
        self.feed_sound(self.pos());
        self.clock().play();
        if let Some(v) = self.sound.as_ref().and_then(|s| s.voice.as_ref()) {
            v.run(true);
        }
    }

    fn halt(&mut self) {
        self.clock().pause();
        if let Some(v) = self.sound.as_ref().and_then(|s| s.voice.as_ref()) {
            v.run(false);
        }
    }

    fn keep_in_sync(&mut self) {
        let now = self.now();
        let drifted = self
            .sound
            .as_ref()
            .and_then(|s| s.voice.as_ref())
            .is_some_and(|v| v.started() && !v.finished() && v.pos().abs_diff(now) > DRIFT);
        if drifted {
            self.run();
        }
    }

    fn feed_sound(&mut self, from: Duration) {
        let Some(s) = &mut self.sound else { return };
        let (source, reader) = (self.source.clone(), self.reader.clone());
        let (rate, channels) = (s.rate, s.channels);
        let mut next = (from.as_secs_f64() * rate as f64) as i64;
        let end = (self.total.as_secs_f64() * rate as f64).ceil() as i64;
        let feed = Feed::spawn(rate, channels, move || {
            loop {
                let n = (end - next).min(CHUNK as i64);
                if n <= 0 {
                    return None;
                }
                match source.audio(next, n as usize, rate) {
                    Ok(chunk) if chunk.frames() == 0 => return None,
                    Ok(chunk) => {
                        next += n;
                        return Some(interleave(&chunk, channels));
                    }
                    Err(_) if cancel::cancelled() => return None,
                    Err(_) if reader.troubled() => std::thread::sleep(RETRY),
                    // undecodable: skip it rather than stop the sound
                    Err(_) => {
                        next += n;
                        return Some(vec![0.0; n as usize * channels as usize]);
                    }
                }
            }
        });
        s.voice = Some(Voice::new(&s.sink, from, feed));
    }

    fn decode_from(&self, from: Duration) -> Frames {
        let reader = self.reader.clone();
        decode_frames(
            self.source.clone(),
            from,
            self.fps,
            self.total,
            self.clock.clone(),
            self.ctx.clone(),
            move || reader.troubled(),
        )
    }
}

impl Playback for Video {
    fn playing(&self) -> bool {
        self.wants_play
    }

    fn pos(&self) -> Duration {
        self.now().min(self.total)
    }

    fn total(&self) -> Option<Duration> {
        Some(self.total)
    }

    fn toggle(&mut self) {
        if self.wants_play {
            self.wants_play = false;
            self.halt();
            return;
        }
        if self.pos() >= self.total {
            self.seek(Duration::ZERO);
        }
        self.wants_play = true;
        self.buffer.start();
    }

    fn seek(&mut self, to: Duration) {
        let to = to.min(self.total);
        self.halt();
        // its thread would keep reading at the old spot and pull the download window back
        if let Some(s) = &mut self.sound {
            s.voice = None;
        }
        self.clock().set(to);
        self.reader.jumped();
        // dropping the old Frames cancels its in-flight decode
        self.frames = self.decode_from(to);
        self.next = None;
        if self.wants_play {
            self.buffer.start();
        }
    }

    fn buffering(&self) -> Option<Buffering> {
        self.buffer.status()
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        let last = OPEN_VIDEOS.fetch_sub(1, Ordering::Relaxed) == 1;
        let thread = self.frames.stop();
        if last {
            std::thread::spawn(move || {
                if let Some(t) = thread {
                    let _ = t.join();
                }
                if OPEN_VIDEOS.load(Ordering::Relaxed) == 0 {
                    filmcraft_codecs::release_idle();
                    filmcraft_frame::pool::trim_planes();
                }
            });
        }
    }
}

struct Frames {
    rx: Receiver<(Duration, ColorImage)>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Frames {
    fn idle() -> Self {
        Self {
            rx: sync_channel(0).1,
            cancel: Arc::default(),
            thread: None,
        }
    }

    fn stop(&mut self) -> Option<JoinHandle<()>> {
        self.cancel.store(true, Ordering::Relaxed);
        self.thread.take()
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.stop();
    }
}

fn decimation(width: u32) -> usize {
    width.div_ceil(MAX_FRAME_WIDTH).max(1).next_power_of_two() as usize
}

fn plan_frame(next: i64, fps: f64, clock: &Clock) -> (i64, Option<Tick>) {
    if clock.since.is_none() {
        return (next, None);
    }
    let now = clock.now().as_secs_f64();
    let due = (now * fps).floor() as i64;
    let i = next.max(due);
    let ahead = (i as f64 / fps - now).max(0.0);
    (i, Some(Tick::from_seconds_f64(ahead)))
}

// `trouble`: whether a failed decode was the network, which is retried, rather than bad data,
// which is skipped
fn decode_frames(
    source: SharedSource,
    from: Duration,
    fps: f64,
    total: Duration,
    clock: SharedClock,
    ctx: egui::Context,
    trouble: impl Fn() -> bool + Send + 'static,
) -> Frames {
    let (tx, rx) = sync_channel(4);
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let thread = std::thread::spawn(move || {
        let mut next = (from.as_secs_f64() * fps).floor() as i64;
        loop {
            let (i, late) = plan_frame(next, fps, &read(&clock));
            let t = i as f64 / fps;
            if t > total.as_secs_f64() || flag.load(Ordering::Relaxed) {
                break;
            }
            let request = FrameRequest::full(Tick::from_seconds_f64(t));
            let decode = || cancel::with_catch_up(late, || source.video_frame(request));
            let frame = match cancel::with_cancel(&flag, decode) {
                Ok(frame) => frame,
                Err(_) if flag.load(Ordering::Relaxed) => break,
                Err(_) if trouble() => {
                    std::thread::sleep(RETRY);
                    continue;
                }
                Err(_) => {
                    next = i + 1;
                    continue;
                }
            };
            let small = frame.box_decimated(decimation(frame.width));
            let frame = small.as_ref().unwrap_or(&frame);
            let size = [frame.width as usize, frame.height as usize];
            let image = ColorImage::from_rgba_unmultiplied(size, &frame.to_rgba8());
            if tx.send((Duration::from_secs_f64(t), image)).is_err() {
                break;
            }
            ctx.request_repaint();
            next = i + 1;
        }
    });
    Frames {
        rx,
        cancel,
        thread: Some(thread),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

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
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + 16000u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(8000u32.to_le_bytes());
        wav.extend(16000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(16000u32.to_le_bytes());
        wav.resize(wav.len() + 16000, 0);
        let len = wav.len() as u64;
        let source = decode(Cursor::new(wav), len, "wav").unwrap();
        assert_eq!(source.total_duration(), Some(Duration::from_secs(1)));
        assert!(decode(Cursor::new(vec![1, 2, 3]), 3, "mp3").is_err());

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
        let mem = |b: Vec<u8>| Arc::new(filmcraft_media::reader::MemReader(b.into()));
        assert!(open_video("clip.mp4", mem(b"not a video".to_vec())).is_err());
        let mut truncated = b"\0\0\0\x20ftypisom".to_vec();
        truncated.resize(64, 0xff);
        assert!(open_video("clip.mp4", mem(truncated)).is_err());
    }

    #[test]
    fn frames_shrink_only_past_the_width_limit() {
        assert_eq!(decimation(0), 1);
        assert_eq!(decimation(640), 1);
        assert_eq!(decimation(1280), 1);
        assert_eq!(decimation(1281), 2);
        assert_eq!(decimation(1920), 2);
        assert_eq!(decimation(2560), 2);
        assert_eq!(decimation(3840), 4);
        assert_eq!(decimation(7680), 8);
    }

    #[test]
    fn playback_skips_frames_that_are_already_late() {
        let mut clock = Clock::default();
        clock.set(Duration::from_secs(10));
        assert_eq!(plan_frame(3, 30.0, &clock), (3, None));
        clock.play();
        let (i, late) = plan_frame(3, 30.0, &clock);
        assert!((300..=301).contains(&i));
        assert!(late.unwrap() < Tick::from_seconds_f64(1.0 / 30.0));
        let (i, late) = plan_frame(330, 30.0, &clock);
        assert_eq!(i, 330);
        let ahead = late.unwrap().seconds();
        assert!(ahead > 0.9 && ahead <= 1.0, "{ahead}");
    }

    struct Stuck {
        info: filmcraft_media::MediaInfo,
        asked: Mutex<Vec<(Tick, Option<Tick>)>>,
        cancelled: AtomicBool,
    }

    impl filmcraft_media::MediaSource for Stuck {
        fn info(&self) -> &filmcraft_media::MediaInfo {
            &self.info
        }

        fn video_frame(
            &self,
            req: FrameRequest,
        ) -> filmcraft_media::Result<Arc<filmcraft_frame::VideoFrame>> {
            self.asked
                .lock()
                .unwrap()
                .push((req.time, cancel::catch_up()));
            let give_up = Instant::now() + Duration::from_secs(5);
            while Instant::now() < give_up {
                if cancel::cancelled() {
                    self.cancelled.store(true, Ordering::Relaxed);
                    return Err(filmcraft_media::MediaError::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(filmcraft_media::MediaError::Decode(
                "never cancelled".into(),
            ))
        }

        fn audio(
            &self,
            _start: i64,
            frames: usize,
            rate: u32,
        ) -> filmcraft_media::Result<filmcraft_frame::AudioBuffer> {
            Ok(filmcraft_frame::AudioBuffer::silence(rate, 2, frames))
        }
    }

    #[test]
    fn seeking_cancels_the_frame_being_decoded() {
        let stuck = Arc::new(Stuck {
            info: filmcraft_media::MediaInfo {
                name: "stuck.mp4".into(),
                kind: filmcraft_media::MediaKind::Movie,
                duration: Tick::from_seconds_f64(60.0),
                video: None,
                audio_streams: Vec::new(),
                container: String::new(),
                start_timecode: None,
                file_size: None,
            },
            asked: Mutex::new(Vec::new()),
            cancelled: AtomicBool::new(false),
        });
        let clock = SharedClock::default();
        clock.lock().unwrap().set(Duration::from_secs(10));
        clock.lock().unwrap().play();
        let source: SharedSource = stuck.clone();
        let mut frames = decode_frames(
            source,
            Duration::ZERO,
            30.0,
            Duration::from_secs(60),
            clock,
            egui::Context::default(),
            || false,
        );
        let wait = |what: &dyn Fn() -> bool| {
            let give_up = Instant::now() + Duration::from_secs(2);
            while !what() && Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(1));
            }
            what()
        };
        assert!(wait(&|| !stuck.asked.lock().unwrap().is_empty()));
        let (t, late) = stuck.asked.lock().unwrap()[0];
        assert!(t.seconds() >= 10.0, "{}", t.seconds());
        assert!(late.is_some());
        let thread = frames.stop();
        drop(frames);
        assert!(wait(&|| stuck.cancelled.load(Ordering::Relaxed)));
        thread.unwrap().join().unwrap();
    }

    fn drain(feed: &mut Feed) -> Vec<f32> {
        let give_up = Instant::now() + Duration::from_secs(2);
        let mut out = Vec::new();
        while Instant::now() < give_up {
            match feed.next() {
                Some(0.0) => std::thread::sleep(Duration::from_millis(1)),
                Some(x) => out.push(x),
                None => return out,
            }
        }
        panic!("the feed never ended: {out:?}");
    }

    #[test]
    fn a_feed_plays_chunks_in_order_and_counts_only_real_frames() {
        let mut chunks = vec![vec![1.0, 2.0, 3.0, 4.0], vec![5.0, 6.0]].into_iter();
        let mut feed = Feed::spawn(48_000, 2, move || chunks.next());
        let played = feed.played.clone();
        assert_eq!(drain(&mut feed), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(played.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn a_starved_feed_plays_silence_and_dropping_it_stops_the_producer() {
        let started = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (b, s) = (started.clone(), stopped.clone());
        let mut feed = Feed::spawn(48_000, 1, move || {
            b.store(true, Ordering::Relaxed);
            while !cancel::cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            s.store(true, Ordering::Relaxed);
            None
        });
        for _ in 0..100 {
            assert_eq!(feed.next(), Some(0.0));
        }
        assert_eq!(feed.played.load(Ordering::Relaxed), 0);
        let give_up = Instant::now() + Duration::from_secs(2);
        while !started.load(Ordering::Relaxed) && Instant::now() < give_up {
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(feed);
        while !stopped.load(Ordering::Relaxed) && Instant::now() < give_up {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(stopped.load(Ordering::Relaxed));
    }

    struct Gate {
        data: Arc<[u8]>,
        open: Arc<AtomicBool>,
    }

    impl stream::Fetch for Gate {
        fn fetch(&self, start: u64, end: u64) -> crate::backend::Res<Vec<u8>> {
            let give_up = Instant::now() + Duration::from_secs(5);
            while !self.open.load(Ordering::Relaxed) && Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(self.data[start as usize..end as usize].to_vec())
        }
    }

    #[test]
    fn playback_starts_early_and_waits_longer_after_running_dry() {
        let block = stream::BLOCK;
        let open = Arc::new(AtomicBool::new(true));
        let gate = Gate {
            data: vec![0; 30 * block as usize].into(),
            open: open.clone(),
        };
        let reader = S3Reader::with_fetch(Box::new(gate), 30 * block, None);
        let total = Duration::from_secs(30);
        let mut buffer = Buffer::new(reader.clone(), Some(total));
        reader.read_at(0, &mut [0; 1]).unwrap();
        buffer.start();
        let run_until = |buffer: &mut Buffer, want: Hold| {
            let give_up = Instant::now() + Duration::from_secs(2);
            while buffer.update(true, total) != want && Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(1));
            }
            buffer.hold
        };
        assert_eq!(run_until(&mut buffer, Hold::Running), Hold::Running);
        let first = buffer.ready;
        assert!(first < 5 * block, "{first}");
        assert_eq!(buffer.stalls, 0);

        open.store(false, Ordering::Relaxed);
        let r = reader.clone();
        let read = std::thread::spawn(move || r.read_at(20 * block, &mut [0; 1]));
        assert_eq!(run_until(&mut buffer, Hold::Buffering), Hold::Buffering);
        assert_eq!(buffer.stalls, 1);
        buffer.update(true, total);
        assert_eq!(buffer.ready, 2 * first);

        open.store(true, Ordering::Relaxed);
        read.join().unwrap().unwrap();
        assert_eq!(run_until(&mut buffer, Hold::Running), Hold::Running);
        assert_eq!(buffer.stalls, 1);
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
