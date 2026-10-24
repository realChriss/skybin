use s3::creds::Credentials;
use s3::serde_types::ObjectIdentifier;
use s3::{Bucket, BucketConfiguration, Region};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub type Report<'a> = &'a (dyn Fn(String) + Send + Sync);

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// Empty = AWS.
    pub endpoint: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub path_style: bool,
    /// Bucket → public base URL (bucket website, CDN or custom domain).
    #[serde(default)]
    pub public_urls: HashMap<String, String>,
}

impl Profile {
    fn region(&self) -> Region {
        let region = match self.region.trim() {
            "" => "us-east-1".to_string(),
            r => r.to_string(),
        };
        match self.endpoint.trim().trim_end_matches('/') {
            "" => region.parse().unwrap_or(Region::UsEast1),
            e => Region::Custom {
                region,
                endpoint: e.to_string(),
            },
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
    /// Full key; folders end with '/'.
    pub key: String,
    /// Name relative to the listed prefix.
    pub name: String,
    pub size: u64,
    pub modified: String,
    pub is_dir: bool,
}

pub fn nice_err(e: &(dyn std::error::Error + Send + Sync)) -> String {
    let s = e.to_string();
    match (s.find("<Message>"), s.find("</Message>")) {
        (Some(a), Some(b)) if a < b => s[a + 9..b].to_string(),
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
        .filter_map(|k| Some((safe_rel(&k[prefix.len()..])?, k)).map(|(rel, k)| (k, dir.join(rel))))
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

/// Copy-source header must be URL-encoded; rust-s3 passes it through verbatim.
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

fn mime(key: &str) -> &'static str {
    let ext = key
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
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
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Full round-trip against a real S3-compatible server; skipped by default. E.g. with MinIO:
    ///   docker run --rm -p 9000:9000 minio/minio server /data
    ///   S3_TEST_ENDPOINT=http://127.0.0.1:9000 cargo test -- --ignored
    /// Optional: S3_TEST_REGION, S3_TEST_KEY, S3_TEST_SECRET (default minioadmin), S3_TEST_PATH_STYLE=0.
    #[test]
    #[ignore]
    fn roundtrip_against_real_s3() {
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        let p = Profile {
            name: "test".into(),
            endpoint: env("S3_TEST_ENDPOINT", ""),
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

            // Folder move with a key that needs URL-encoding in the copy source.
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

            delete_bucket(&b, true, &report).await.unwrap();
            assert!(!list_buckets(&p).await.unwrap().contains(&name));
            let _ = std::fs::remove_dir_all(&tmp);
        });
    }
}
