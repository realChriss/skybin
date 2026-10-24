use crate::backend::{self, Entry, Profile};
use crate::update;
use eframe::egui;
use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

pub(crate) type Props = Result<Vec<(String, String)>, String>;

pub(crate) enum Msg {
    Buckets(Result<Vec<String>, String>),
    Listing {
        bucket: String,
        prefix: String,
        result: Result<Vec<Entry>, String>,
    },
    Props(String, Props),
    Progress(String),
    Done {
        result: Result<String, String>,
        buckets_changed: bool,
    },
    UpdateAvailable(String),
    Updated(String, Result<(), String>),
}

pub(crate) fn done(r: backend::Res<String>, buckets_changed: bool) -> Msg {
    Msg::Done {
        result: r.map_err(|e| backend::nice_err(&*e)),
        buckets_changed,
    }
}

pub(crate) enum Dialog {
    Profile { idx: Option<usize>, p: Profile },
    NewBucket(String),
    DeleteBucket { name: String, empty_first: bool },
    PublicUrl { bucket: String, url: String },
    NewFolder(String),
    Rename { from: Entry, to: String },
    Delete(Vec<Entry>),
}

#[derive(PartialEq, Clone, Copy)]
pub(crate) enum Sort {
    Name,
    Size,
    Modified,
}

pub struct App {
    pub(crate) rt: tokio::runtime::Runtime,
    pub(crate) tx: Sender<Msg>,
    pub(crate) rx: Receiver<Msg>,
    pub(crate) ctx: egui::Context,
    pub(crate) started: bool,

    pub(crate) profiles: Vec<Profile>,
    pub(crate) profile: Option<usize>,
    pub(crate) buckets: Vec<String>,
    pub(crate) bucket: Option<String>,
    pub(crate) prefix: String,
    pub(crate) entries: Vec<Entry>,
    pub(crate) selected: HashSet<String>,
    pub(crate) anchor: Option<usize>,
    pub(crate) filter: String,
    pub(crate) sort: (Sort, bool),
    pub(crate) props: Option<(String, Props)>,

    pub(crate) dialog: Option<Dialog>,
    pub(crate) focus_dialog: bool,
    pub(crate) busy: usize,
    pub(crate) loading: bool,
    pub(crate) status: Result<String, String>,
    pub(crate) new_version: Option<String>,
    pub(crate) update_line: Option<String>,
}

impl App {
    pub fn new(profiles: Vec<Profile>, profile: Option<usize>) -> Self {
        let (tx, rx) = channel();
        Self {
            rt: tokio::runtime::Runtime::new().expect("tokio runtime"),
            tx,
            rx,
            ctx: egui::Context::default(),
            started: false,
            profile: profile.filter(|&i| i < profiles.len()),
            profiles,
            buckets: vec![],
            bucket: None,
            prefix: String::new(),
            entries: vec![],
            selected: HashSet::new(),
            anchor: None,
            filter: String::new(),
            sort: (Sort::Name, true),
            props: None,
            dialog: None,
            focus_dialog: false,
            busy: 0,
            loading: false,
            status: Ok("Ready".into()),
            new_version: None,
            update_line: None,
        }
    }

    pub(crate) fn begin_frame(&mut self, ctx: &egui::Context) {
        self.ctx = ctx.clone();
        if !self.started {
            self.started = true;
            self.refresh_buckets();
            self.check_update();
        }
        self.poll();
    }

    fn check_update(&self) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            if let Some(tag) = update::check().await {
                let _ = tx.send(Msg::UpdateAvailable(tag));
                ctx.request_repaint();
            }
        });
    }

    pub(crate) fn install_update(&mut self) {
        let Some(tag) = self.new_version.take() else {
            return;
        };
        self.update_line = Some(format!("Downloading Skybin {tag}…"));
        self.spawn(async move {
            let r = update::install(&tag).await;
            Msg::Updated(tag, r)
        });
    }

    pub(crate) fn spawn(&mut self, f: impl Future<Output = Msg> + Send + 'static) {
        self.busy += 1;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let _ = tx.send(f.await);
            ctx.request_repaint();
        });
    }

    pub(crate) fn reporter(&self) -> impl Fn(String) + Send + Sync + 'static {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        move |s| {
            let _ = tx.send(Msg::Progress(s));
            ctx.request_repaint();
        }
    }

    pub(crate) fn poll(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            if !matches!(msg, Msg::Progress(_) | Msg::UpdateAvailable(_)) {
                self.busy = self.busy.saturating_sub(1);
            }
            match msg {
                Msg::Buckets(r) => match r {
                    Ok(b) => {
                        self.status = Ok(format!("{} bucket(s)", b.len()));
                        self.buckets = b;
                    }
                    Err(e) => self.status = Err(e),
                },
                Msg::Listing {
                    bucket,
                    prefix,
                    result,
                } => {
                    if self.bucket.as_ref() != Some(&bucket) || self.prefix != prefix {
                        continue;
                    }
                    self.loading = false;
                    match result {
                        Ok(e) => {
                            self.entries = e;
                            self.sort_entries();
                            let keys: HashSet<_> = self.entries.iter().map(|e| &e.key).collect();
                            self.selected.retain(|k| keys.contains(k));
                        }
                        Err(e) => self.status = Err(e),
                    }
                }
                Msg::Props(key, r) => self.props = Some((key, r)),
                Msg::Progress(s) => self.status = Ok(s),
                Msg::Done {
                    result,
                    buckets_changed,
                } => {
                    self.status = result;
                    if buckets_changed {
                        self.refresh_buckets();
                    } else {
                        self.refresh();
                    }
                }
                Msg::UpdateAvailable(tag) => self.new_version = Some(tag),
                Msg::Updated(tag, r) => match r {
                    Ok(()) => {
                        self.update_line =
                            Some(format!("✓ Updated to {tag}. Restart Skybin to use it."))
                    }
                    Err(e) => {
                        self.update_line = None;
                        self.status = Err(format!("Update failed: {e}"));
                        self.new_version = Some(tag);
                    }
                },
            }
        }
    }

    pub(crate) fn cur_profile(&self) -> Option<Profile> {
        self.profiles.get(self.profile?).cloned()
    }

    pub(crate) fn cur_bucket(&mut self) -> Option<Box<s3::Bucket>> {
        let r = self.cur_profile()?.bucket(self.bucket.as_ref()?);
        r.map_err(|e| self.status = Err(backend::nice_err(&*e)))
            .ok()
    }

    pub(crate) fn refresh_buckets(&mut self) {
        let Some(p) = self.cur_profile() else { return };
        self.status = Ok("Loading buckets…".into());
        self.spawn(async move {
            Msg::Buckets(
                backend::list_buckets(&p)
                    .await
                    .map_err(|e| backend::nice_err(&*e)),
            )
        });
    }

    pub(crate) fn refresh(&mut self) {
        let Some(b) = self.cur_bucket() else { return };
        let (bucket, prefix) = (b.name(), self.prefix.clone());
        self.loading = true;
        self.spawn(async move {
            let result = backend::list_dir(&b, &prefix)
                .await
                .map_err(|e| backend::nice_err(&*e));
            Msg::Listing {
                bucket,
                prefix,
                result,
            }
        });
    }

    pub(crate) fn navigate(&mut self, bucket: Option<String>, prefix: String) {
        if self.bucket == bucket && self.prefix == prefix {
            return;
        }
        self.bucket = bucket;
        self.prefix = prefix;
        self.entries.clear();
        self.selected.clear();
        self.anchor = None;
        self.filter.clear();
        self.props = None;
        self.refresh();
    }

    pub(crate) fn go_up(&mut self) {
        let p = self.prefix.trim_end_matches('/');
        let up = p
            .rfind('/')
            .map(|i| p[..=i].to_string())
            .unwrap_or_default();
        self.navigate(self.bucket.clone(), up);
    }

    pub(crate) fn open(&mut self, e: &Entry) {
        if e.is_dir {
            self.navigate(self.bucket.clone(), e.key.clone());
        } else {
            self.show_props(e.key.clone());
        }
    }

    pub(crate) fn show_props(&mut self, key: String) {
        let Some(b) = self.cur_bucket() else { return };
        self.props = Some((key.clone(), Ok(vec![])));
        self.spawn(async move {
            let r = backend::properties(&b, &key)
                .await
                .map_err(|e| backend::nice_err(&*e));
            Msg::Props(key, r)
        });
    }

    /// 7 days is the S3 maximum. Signing is local, so blocking is fine.
    pub(crate) fn copy_url(&mut self, key: &str) {
        let Some(b) = self.cur_bucket() else { return };
        match self.rt.block_on(b.presign_get(key, 7 * 24 * 3600, None)) {
            Ok(url) => {
                self.ctx.copy_text(url);
                self.status = Ok("Copied link (valid 7 days)".into());
            }
            Err(e) => self.status = Err(backend::nice_err(&e)),
        }
    }

    pub(crate) fn public_url(&self, key: &str) -> Option<String> {
        self.cur_profile()?.public_url(self.bucket.as_ref()?, key)
    }

    pub(crate) fn selection(&self) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|e| self.selected.contains(&e.key))
            .cloned()
            .collect()
    }

    pub(crate) fn upload(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let Some(b) = self.cur_bucket() else { return };
        let (prefix, report) = (self.prefix.clone(), self.reporter());
        self.spawn(async move { done(backend::upload(&b, paths, &prefix, &report).await, false) });
    }

    pub(crate) fn download(&mut self) {
        let sel = self.selection();
        let Some(b) = self.cur_bucket() else { return };
        let report = self.reporter();
        if let [e] = &sel[..]
            && !e.is_dir
        {
            let Some(path) = rfd::FileDialog::new().set_file_name(&e.name).save_file() else {
                return;
            };
            let key = e.key.clone();
            self.spawn(async move {
                done(
                    backend::download(&b, vec![(key, path)], &report).await,
                    false,
                )
            });
        } else if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let prefix = self.prefix.clone();
            self.spawn(async move {
                let r = async {
                    backend::download(
                        &b,
                        backend::plan_download(&b, &sel, &prefix, &dir).await?,
                        &report,
                    )
                    .await
                };
                done(r.await, false)
            });
        }
    }

    pub(crate) fn ask_rename(&mut self) {
        if let [e] = &self.selection()[..] {
            self.open_dialog(Dialog::Rename {
                from: e.clone(),
                to: e.key.clone(),
            });
        }
    }

    pub(crate) fn ask_delete(&mut self) {
        let sel = self.selection();
        if !sel.is_empty() {
            self.open_dialog(Dialog::Delete(sel));
        }
    }

    pub(crate) fn open_dialog(&mut self, d: Dialog) {
        self.dialog = Some(d);
        self.focus_dialog = true;
    }

    pub(crate) fn sort_entries(&mut self) {
        let (col, asc) = self.sort;
        self.entries.sort_by(|a, b| {
            let o = match col {
                Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                Sort::Size => a.size.cmp(&b.size),
                Sort::Modified => a.modified.cmp(&b.modified),
            };
            b.is_dir
                .cmp(&a.is_dir)
                .then(if asc { o } else { o.reverse() })
        });
    }

    pub(crate) fn submit(&mut self, d: Dialog) {
        match d {
            Dialog::Profile { idx, p } => {
                match idx {
                    Some(i) => self.profiles[i] = p,
                    None => self.profiles.push(p),
                }
                self.select_profile(Some(idx.unwrap_or(self.profiles.len() - 1)));
            }
            Dialog::NewBucket(name) => {
                let Some(p) = self.cur_profile() else { return };
                let name = name.trim().to_string();
                self.spawn(async move { done(backend::create_bucket(&p, &name).await, true) });
            }
            Dialog::DeleteBucket { name, empty_first } => {
                let Some(b) = self.cur_profile().and_then(|p| p.bucket(&name).ok()) else {
                    return;
                };
                if self.bucket.as_ref() == Some(&name) {
                    self.bucket = None;
                    self.entries.clear();
                }
                let report = self.reporter();
                self.spawn(async move {
                    done(backend::delete_bucket(&b, empty_first, &report).await, true)
                });
            }
            Dialog::PublicUrl { bucket, url } => {
                let Some(p) = self.profile.and_then(|i| self.profiles.get_mut(i)) else {
                    return;
                };
                match url.trim() {
                    "" => p.public_urls.remove(&bucket),
                    u => p.public_urls.insert(bucket, u.to_string()),
                };
            }
            Dialog::NewFolder(name) => {
                let Some(b) = self.cur_bucket() else { return };
                let key = format!("{}{}/", self.prefix, name.trim().trim_matches('/'));
                self.spawn(async move { done(backend::new_folder(&b, key).await, false) });
            }
            Dialog::Rename { from, to } => {
                let Some(b) = self.cur_bucket() else { return };
                let report = self.reporter();
                self.spawn(
                    async move { done(backend::rename(&b, from, to, &report).await, false) },
                );
            }
            Dialog::Delete(entries) => {
                let Some(b) = self.cur_bucket() else { return };
                let report = self.reporter();
                self.selected.clear();
                self.spawn(async move { done(backend::delete(&b, entries, &report).await, false) });
            }
        }
    }

    pub(crate) fn select_profile(&mut self, i: Option<usize>) {
        self.profile = i;
        self.buckets.clear();
        self.navigate(None, String::new());
        self.refresh_buckets();
    }
}
