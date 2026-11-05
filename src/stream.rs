use crate::backend::{self, Res, Slot};
use filmcraft_media::{ByteReader, cancel};
use s3::Bucket;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;

pub const BLOCK: u64 = 1024 * 1024;
// blocks kept in memory; the window ahead lives on disk
const KEEP: usize = 16;
// enough for opening and the first frames, until the player sets a target
const START_AHEAD: u64 = 2;
const MAX_INFLIGHT: usize = 4;
const RETRY_AFTER: Duration = Duration::from_secs(2);
// an unresolved failure this recent means the network is in trouble
const TROUBLE_FOR: Duration = Duration::from_secs(10);

pub trait Fetch: Send + Sync + 'static {
    fn fetch(&self, start: u64, end: u64) -> Res<Vec<u8>>;
}

struct S3Fetch {
    bucket: Box<Bucket>,
    key: String,
    rt: Handle,
}

impl Fetch for S3Fetch {
    fn fetch(&self, start: u64, end: u64) -> Res<Vec<u8>> {
        let get = backend::get_range(&self.bucket, &self.key, start, end);
        self.rt.block_on(get)
    }
}

// reads an object in blocks: memory, then the disk cache, then ranged GETs, and keeps the
// window ahead of the latest read downloaded
pub struct S3Reader {
    inner: Arc<Inner>,
}

struct Inner {
    len: u64,
    fetch: Box<dyn Fetch>,
    disk: Option<Slot>,
    state: Mutex<State>,
    landed: Condvar,
}

#[derive(Default)]
struct State {
    mem: HashMap<u64, Arc<[u8]>>,
    recent: VecDeque<u64>,
    stored: HashSet<u64>,
    loading: HashSet<u64>,
    failed: HashMap<u64, (String, Instant)>,
    anchor: Option<u64>,
    ahead: u64,
    waiting: usize,
    meter: Meter,
    closed: bool,
}

impl State {
    fn present(&self, i: u64) -> bool {
        self.mem.contains_key(&i) || self.stored.contains(&i)
    }

    fn remember(&mut self, i: u64, data: Arc<[u8]>) {
        if self.mem.insert(i, data).is_some() {
            self.recent.retain(|&j| j != i);
        }
        self.recent.push_back(i);
        while self.recent.len() > KEEP {
            if let Some(old) = self.recent.pop_front() {
                self.mem.remove(&old);
            }
        }
    }

    // a recent, unresolved failure in the window playback needs next
    fn trouble(&self) -> Option<String> {
        let window = self.anchor.map(|a| a..a.saturating_add(self.ahead));
        self.failed
            .iter()
            .filter(|(i, (_, at))| {
                at.elapsed() < TROUBLE_FOR && window.as_ref().is_none_or(|w| w.contains(i))
            })
            .max_by_key(|(_, (_, at))| *at)
            .map(|(_, (e, _))| e.clone())
    }
}

// what the player needs to decide whether to wait
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fill {
    pub waiting: bool,
    pub buffered: u64,
    pub to_end: u64,
    pub trouble: Option<String>,
}

impl S3Reader {
    pub fn open(
        bucket: Box<Bucket>,
        key: String,
        len: u64,
        rt: Handle,
        disk: Option<Slot>,
    ) -> Arc<Self> {
        let fetch = S3Fetch { bucket, key, rt };
        Self::with_fetch(Box::new(fetch), len, disk)
    }

    pub fn with_fetch(fetch: Box<dyn Fetch>, len: u64, disk: Option<Slot>) -> Arc<Self> {
        let state = State {
            ahead: START_AHEAD,
            ..Default::default()
        };
        Arc::new(Self {
            inner: Arc::new(Inner {
                len,
                fetch,
                disk,
                state: Mutex::new(state),
                landed: Condvar::new(),
            }),
        })
    }

    // the bytes to keep downloaded from the latest read on
    pub fn set_ahead(&self, bytes: u64) {
        let mut blocks = bytes.div_ceil(BLOCK) + 1;
        if self.inner.disk.is_none() {
            blocks = blocks.min(KEEP as u64 - 4);
        }
        let mut st = self.inner.state();
        if st.ahead != blocks {
            st.ahead = blocks;
            self.inner.schedule(&mut st);
        }
    }

    // after a seek the old position says nothing until the first read at the new one
    pub fn jumped(&self) {
        self.inner.state().anchor = None;
    }

    pub fn fill(&self) -> Fill {
        let st = self.inner.state();
        let Some(a) = st.anchor else {
            return Fill {
                waiting: st.waiting > 0,
                to_end: self.inner.len,
                trouble: st.trouble(),
                ..Default::default()
            };
        };
        let ready = (a..self.inner.blocks())
            .take(st.ahead as usize)
            .take_while(|&i| st.present(i))
            .count() as u64;
        let from = a * BLOCK;
        Fill {
            waiting: st.waiting > 0,
            buffered: (ready * BLOCK).min(self.inner.len - from),
            to_end: self.inner.len - from,
            trouble: st.trouble(),
        }
    }

    pub fn rate(&self) -> Option<f64> {
        self.inner.state().meter.rate()
    }

    pub fn troubled(&self) -> bool {
        self.inner.state().trouble().is_some()
    }
}

impl Drop for S3Reader {
    fn drop(&mut self) {
        self.inner.state().closed = true;
    }
}

fn eof() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "read past the end of the object",
    )
}

impl ByteReader for S3Reader {
    fn len(&self) -> u64 {
        self.inner.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|&e| e <= self.inner.len)
            .ok_or_else(eof)?;
        if end == offset {
            return Ok(());
        }
        // a cancelled read must not move the window or start downloads
        if cancel::cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        {
            let mut st = self.inner.state();
            st.anchor = Some(offset / BLOCK);
            self.inner.schedule(&mut st);
        }
        let mut pos = offset;
        while pos < end {
            let i = pos / BLOCK;
            let block = self.inner.block(i)?;
            let from = (pos - i * BLOCK) as usize;
            let n = block.len().saturating_sub(from).min((end - pos) as usize);
            if n == 0 {
                return Err(eof());
            }
            let at = (pos - offset) as usize;
            buf[at..at + n].copy_from_slice(&block[from..from + n]);
            pos += n as u64;
        }
        Ok(())
    }
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn blocks(&self) -> u64 {
        self.len.div_ceil(BLOCK)
    }

    fn block(self: &Arc<Self>, i: u64) -> io::Result<Arc<[u8]>> {
        // only a failure while this read waits is its answer; an older one is retried
        let asked = Instant::now();
        let mut st = self.state();
        let mut counted = false;
        let r = loop {
            if let Some(b) = st.mem.get(&i).cloned() {
                st.remember(i, b.clone());
                break Ok(b);
            }
            if st.stored.contains(&i) {
                drop(st);
                let data = self.read_stored(i);
                st = self.state();
                match data {
                    Some(d) => {
                        st.remember(i, d.clone());
                        break Ok(d);
                    }
                    None => {
                        st.stored.remove(&i);
                        continue;
                    }
                }
            }
            if let Some((e, at)) = st.failed.get(&i)
                && *at >= asked
            {
                break Err(io::Error::other(e.clone()));
            }
            // seeking or closing the preview must not wait out a slow request
            if cancel::cancelled() {
                break Err(io::Error::other("cancelled"));
            }
            if !st.loading.contains(&i) {
                self.start(&mut st, i);
                continue;
            }
            if !counted {
                st.waiting += 1;
                counted = true;
            }
            st = self
                .landed
                .wait_timeout(st, Duration::from_millis(50))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        };
        if counted {
            st.waiting -= 1;
        }
        r
    }

    fn schedule(self: &Arc<Self>, st: &mut State) {
        let Some(a) = st.anchor.filter(|_| !st.closed) else {
            return;
        };
        let end = a.saturating_add(st.ahead).min(self.blocks());
        for i in a..end {
            if st.loading.len() >= MAX_INFLIGHT {
                break;
            }
            let backing_off = st
                .failed
                .get(&i)
                .is_some_and(|(_, at)| at.elapsed() < RETRY_AFTER);
            if !st.present(i) && !st.loading.contains(&i) && !backing_off {
                self.start(st, i);
            }
        }
    }

    // a plain thread, because a runtime drops queued tasks on shutdown and the block would never land
    fn start(self: &Arc<Self>, st: &mut State, i: u64) {
        let inner = self.clone();
        let spawned = std::thread::Builder::new()
            .name("skybin-fetch".into())
            .spawn(move || inner.load(i));
        match spawned {
            Ok(_) => {
                st.loading.insert(i);
            }
            Err(e) => {
                st.failed.insert(i, (e.to_string(), Instant::now()));
            }
        }
    }

    fn load(self: &Arc<Self>, i: u64) {
        let r = if self.state().closed {
            Err("closed".to_string())
        } else {
            catch_unwind(AssertUnwindSafe(|| self.read_block(i)))
                .unwrap_or_else(|_| Err("reading the object crashed".into()))
        };
        let mut st = self.state();
        st.loading.remove(&i);
        match r {
            Ok((data, stored)) => {
                st.failed.remove(&i);
                if stored {
                    st.stored.insert(i);
                }
                st.remember(i, data);
            }
            Err(e) => {
                st.failed.insert(i, (e, Instant::now()));
            }
        }
        self.schedule(&mut st);
        drop(st);
        self.landed.notify_all();
    }

    fn block_path(&self, i: u64) -> Option<(&backend::Cache, std::path::PathBuf)> {
        let (c, dir) = self.disk.as_ref()?;
        Some((c, dir.join(i.to_string())))
    }

    fn want(&self, i: u64) -> usize {
        let start = i * BLOCK;
        ((start + BLOCK).min(self.len) - start) as usize
    }

    fn read_stored(&self, i: u64) -> Option<Arc<[u8]>> {
        let (c, p) = self.block_path(i)?;
        c.get(&p)
            .filter(|d| d.len() == self.want(i))
            .map(Into::into)
    }

    // the block, and whether it is on disk
    fn read_block(&self, i: u64) -> Result<(Arc<[u8]>, bool), String> {
        if let Some(data) = self.read_stored(i) {
            return Ok((data, true));
        }
        let (start, want) = (i * BLOCK, self.want(i));
        self.state().meter.started(Instant::now());
        let got = catch_unwind(AssertUnwindSafe(|| {
            self.fetch.fetch(start, start + want as u64)
        }));
        let n = match &got {
            Ok(Ok(d)) => d.len() as u64,
            _ => 0,
        };
        self.state().meter.finished(Instant::now(), n);
        let data = match got {
            Ok(r) => backend::nice(r)?,
            Err(_) => return Err("reading the object crashed".into()),
        };
        if data.len() != want {
            return Err(format!("got {} bytes instead of {want}", data.len()));
        }
        let stored = self.block_path(i).is_some_and(|(c, p)| c.put(&p, &data));
        Ok((data.into(), stored))
    }
}

// bytes per second while at least one download runs, so idle time with a full buffer
// doesn't count as a slow connection
#[derive(Default)]
struct Meter {
    running: usize,
    since: Option<Instant>,
    busy: Duration,
    bytes: u64,
    samples: VecDeque<(Duration, u64)>,
}

impl Meter {
    const SAMPLES: usize = 16;

    fn busy_at(&self, now: Instant) -> Duration {
        self.busy
            + self
                .since
                .map_or(Duration::ZERO, |s| now.saturating_duration_since(s))
    }

    fn sample(&mut self, now: Instant) {
        self.samples.push_back((self.busy_at(now), self.bytes));
        if self.samples.len() > Self::SAMPLES {
            self.samples.pop_front();
        }
    }

    fn started(&mut self, now: Instant) {
        if self.running == 0 {
            self.since = Some(now);
            self.sample(now);
        }
        self.running += 1;
    }

    fn finished(&mut self, now: Instant, bytes: u64) {
        self.running = self.running.saturating_sub(1);
        self.bytes += bytes;
        self.sample(now);
        if self.running == 0 {
            self.busy = self.busy_at(now);
            self.since = None;
        }
    }

    fn rate(&self) -> Option<f64> {
        let (b0, n0) = *self.samples.front()?;
        let (b1, n1) = *self.samples.back()?;
        let secs = (b1 - b0).as_secs_f64();
        (secs > 0.0).then(|| (n1 - n0) as f64 / secs)
    }
}

// absorbs hiccups when the connection is faster than the video
const AHEAD_SECS: f64 = 8.0;
// bitrate varies within a file
const MARGIN: f64 = 1.25;
pub const MAX_AHEAD: u64 = 512 * BLOCK;
const FALLBACK_AHEAD: u64 = 8 * BLOCK;

// on a connection slower than the video, enough head start to play to the end without stalling
pub fn ahead_bytes(bitrate: Option<f64>, rate: Option<f64>, remaining: Duration) -> u64 {
    let Some(bitrate) = bitrate.filter(|b| b.is_finite() && *b > 0.0) else {
        return FALLBACK_AHEAD;
    };
    let need = bitrate * MARGIN;
    let short = rate.map_or(0.0, |r| (need - r).max(0.0));
    let bytes = need * AHEAD_SECS + short * remaining.as_secs_f64();
    (bytes as u64).min(MAX_AHEAD)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hold {
    Running,
    Buffering,
}

// enough to start or resume; the rest of the target keeps downloading while it plays
const START_SECS: f64 = 3.0;
const FALLBACK_START: u64 = 2 * BLOCK;
const MAX_DOUBLINGS: u32 = 6;

// each stall doubles it, so a connection slower than the video stops less often
pub fn ready_bytes(bitrate: Option<f64>, stalls: u32, target: u64, to_end: u64) -> u64 {
    let base = match bitrate.filter(|b| b.is_finite() && *b > 0.0) {
        Some(b) => (b * MARGIN * START_SECS) as u64,
        None => FALLBACK_START,
    };
    let grown = base.saturating_mul(1 << stalls.min(MAX_DOUBLINGS));
    grown.min(target).min(to_end)
}

// `starting`: play was pressed or a seek landed while playing
pub fn hold(state: Hold, starting: bool, fill: &Fill, ready: u64) -> Hold {
    let short = fill.buffered < ready;
    let stuck = fill.waiting || fill.trouble.is_some();
    match state {
        Hold::Buffering if !stuck && !short => Hold::Running,
        Hold::Running if stuck || (starting && short) => Hold::Buffering,
        s => s,
    }
}

pub fn progress(fill: &Fill, ready: u64) -> f32 {
    match ready {
        0 => 1.0,
        r => (fill.buffered as f64 / r as f64).min(1.0) as f32,
    }
}

// rodio needs a cursor
pub struct S3Cursor {
    reader: Arc<S3Reader>,
    pos: u64,
    // rodio ends a track on a read error, so playback waits out network trouble instead
    patient: bool,
}

impl S3Cursor {
    pub fn new(reader: Arc<S3Reader>) -> Self {
        Self {
            reader,
            pos: 0,
            patient: false,
        }
    }

    pub fn patient(reader: Arc<S3Reader>) -> Self {
        Self {
            patient: true,
            ..Self::new(reader)
        }
    }
}

impl Read for S3Cursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.reader.len().saturating_sub(self.pos);
        let n = (buf.len() as u64).min(left) as usize;
        loop {
            match self.reader.read_at(self.pos, &mut buf[..n]) {
                Ok(()) => break,
                Err(_) if self.patient && !cancel::cancelled() && self.reader.troubled() => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e),
            }
        }
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for S3Cursor {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.reader.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Cache;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Clone)]
    struct Fake {
        data: Arc<[u8]>,
        asked: Arc<Mutex<Vec<u64>>>,
        open: Arc<AtomicBool>,
        delay: Duration,
        running: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        fail_once: Arc<Mutex<HashSet<u64>>>,
        fail_always: Arc<Mutex<HashSet<u64>>>,
    }

    impl Fake {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data: data.into(),
                asked: Arc::default(),
                open: Arc::new(AtomicBool::new(true)),
                delay: Duration::ZERO,
                running: Arc::default(),
                peak: Arc::default(),
                fail_once: Arc::default(),
                fail_always: Arc::default(),
            }
        }

        fn asked(&self) -> Vec<u64> {
            let mut v = self.asked.lock().unwrap().clone();
            v.sort();
            v
        }

        fn times(&self, i: u64) -> usize {
            self.asked().iter().filter(|&&j| j == i).count()
        }

        fn reader(&self) -> Arc<S3Reader> {
            S3Reader::with_fetch(Box::new(self.clone()), self.data.len() as u64, None)
        }
    }

    impl Fetch for Fake {
        fn fetch(&self, start: u64, end: u64) -> Res<Vec<u8>> {
            let i = start / BLOCK;
            self.asked.lock().unwrap().push(i);
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            let give_up = Instant::now() + Duration::from_secs(5);
            while !self.open.load(Ordering::Relaxed) && Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(1));
            }
            self.running.fetch_sub(1, Ordering::SeqCst);
            if self.fail_once.lock().unwrap().remove(&i)
                || self.fail_always.lock().unwrap().contains(&i)
            {
                return Err(format!("block {i} failed").into());
            }
            Ok(self.data[start as usize..end as usize].to_vec())
        }
    }

    fn bytes(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 % 251) as u8).collect()
    }

    fn blocks(n: u64) -> Vec<u8> {
        bytes((n * BLOCK) as usize)
    }

    fn wait(what: impl Fn() -> bool) -> bool {
        let give_up = Instant::now() + Duration::from_secs(2);
        while !what() && Instant::now() < give_up {
            std::thread::sleep(Duration::from_millis(1));
        }
        what()
    }

    #[test]
    fn reads_across_blocks_and_fetches_each_once() {
        let data = bytes(8 * BLOCK as usize + 100);
        let fake = Fake::new(data.clone());
        let r = fake.reader();
        let mut buf = vec![0; 10];
        let at = BLOCK - 4;
        r.read_at(at, &mut buf).unwrap();
        assert_eq!(buf, data[at as usize..at as usize + 10]);
        r.read_at(at, &mut buf).unwrap();
        let mut tail = vec![0; 100];
        r.read_at(8 * BLOCK, &mut tail).unwrap();
        assert_eq!(tail, data[8 * BLOCK as usize..]);
        assert!(r.read_at(8 * BLOCK + 1, &mut tail).is_err());
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(fake.asked(), [0, 1, 8]);
    }

    #[test]
    fn the_window_fills_while_paused_and_follows_reads() {
        let fake = Fake::new(blocks(20));
        let r = fake.reader();
        r.set_ahead(5 * BLOCK);
        r.read_at(0, &mut [0; 10]).unwrap();
        assert!(
            wait(|| r.fill().buffered == 6 * BLOCK),
            "{:?}",
            fake.asked()
        );
        assert_eq!(fake.asked(), [0, 1, 2, 3, 4, 5]);
        assert_eq!(r.fill().to_end, 20 * BLOCK);

        r.read_at(12 * BLOCK + 3, &mut [0; 10]).unwrap();
        assert!(
            wait(|| r.fill().buffered == 6 * BLOCK),
            "{:?}",
            fake.asked()
        );
        let want: Vec<u64> = (0..6).chain(12..18).collect();
        assert_eq!(fake.asked(), want);
        assert_eq!(r.fill().to_end, 8 * BLOCK);

        r.jumped();
        let fill = r.fill();
        assert_eq!((fill.buffered, fill.to_end), (0, 20 * BLOCK));
    }

    #[test]
    fn at_most_a_few_downloads_run_at_once() {
        let mut fake = Fake::new(blocks(24));
        fake.delay = Duration::from_millis(20);
        let r = fake.reader();
        r.set_ahead(20 * BLOCK);
        r.read_at(0, &mut [0; 10]).unwrap();
        assert!(
            wait(|| fake.asked().len() == KEEP - 4),
            "{:?}",
            fake.asked()
        );
        assert!(fake.peak.load(Ordering::SeqCst) <= MAX_INFLIGHT);
    }

    #[test]
    fn without_a_disk_cache_the_window_fits_in_memory() {
        let fake = Fake::new(blocks(40));
        let r = fake.reader();
        r.set_ahead(30 * BLOCK);
        r.read_at(0, &mut [0; 10]).unwrap();
        assert!(
            wait(|| fake.asked().len() == KEEP - 4),
            "{:?}",
            fake.asked()
        );
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(fake.asked().len(), KEEP - 4);
    }

    #[test]
    fn evicted_blocks_come_back_from_disk() {
        let dir = std::env::temp_dir().join(format!("skybin-evict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = Arc::new(Cache::new(dir.clone(), u64::MAX));
        let data = blocks(KEEP as u64 + 8);
        let fake = Fake::new(data.clone());
        let r = S3Reader::with_fetch(
            Box::new(fake.clone()),
            data.len() as u64,
            Some((cache, dir.join("o"))),
        );
        let mut all = vec![0; data.len()];
        r.read_at(0, &mut all).unwrap();
        let fetched = fake.asked().len();
        let mut first = [0; 16];
        r.read_at(0, &mut first).unwrap();
        assert_eq!(first[..], data[..16]);
        assert_eq!(fake.asked().len(), fetched);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_reader_is_served_from_the_disk_cache() {
        let dir = std::env::temp_dir().join(format!("skybin-stream-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = Arc::new(Cache::new(dir.clone(), u64::MAX));
        let slot = (cache, dir.join("object"));
        let data = bytes(3 * BLOCK as usize + BLOCK as usize / 2);
        let len = data.len() as u64;
        let mut all = vec![0; data.len()];

        let first = Fake::new(data.clone());
        let r = S3Reader::with_fetch(Box::new(first.clone()), len, Some(slot.clone()));
        r.read_at(0, &mut all).unwrap();
        assert_eq!(all, data);
        drop(r);

        let second = Fake::new(data.clone());
        let r = S3Reader::with_fetch(Box::new(second.clone()), len, Some(slot));
        all.fill(0);
        r.read_at(0, &mut all).unwrap();
        assert_eq!(all, data);
        assert!(second.asked().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_readahead_failure_does_not_fail_a_later_read() {
        let fake = Fake::new(blocks(6));
        fake.fail_once.lock().unwrap().insert(2);
        let r = fake.reader();
        r.set_ahead(4 * BLOCK);
        r.read_at(0, &mut [0; 10]).unwrap();
        assert!(wait(|| fake.times(2) == 1 && fake.asked().len() == 5));
        std::thread::sleep(Duration::from_millis(20));
        let mut buf = [0; 10];
        r.read_at(2 * BLOCK, &mut buf).unwrap();
        assert_eq!(buf[..], blocks(3)[2 * BLOCK as usize..][..10]);
        assert_eq!(fake.times(2), 2);
    }

    #[test]
    fn a_failure_while_waiting_is_the_answer_and_the_next_read_retries() {
        let fake = Fake::new(blocks(2));
        fake.fail_once.lock().unwrap().insert(0);
        let r = fake.reader();
        let err = r.read_at(0, &mut [0; 10]).unwrap_err();
        assert!(err.to_string().contains("block 0 failed"), "{err}");
        r.read_at(0, &mut [0; 10]).unwrap();
        assert_eq!(fake.times(0), 2);
    }

    #[test]
    fn the_window_backs_off_from_a_failing_block_and_reports_trouble() {
        let fake = Fake::new(blocks(6));
        fake.fail_always.lock().unwrap().insert(1);
        let r = fake.reader();
        r.set_ahead(3 * BLOCK);
        r.read_at(0, &mut [0; 10]).unwrap();
        assert!(wait(|| fake.asked().len() == 4), "{:?}", fake.asked());
        for _ in 0..5 {
            r.read_at(0, &mut [0; 10]).unwrap();
        }
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(fake.times(1), 1);
        let fill = r.fill();
        assert_eq!(fill.buffered, BLOCK);
        assert!(fill.trouble.unwrap().contains("block 1 failed"));
        assert!(r.troubled());
    }

    #[test]
    fn waiting_is_reported_only_while_a_read_is_blocked() {
        let fake = Fake::new(blocks(4));
        fake.open.store(false, Ordering::Relaxed);
        let r = fake.reader();
        assert!(!r.fill().waiting);
        let r2 = r.clone();
        let read = std::thread::spawn(move || r2.read_at(0, &mut [0; 4]));
        assert!(wait(|| r.fill().waiting));
        fake.open.store(true, Ordering::Relaxed);
        read.join().unwrap().unwrap();
        assert!(!r.fill().waiting);

        fake.open.store(false, Ordering::Relaxed);
        let (r2, flag) = (r.clone(), Arc::new(AtomicBool::new(false)));
        let f = flag.clone();
        let read = std::thread::spawn(move || {
            cancel::with_cancel(&f, || r2.read_at(3 * BLOCK, &mut [0; 4]))
        });
        assert!(wait(|| r.fill().waiting));
        flag.store(true, Ordering::Relaxed);
        assert!(read.join().unwrap().is_err());
        assert!(!r.fill().waiting);
        fake.open.store(true, Ordering::Relaxed);
    }

    #[test]
    fn the_meter_counts_busy_time_only() {
        let mut m = Meter::default();
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        assert_eq!(m.rate(), None);
        m.started(at(0));
        m.started(at(0));
        m.finished(at(1000), BLOCK);
        m.finished(at(1000), BLOCK);
        assert_eq!(m.rate(), Some(2.0 * BLOCK as f64));
        m.started(at(11_000));
        m.finished(at(12_000), BLOCK);
        assert_eq!(m.rate(), Some(1.5 * BLOCK as f64));
    }

    #[test]
    fn ahead_follows_the_connection() {
        let mb = 1_000_000.0;
        let minute = Duration::from_secs(60);
        assert_eq!(ahead_bytes(Some(mb), Some(10.0 * mb), minute), 10_000_000);
        assert_eq!(ahead_bytes(Some(mb), None, minute), 10_000_000);
        assert_eq!(ahead_bytes(Some(mb), Some(0.5 * mb), minute), 55_000_000);
        assert_eq!(ahead_bytes(None, Some(mb), minute), FALLBACK_AHEAD);
        assert_eq!(ahead_bytes(Some(0.0), Some(mb), minute), FALLBACK_AHEAD);
        let hour = Duration::from_secs(3600);
        assert_eq!(ahead_bytes(Some(50.0 * mb), Some(mb), hour), MAX_AHEAD);
    }

    #[test]
    fn hold_waits_for_enough_and_does_not_flap() {
        let fill = |buffered, to_end, waiting| Fill {
            waiting,
            buffered,
            to_end,
            trouble: None,
        };
        use Hold::{Buffering as B, Running as R};
        assert_eq!(hold(R, false, &fill(100, 1000, true), 500), B);
        assert_eq!(hold(R, true, &fill(100, 1000, false), 500), B);
        assert_eq!(hold(R, true, &fill(500, 1000, false), 500), R);
        assert_eq!(hold(R, false, &fill(100, 1000, false), 500), R);
        assert_eq!(hold(B, false, &fill(100, 1000, false), 500), B);
        assert_eq!(hold(B, false, &fill(500, 1000, true), 500), B);
        assert_eq!(hold(B, false, &fill(500, 1000, false), 500), R);
        assert_eq!(hold(B, false, &fill(200, 200, false), 200), R);
        let troubled = Fill {
            trouble: Some("down".into()),
            ..fill(900, 1000, false)
        };
        assert_eq!(hold(R, false, &troubled, 500), B);
        assert_eq!(hold(B, false, &troubled, 500), B);
        assert_eq!(progress(&fill(250, 1000, false), 500), 0.5);
        assert_eq!(progress(&fill(0, 0, false), 0), 1.0);
    }

    #[test]
    fn playback_starts_after_a_few_seconds_and_waits_longer_after_each_stall() {
        let mb = 1_000_000.0;
        let (target, far) = (100_000_000, u64::MAX);
        assert_eq!(ready_bytes(Some(mb), 0, target, far), 3_750_000);
        assert_eq!(ready_bytes(Some(mb), 1, target, far), 7_500_000);
        assert_eq!(ready_bytes(Some(mb), 2, target, far), 15_000_000);
        assert_eq!(ready_bytes(Some(mb), 5, 20_000_000, far), 20_000_000);
        assert_eq!(ready_bytes(Some(mb), 0, target, 1_000_000), 1_000_000);
        assert_eq!(ready_bytes(None, 0, target, far), FALLBACK_START);
        assert_eq!(
            ready_bytes(Some(mb), u32::MAX, MAX_AHEAD, far),
            64 * 3_750_000
        );
    }

    #[test]
    fn a_cancelled_read_stops_waiting_and_starts_no_fetch() {
        let fake = Fake::new(bytes(3 * BLOCK as usize));
        fake.open.store(false, Ordering::Relaxed);
        let r = S3Reader::with_fetch(Box::new(fake.clone()), 3 * BLOCK, None);
        let flag = Arc::new(AtomicBool::new(false));
        let (r2, f2) = (r.clone(), flag.clone());
        let read =
            std::thread::spawn(move || cancel::with_cancel(&f2, || r2.read_at(0, &mut [0; 4])));
        assert!(wait(|| fake.asked() == [0, 1]));
        flag.store(true, Ordering::Relaxed);
        let started = Instant::now();
        assert!(read.join().unwrap().is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(cancel::with_cancel(&flag, || r.read_at(2 * BLOCK, &mut [0; 4])).is_err());
        assert_eq!(fake.asked(), [0, 1]);

        fake.open.store(true, Ordering::Relaxed);
        let mut buf = [0; 4];
        r.read_at(0, &mut buf).unwrap();
        assert_eq!(buf[..], bytes(4));
    }

    struct Slow(Arc<[u8]>);

    impl Fetch for Slow {
        fn fetch(&self, start: u64, end: u64) -> Res<Vec<u8>> {
            std::thread::sleep(Duration::from_millis(300));
            Ok(self.0[start as usize..end as usize].to_vec())
        }
    }

    // the app's open runs on the runtime, which waits for it on exit
    #[test]
    fn a_cancelled_open_lets_the_runtime_shut_down() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let data: Arc<[u8]> = bytes(40 * BLOCK as usize).into();
        let r = S3Reader::with_fetch(Box::new(Slow(data.clone())), data.len() as u64, None);
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        rt.spawn_blocking(move || {
            cancel::with_cancel(&f, || {
                (0..40).try_for_each(|i| r.read_at(i * BLOCK, &mut [0; 8]))
            })
        });
        std::thread::sleep(Duration::from_millis(400));
        flag.store(true, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(rt);
            let _ = tx.send(());
        });
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn cursor_seeks_and_reads() {
        let data = bytes(2 * BLOCK as usize);
        let fake = Fake::new(data.clone());
        let r = S3Reader::with_fetch(Box::new(fake), data.len() as u64, None);
        let mut c = S3Cursor::new(r);
        assert_eq!(c.seek(SeekFrom::End(-5)).unwrap(), 2 * BLOCK - 5);
        let mut buf = Vec::new();
        c.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, data[data.len() - 5..]);
        c.seek(SeekFrom::Start(BLOCK - 2)).unwrap();
        let mut four = [0; 4];
        c.read_exact(&mut four).unwrap();
        assert_eq!(four[..], data[BLOCK as usize - 2..BLOCK as usize + 2]);
        assert!(c.seek(SeekFrom::Current(-(4 * BLOCK as i64))).is_err());
    }

    fn wav(seconds: u32) -> Vec<u8> {
        let bytes = 8000 * 2 * seconds;
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + bytes).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(8000u32.to_le_bytes());
        wav.extend(16000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(bytes.to_le_bytes());
        wav.resize(wav.len() + bytes as usize, 0);
        wav
    }

    #[test]
    fn opening_media_reads_only_the_start() {
        let data = wav(1000);
        assert!(data.len() as u64 > 15 * BLOCK);
        let fake = Fake::new(data.clone());
        let r = S3Reader::with_fetch(Box::new(fake.clone()), data.len() as u64, None);
        let source = crate::media::open_video("long.wav", r.clone()).unwrap();
        assert_eq!(source.info().duration.seconds().round(), 1000.0);
        let audio = crate::media::open_audio("long.wav", r).unwrap();
        assert_eq!(audio.total(), Some(Duration::from_secs(1000)));
        std::thread::sleep(Duration::from_millis(20));
        assert!(fake.asked().len() <= 4, "{:?}", fake.asked());
    }
}
