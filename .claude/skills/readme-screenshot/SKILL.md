---
name: readme-screenshot
description: Regenerate assets/screenshot.png, the README preview of Skybin with example data and the details panel open. Use when asked to create or update the app screenshot or preview image.
---

# README screenshot

Render the real UI headlessly with egui_kittest and invented data. This needs no S3 server and doesn't touch saved connections. Do it in a throwaway copy, so the `wgpu` dev feature and the test never reach the repo.

## 1. Make a copy

From the repo root, with `$WT` an empty scratch folder:

```sh
git archive HEAD | tar -x -C "$WT"
sed -i 's/^egui_kittest = "0.37.0"/egui_kittest = { version = "0.37.0", features = ["wgpu"] }/' "$WT/Cargo.toml"
```

## 2. Append the test to `$WT/src/tests.rs`

Extend `entries` to about 13 rows mixing folders, images, video, audio and a CSV. Use plausible sizes and recent dates. Keep names generic, and never use real connections, people or companies.

```rust
#[test]
#[ignore]
fn readme_screenshot() {
    let f = |name: &str, size: u64, modified: &str| Entry {
        key: format!("photos/2026/{name}"),
        ..file(name, size, modified)
    };
    let mut app = App::new(
        vec![Profile { name: "Production".into(), ..Default::default() }],
        Some(0),
    );
    app.started = true; // skip the bucket fetch and update check
    app.buckets = ["app-backups", "brand-assets", "media-library", "user-uploads", "website-static"]
        .map(String::from)
        .to_vec();
    app.bucket = Some("media-library".into());
    app.prefix = "photos/2026/".into();
    app.entries = vec![
        Entry {
            key: "photos/2026/originals/".into(),
            name: "originals".into(),
            size: 0,
            modified: String::new(),
            etag: String::new(),
            is_dir: true,
        },
        f("alpine-sunrise.jpg", 4_821_337, "2026-10-28T06:42:11.000Z"),
        f("coastline-drone.mp4", 186_442_018, "2026-09-12T11:27:49.000Z"),
    ];
    app.sort_entries();
    let key = "photos/2026/alpine-sunrise.jpg".to_string();
    app.selected.insert(key.clone());
    app.status = Ok("5 buckets".into());
    app.preview = Some((
        key.clone(),
        Preview::Image(std::fs::read(env!("SAMPLE_JPG")).unwrap().into()),
    ));
    let rows = [
        ("Size", "4821337 bytes"),
        ("Content-Type", "image/jpeg"),
        ("Last-Modified", "Wed, 28 Oct 2026 06:42:11 GMT"),
        ("ETag", "\"9f2c41de7ab05e63c18d24f07b6ea3d5\""),
        ("Storage-Class", "STANDARD"),
        ("x-amz-meta-camera", "Sony A7 IV"),
    ];
    app.props = Some((key, Ok(rows.map(|(k, v)| (k.to_string(), v.to_string())).to_vec())));

    let mut h = Harness::builder()
        .wgpu()
        .with_accessibility_check(false)
        .with_size([1060.0, 680.0])
        .with_pixels_per_point(2.0)
        .with_context_setup(|ctx| {
            crate::ui::theme::apply(ctx);
            crate::ui::theme::fonts(ctx, false);
            egui_extras::install_image_loaders(ctx);
        })
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.run();
    h.render().unwrap().save(env!("SHOT_PNG")).unwrap();
}
```

- `preview` and `props` must use the same key, or `begin_frame` drops the preview.
- `started = true` skips `install_image_loaders`, so the context setup has to call it.

## 3. Render

`SAMPLE_JPG` is any landscape JPEG of about 3:2 that is safe to publish. Generate one with Pillow rather than using a stock photo. Pass absolute paths for both variables. In Git Bash, convert them with `cygpath -m`. Sharing the repo's target folder avoids a full rebuild.

```sh
cd "$WT"
SAMPLE_JPG=... SHOT_PNG=... CARGO_TARGET_DIR=<repo>/target cargo test readme_screenshot -- --ignored
```

If `wgpu-hal` fails with mismatched `windows` types, run it again. That cleared on a rebuild, with no lockfile change.

## 4. Finish

The harness adds a black 8pt margin that isn't part of the app. Crop it and round the corners:

```python
import sys
from PIL import Image, ImageDraw

im = Image.open(sys.argv[1]).convert("RGB")
im = im.crop(im.point(lambda v: 255 if v > 8 else 0).getbbox())
s = 4
mask = Image.new("L", (im.width * s, im.height * s), 0)
ImageDraw.Draw(mask).rounded_rectangle([0, 0, mask.width - 1, mask.height - 1], 20 * s, fill=255)
im.putalpha(mask.resize(im.size, Image.LANCZOS))
im.save(sys.argv[2], optimize=True)
```

Save to `assets/screenshot.png`, then open the image and check it: text is sharp, the preview loaded, no black edge. Delete `$WT`.
