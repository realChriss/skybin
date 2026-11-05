use s3::creds::Credentials;
use s3::serde_types::ObjectIdentifier;
use s3::{Bucket, BucketConfiguration, Region};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub type Report<'a> = &'a (dyn Fn(String) + Send + Sync);

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub endpoint: String,
    pub region: String,
    #[serde(with = "crate::secret")]
    pub access_key: String,
    #[serde(with = "crate::secret")]
    pub secret_key: String,
    pub path_style: bool,
    #[serde(default)]
    pub public_urls: HashMap<String, String>,
}

impl Profile {
    fn region(&self) -> Region {
        let region = match self.region.trim() {
            "" => "us-east-1",
            r => r,
        };
        Region::Custom {
            region: region.to_string(),
            endpoint: self.endpoint.trim().trim_end_matches('/').to_string(),
        }
    }

    fn creds(&self) -> Res<Credentials> {
        Ok(Credentials::new(
            Some(self.access_key.trim()),
            Some(self.secret_key.trim()),
            None,
            None,
            None,
        )?)
    }

    pub fn public_url(&self, bucket: &str, key: &str) -> Option<String> {
        let base = self.public_urls.get(bucket)?.trim().trim_end_matches('/');
        let scheme = if base.contains("://") { "" } else { "https://" };
        Some(format!("{scheme}{base}/{}", encode(key)))
    }

    pub fn bucket(&self, name: &str) -> Res<Box<Bucket>> {
        let b = Bucket::new(name, self.region(), self.creds()?)?;
        Ok(if self.path_style {
            b.with_path_style()
        } else {
            b
        })
    }
}

#[derive(Clone)]
pub struct Entry {
    pub key: String,
    pub name: String,
    pub size: u64,
    pub modified: String,
    pub etag: String,
    pub is_dir: bool,
}

pub fn nice<T>(r: Res<T>) -> Result<T, String> {
    r.map_err(|e| nice_err(&*e))
}

pub fn nice_err(e: &(dyn std::error::Error + Send + Sync)) -> String {
    let s = e.to_string();
    match (s.find("<Message>"), s.find("</Message>")) {
        (Some(a), Some(b)) if a < b => s[a + 9..b].to_string(),
        _ if s.contains("missing field `Name`") => {
            "The server doesn't support virtual-hosted URLs. Enable \"Path-style URLs\" for this connection.".into()
        }
        _ => s.chars().take(300).collect(),
    }
}

pub async fn list_buckets(p: &Profile) -> Res<Vec<String>> {
    let mut names: Vec<String> = Bucket::list_buckets(p.region(), p.creds()?)
        .await?
        .bucket_names()
        .collect();
    names.sort();
    Ok(names)
}

pub async fn create_bucket(p: &Profile, name: &str) -> Res<String> {
    let cfg = BucketConfiguration::default();
    let r = if p.path_style {
        Bucket::create_with_path_style(name, p.region(), p.creds()?, cfg).await?
    } else {
        Bucket::create(name, p.region(), p.creds()?, cfg).await?
    };
    if !r.success() {
        return Err(format!("HTTP {}: {}", r.response_code, r.response_text).into());
    }
    Ok(format!("Created bucket {name}"))
}

pub async fn delete_bucket(b: &Bucket, empty_first: bool, report: Report<'_>) -> Res<String> {
    if empty_first {
        let keys = list_all(b, "").await?;
        report(format!("Emptying bucket ({} objects)", keys.len()));
        delete_keys(b, keys).await?;
    }
    b.delete().await?;
    Ok(format!("Deleted bucket {}", b.name()))
}

pub async fn list_dir(b: &Bucket, prefix: &str) -> Res<Vec<Entry>> {
    let mut out = Vec::new();
    for page in b.list(prefix.to_string(), Some("/".to_string())).await? {
        for cp in page.common_prefixes.unwrap_or_default() {
            let name = cp.prefix[prefix.len()..].trim_end_matches('/').to_string();
            out.push(Entry {
                name,
                key: cp.prefix,
                size: 0,
                modified: String::new(),
                etag: String::new(),
                is_dir: true,
            });
        }
        for o in page.contents {
            if o.key == prefix {
                continue; // the folder's own placeholder object
            }
            out.push(Entry {
                name: o.key[prefix.len()..].to_string(),
                key: o.key,
                size: o.size,
                modified: o.last_modified,
                etag: o.e_tag.unwrap_or_default(),
                is_dir: false,
            });
        }
    }
    Ok(out)
}

async fn list_all(b: &Bucket, prefix: &str) -> Res<Vec<String>> {
    Ok(b.list(prefix.to_string(), None)
        .await?
        .into_iter()
        .flat_map(|p| p.contents)
        .map(|o| o.key)
        .collect())
}

async fn expand(b: &Bucket, entries: &[Entry]) -> Res<Vec<String>> {
    let mut keys = Vec::new();
    for e in entries {
        if e.is_dir {
            keys.extend(list_all(b, &e.key).await?);
        } else {
            keys.push(e.key.clone());
        }
    }
    Ok(keys)
}

async fn delete_keys(b: &Bucket, keys: Vec<String>) -> Res<usize> {
    if keys.is_empty() {
        return Ok(0);
    }
    let r = b
        .delete_objects(
            keys.into_iter()
                .map(ObjectIdentifier::new)
                .collect::<Vec<_>>(),
        )
        .await?;
    match r.errors.first() {
        None => Ok(r.deleted.len()),
        Some(e) => Err(format!(
            "{} object(s) not deleted, e.g. {}: {}",
            r.errors.len(),
            e.key,
            e.message
        )
        .into()),
    }
}

pub async fn delete(b: &Bucket, entries: Vec<Entry>, report: Report<'_>) -> Res<String> {
    let keys = expand(b, &entries).await?;
    report(format!("Deleting {} object(s)", keys.len()));
    Ok(format!("Deleted {} object(s)", delete_keys(b, keys).await?))
}

fn walk(
    path: &Path,
    base: &Path,
    prefix: &str,
    out: &mut Vec<(PathBuf, String)>,
) -> std::io::Result<()> {
    if path.is_dir() {
        for e in std::fs::read_dir(path)? {
            walk(&e?.path(), base, prefix, out)?;
        }
    } else {
        let rel = path.strip_prefix(base).unwrap_or(path);
        let parts: Vec<_> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect();
        out.push((path.to_path_buf(), format!("{prefix}{}", parts.join("/"))));
    }
    Ok(())
}

pub async fn upload(
    b: &Bucket,
    paths: Vec<PathBuf>,
    prefix: &str,
    report: Report<'_>,
) -> Res<String> {
    let mut files = Vec::new();
    for p in &paths {
        walk(p, p.parent().unwrap_or(p), prefix, &mut files)?;
    }
    let n = files.len();
    for (i, (path, key)) in files.iter().enumerate() {
        report(format!("Uploading {}/{n}: {key}", i + 1));
        let mut f = tokio::fs::File::open(path).await?;
        b.put_object_stream_with_content_type(&mut f, key, mime(key))
            .await?;
    }
    Ok(format!("Uploaded {n} file(s)"))
}

fn safe_rel(rel: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
    let bad = |s: &&str| *s == "." || *s == ".." || s.contains([':', '\\']);
    (!parts.is_empty() && !parts.iter().any(bad)).then(|| parts.iter().collect())
}

pub async fn plan_download(
    b: &Bucket,
    entries: &[Entry],
    prefix: &str,
    dir: &Path,
) -> Res<Vec<(String, PathBuf)>> {
    Ok(expand(b, entries)
        .await?
        .into_iter()
        .filter(|k| !k.ends_with('/'))
        .filter_map(|k| {
            let path = dir.join(safe_rel(&k[prefix.len()..])?);
            Some((k, path))
        })
        .collect())
}

pub async fn download(b: &Bucket, jobs: Vec<(String, PathBuf)>, report: Report<'_>) -> Res<String> {
    let n = jobs.len();
    for (i, (key, path)) in jobs.iter().enumerate() {
        report(format!("Downloading {}/{n}: {key}", i + 1));
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let mut f = tokio::fs::File::create(path).await?;
        if let Err(e) = b.get_object_to_writer(key, &mut f).await {
            drop(f);
            let _ = tokio::fs::remove_file(path).await;
            return Err(e.into());
        }
    }
    Ok(format!("Downloaded {n} file(s)"))
}

// also for the copy-source header, which rust-s3 passes through verbatim
fn encode(key: &str) -> String {
    key.bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (c as char).to_string()
            }
            _ => format!("%{c:02X}"),
        })
        .collect()
}

// ponytail: single CopyObject per key, so objects > 5 GB fail; add multipart copy if that matters.
pub async fn rename(b: &Bucket, from: Entry, to: String, report: Report<'_>) -> Res<String> {
    let pairs: Vec<(String, String)> = if from.is_dir {
        let to = if to.ends_with('/') {
            to
        } else {
            format!("{to}/")
        };
        list_all(b, &from.key)
            .await?
            .into_iter()
            .map(|k| {
                let dst = format!("{to}{}", &k[from.key.len()..]);
                (k, dst)
            })
            .collect()
    } else {
        vec![(from.key.clone(), to)]
    };
    let n = pairs.len();
    for (i, (src, dst)) in pairs.iter().enumerate() {
        report(format!("Copying {}/{n}: {dst}", i + 1));
        b.copy_object_internal(encode(src), dst).await?;
    }
    delete_keys(b, pairs.into_iter().map(|(src, _)| src).collect()).await?;
    Ok(format!("Moved {n} object(s)"))
}

pub async fn new_folder(b: &Bucket, key: String) -> Res<String> {
    b.put_object(&key, &[]).await?;
    Ok(format!("Created {key}"))
}

const CACHE_BUDGET: u64 = 2 * 1024 * 1024 * 1024;

pub struct Cache {
    pub(crate) dir: PathBuf,
    budget: u64,
    used: AtomicU64,
    trimming: Mutex<()>,
}

pub type Slot = (Arc<Cache>, PathBuf);

fn cache_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("skybin"));
    Some(
        local
            .or_else(|| eframe::storage_dir("skybin"))?
            .join("preview-cache"),
    )
}

pub fn open_cache() -> Option<Arc<Cache>> {
    let dir = cache_dir()?;
    let _ = std::fs::remove_dir_all(&dir);
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(&dir).ok()?;
    Some(Arc::new(Cache::new(dir, CACHE_BUDGET)))
}

impl Cache {
    pub(crate) fn new(dir: PathBuf, budget: u64) -> Self {
        Self {
            dir,
            budget,
            used: AtomicU64::new(0),
            trimming: Mutex::new(()),
        }
    }

    pub fn path(&self, b: &Bucket, e: &Entry) -> PathBuf {
        let mut h = DefaultHasher::new();
        (b.url(), &e.key, e.size, &e.modified, &e.etag).hash(&mut h);
        self.dir.join(format!("{:016x}", h.finish()))
    }

    pub fn get(&self, path: &Path) -> Option<Vec<u8>> {
        let data = std::fs::read(path).ok()?;
        let file = std::fs::OpenOptions::new().write(true).open(path);
        let _ = file.and_then(|f| f.set_modified(SystemTime::now()));
        Some(data)
    }

    // never creates the cache folder itself, so a download finishing after exit's cleanup can't refill it
    pub fn put(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
        if let Some(parent) = path.parent().filter(|p| *p != self.dir) {
            let _ = std::fs::create_dir(parent);
        }
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::File::create_new(&tmp)?;
        let written = f.write_all(data);
        drop(f);
        if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        let used = self.used.fetch_add(data.len() as u64, Ordering::Relaxed) + data.len() as u64;
        if used > self.budget {
            self.trim();
        }
        Ok(())
    }

    fn trim(&self) {
        let Ok(_busy) = self.trimming.try_lock() else {
            return;
        };
        let mut files = Vec::new();
        cache_files(&self.dir, true, &mut files);
        files.sort();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        // headroom, so the next few puts don't rescan the whole cache
        let keep = self.budget - self.budget / 10;
        for (_, len, path) in files {
            if total <= keep {
                break;
            }
            if std::fs::remove_file(path).is_ok() {
                total -= len;
            }
        }
        self.used.store(total, Ordering::Relaxed);
    }
}

fn cache_files(dir: &Path, nested: bool, out: &mut Vec<(SystemTime, u64, PathBuf)>) {
    let Ok(files) = std::fs::read_dir(dir) else {
        return;
    };
    for f in files.flatten() {
        let (Ok(meta), path) = (f.metadata(), f.path()) else {
            continue;
        };
        if meta.is_dir() {
            if nested {
                cache_files(&path, false, out);
            }
        } else if path.extension().is_none_or(|e| e != "tmp")
            && let Ok(modified) = meta.modified()
        {
            out.push((modified, meta.len(), path));
        }
    }
}

pub async fn fetch(b: &Bucket, key: &str, cached: Option<&Slot>) -> Res<Vec<u8>> {
    if let Some(data) = cached.and_then(|(c, p)| c.get(p)) {
        return Ok(data);
    }
    let data = b.get_object(key).await?.to_vec();
    if let Some((c, p)) = cached {
        let _ = c.put(p, &data);
    }
    Ok(data)
}

pub async fn get_range(b: &Bucket, key: &str, start: u64, end: u64) -> Res<Vec<u8>> {
    let data = b
        .get_object_range(key, start, Some(end - 1))
        .await?
        .to_vec();
    if data.len() as u64 != end - start {
        return Err(format!(
            "the server sent {} bytes instead of {}",
            data.len(),
            end - start
        )
        .into());
    }
    Ok(data)
}

pub async fn size(b: &Bucket, key: &str) -> Res<u64> {
    let (h, _) = b.head_object(key).await?;
    Ok(h.content_length.unwrap_or(0).max(0) as u64)
}

// stored media types are often wrong (`text/plain`), which stops browsers playing the file
pub async fn browser_link(b: &Bucket, key: &str) -> Res<String> {
    let queries = crate::media::kind(key).map(|_| {
        HashMap::from([
            ("response-content-type".to_string(), mime(key).to_string()),
            (
                "response-content-disposition".to_string(),
                "inline".to_string(),
            ),
        ])
    });
    Ok(b.presign_get(key, 3600, queries).await?)
}

pub async fn properties(b: &Bucket, key: &str) -> Res<Vec<(String, String)>> {
    let (h, _) = b.head_object(key).await?;
    let mut out: Vec<(String, String)> = [
        ("Size", h.content_length.map(|n| format!("{n} bytes"))),
        ("Content-Type", h.content_type),
        ("Last-Modified", h.last_modified),
        ("ETag", h.e_tag),
        ("Cache-Control", h.cache_control),
        ("Content-Encoding", h.content_encoding),
        ("Content-Disposition", h.content_disposition),
        ("Storage-Class", h.storage_class),
        ("Version-Id", h.version_id),
    ]
    .into_iter()
    .filter_map(|(k, v)| Some((k.to_string(), v?)))
    .collect();
    let mut meta: Vec<_> = h
        .metadata
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (format!("x-amz-meta-{k}"), v))
        .collect();
    meta.sort();
    out.extend(meta);
    Ok(out)
}

pub fn ext(key: &str) -> String {
    key.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn mime(key: &str) -> &'static str {
    match ext(key).as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "ogv" => "video/ogg",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "m4a" => "audio/mp4",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_link_overrides_media_type() {
        let p = Profile {
            endpoint: "https://s3.example.com".into(),
            access_key: "AKIA".into(),
            secret_key: "secret".into(),
            ..Default::default()
        };
        let b = p.bucket("b").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let video = rt.block_on(browser_link(&b, "a/Clip.MP4")).unwrap();
        assert!(video.contains("response-content-type=video%2Fmp4"));
        assert!(video.contains("response-content-disposition=inline"));
        let text = rt.block_on(browser_link(&b, "notes.txt")).unwrap();
        assert!(!text.contains("response-content-type"));
    }

    #[test]
    fn paths_and_encoding() {
        assert_eq!(safe_rel("a/b.txt"), Some(PathBuf::from("a").join("b.txt")));
        assert_eq!(safe_rel("../evil"), None);
        assert_eq!(safe_rel("a/../../evil"), None);
        assert_eq!(safe_rel("C:/x"), None);
        assert_eq!(safe_rel("a\\..\\x"), None);
        assert_eq!(safe_rel(""), None);
        assert_eq!(encode("dir/a b+ü.txt"), "dir/a%20b%2B%C3%BC.txt");
        assert_eq!(mime("x/Index.HTML"), "text/html");
        let p = Profile {
            public_urls: HashMap::from([("b".into(), "https://cdn.example.com/".into())]),
            ..Default::default()
        };
        assert_eq!(
            p.public_url("b", "v/a b.mp4").as_deref(),
            Some("https://cdn.example.com/v/a%20b.mp4")
        );
        assert_eq!(p.public_url("other", "x"), None);
        let bare = Profile {
            public_urls: HashMap::from([("b".into(), " cdn.example.com// ".into())]),
            ..Default::default()
        };
        assert_eq!(
            bare.public_url("b", "x.mp4").as_deref(),
            Some("https://cdn.example.com/x.mp4")
        );
    }

    #[test]
    fn nice_err_hints_at_path_style() {
        let e: Box<dyn std::error::Error + Send + Sync> = "serde xml: missing field `Name`".into();
        assert!(nice_err(&*e).contains("Path-style URLs"));
    }

    #[test]
    fn preview_cache_serves_stored_files_and_trims_the_oldest() {
        let dir = std::env::temp_dir().join(format!("skybin-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = Arc::new(Cache::new(dir.clone(), 10));
        let p = Profile {
            endpoint: "http://127.0.0.1:9".into(),
            access_key: "k".into(),
            secret_key: "s".into(),
            path_style: true,
            ..Default::default()
        };
        let b = p.bucket("b").unwrap();
        let mut e = Entry {
            key: "a.png".into(),
            name: "a.png".into(),
            size: 5,
            modified: "2024-05-01T10:00:00.000Z".into(),
            etag: "one".into(),
            is_dir: false,
        };
        let old = cache.path(&b, &e);
        assert_ne!(old, cache.path(&p.bucket("c").unwrap(), &e));
        cache.put(&old, b"hello").unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let hit = rt.block_on(fetch(&b, &e.key, Some(&(cache.clone(), old.clone()))));
        assert_eq!(hit.unwrap(), b"hello");

        e.etag = "two".into();
        let new = cache.path(&b, &e);
        assert_ne!(old, new);
        assert!(
            rt.block_on(fetch(&b, &e.key, Some(&(cache.clone(), new.clone()))))
                .is_err()
        );
        let block = new.join("0");
        cache.put(&block, b"world").unwrap();
        assert_eq!(cache.get(&block).unwrap(), b"world");

        let age = |path: &Path, secs| {
            let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
            let t = SystemTime::now() - std::time::Duration::from_secs(secs);
            file.set_modified(t).unwrap();
        };
        age(&old, 60);
        age(&block, 30);
        let newest = new.join("1");
        cache.put(&newest, b"!!").unwrap();
        assert!(!old.exists() && block.exists() && newest.exists());
        cache.put(&new.join("2"), b"later").unwrap();
        assert!(!block.exists() && newest.exists());

        std::fs::remove_dir_all(&dir).unwrap();
        assert!(cache.put(&old, b"late").is_err());
        assert!(cache.put(&new.join("3"), b"late").is_err());
        assert!(!dir.exists());
    }

    // docker run --rm -p 9000:9000 minio/minio server /data
    // S3_TEST_ENDPOINT=http://127.0.0.1:9000 cargo test -- --ignored
    #[test]
    #[ignore]
    fn roundtrip_against_real_s3() {
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        let p = Profile {
            name: "test".into(),
            endpoint: env("S3_TEST_ENDPOINT", "http://127.0.0.1:9000"),
            region: env("S3_TEST_REGION", "us-east-1"),
            access_key: env("S3_TEST_KEY", "minioadmin"),
            secret_key: env("S3_TEST_SECRET", "minioadmin"),
            path_style: env("S3_TEST_PATH_STYLE", "1") == "1",
            ..Default::default()
        };
        let dir = |key: &str| Entry {
            key: key.into(),
            name: key.trim_end_matches('/').into(),
            size: 0,
            modified: String::new(),
            etag: String::new(),
            is_dir: true,
        };
        let names = |v: Vec<Entry>| {
            v.into_iter()
                .map(|e| (e.name, e.is_dir))
                .collect::<Vec<_>>()
        };
        let report = |_: String| {};

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let name = format!("skybin-test-{}", std::process::id());
            create_bucket(&p, &name).await.unwrap();
            assert!(list_buckets(&p).await.unwrap().contains(&name));
            let b = p.bucket(&name).unwrap();

            let tmp = std::env::temp_dir().join(&name);
            let src = tmp.join("src");
            std::fs::create_dir_all(src.join("sub")).unwrap();
            std::fs::write(src.join("a.txt"), "hello").unwrap();
            std::fs::write(src.join("sub").join("b ü+.txt"), "world").unwrap();

            upload(&b, vec![src.clone()], "", &report).await.unwrap();
            new_folder(&b, "empty/".into()).await.unwrap();
            assert_eq!(
                names(list_dir(&b, "").await.unwrap()),
                [("empty".into(), true), ("src".into(), true)]
            );
            assert_eq!(
                names(list_dir(&b, "src/").await.unwrap()),
                [("sub".into(), true), ("a.txt".into(), false)]
            );

            let props = properties(&b, "src/a.txt").await.unwrap();
            assert!(
                props.contains(&("Content-Type".into(), "text/plain".into())),
                "{props:?}"
            );
            assert!(
                props.contains(&("Size".into(), "5 bytes".into())),
                "{props:?}"
            );

            // key needs URL-encoding in the copy source
            rename(&b, dir("src/"), "moved".into(), &report)
                .await
                .unwrap();
            assert_eq!(
                names(list_dir(&b, "").await.unwrap()),
                [("empty".into(), true), ("moved".into(), true)]
            );

            let jobs = plan_download(&b, &[dir("moved/")], "", &tmp.join("dl"))
                .await
                .unwrap();
            download(&b, jobs, &report).await.unwrap();
            let got =
                std::fs::read_to_string(tmp.join("dl").join("moved").join("sub").join("b ü+.txt"))
                    .unwrap();
            assert_eq!(got, "world");

            delete(&b, vec![dir("empty/")], &report).await.unwrap();
            assert_eq!(
                names(list_dir(&b, "").await.unwrap()),
                [("moved".into(), true)]
            );

            let block = crate::stream::BLOCK as usize;
            let big: Vec<u8> = (0..3 * block + 10).map(|i| (i % 251) as u8).collect();
            b.put_object("big.bin", &big).await.unwrap();
            assert_eq!(size(&b, "big.bin").await.unwrap(), big.len() as u64);
            let (rb, len) = (b.clone(), big.len() as u64);
            let rt = tokio::runtime::Handle::current();
            let got = tokio::task::spawn_blocking(move || {
                use filmcraft_media::ByteReader;
                let r = crate::stream::S3Reader::open(rb, "big.bin".into(), len, rt, None);
                let mut buf = vec![0; block + 20];
                r.read_at(block as u64 * 2 - 10, &mut buf).map(|_| buf)
            });
            let got = got.await.unwrap().unwrap();
            assert_eq!(got, big[block * 2 - 10..block * 3 + 10]);

            delete_bucket(&b, true, &report).await.unwrap();
            assert!(!list_buckets(&p).await.unwrap().contains(&name));
            let _ = std::fs::remove_dir_all(&tmp);
        });
    }
}
