use crate::app::{App, Dialog, Preview};
use crate::backend::{Entry, Profile};
use eframe::egui::{self, Key, Modifiers, accesskit::Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

fn offline() -> Profile {
    Profile {
        name: "Local".into(),
        endpoint: "http://127.0.0.1:9".into(),
        region: "us-east-1".into(),
        access_key: "k".into(),
        secret_key: "s".into(),
        path_style: true,
        ..Default::default()
    }
}

fn file(name: &str, size: u64, modified: &str) -> Entry {
    Entry {
        key: name.into(),
        name: name.into(),
        size,
        modified: modified.into(),
        is_dir: false,
    }
}

fn browsing() -> App {
    let mut app = App::new(vec![offline()], Some(0));
    app.started = true; // skip the initial bucket fetch
    app.buckets = vec!["photos".into()];
    app.bucket = Some("photos".into());
    app.entries = vec![
        file("b.jpg", 2048, "2024-05-01T10:00:00.000Z"),
        Entry {
            key: "2024/".into(),
            name: "2024".into(),
            size: 0,
            modified: String::new(),
            is_dir: true,
        },
        file("a.txt", 10, "2024-06-01T09:30:00.000Z"),
    ];
    app.sort_entries();
    app
}

fn harness(app: App) -> Harness<'static, App> {
    let mut h = Harness::builder()
        .with_accessibility_check(false)
        .with_size([1100.0, 700.0])
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    crate::ui::theme::apply(&h.ctx);
    h.run();
    h
}

fn names(h: &Harness<App>) -> Vec<String> {
    h.state().entries.iter().map(|e| e.name.clone()).collect()
}

#[test]
fn empty_state_add_connection() {
    let mut h = harness(App::new(vec![], None));
    h.run();
    h.get_by_label("Add a connection to get started");
    h.get_by_label("Add connection").click();
    h.run();

    h.get_by_label("New connection");
    assert!(
        h.get_by_label("Save").accesskit_node().is_disabled(),
        "save needs a name"
    );
    h.get_by_role_and_label(Role::TextInput, "Name")
        .type_text("Local");
    let endpoint = h.get_by_role_and_label(Role::TextInput, "Endpoint");
    endpoint.focus();
    endpoint.type_text("http://127.0.0.1:9");
    h.run();
    h.get_by_label("Save").click();
    h.run_ok(); // saving kicks off a (failing, offline) bucket fetch with a spinner

    let app = h.state();
    assert!(app.dialog.is_none());
    assert_eq!(app.profiles.len(), 1);
    assert_eq!(app.profiles[0].name, "Local");
    assert_eq!(app.profiles[0].endpoint, "http://127.0.0.1:9");
    assert_eq!(app.profile, Some(0));
}

#[test]
fn folders_first_and_sorting() {
    let mut h = harness(browsing());
    h.run();
    assert_eq!(names(&h), ["2024", "a.txt", "b.jpg"]);

    h.get_by_value("Size").click();
    h.run();
    assert_eq!(names(&h), ["2024", "a.txt", "b.jpg"]);
    h.get_by_value("Size ⏶").click();
    h.run();
    assert_eq!(
        names(&h),
        ["2024", "b.jpg", "a.txt"],
        "descending, folder still first"
    );

    h.get_by_value("Modified").click();
    h.run();
    assert_eq!(names(&h), ["2024", "b.jpg", "a.txt"]);
}

#[test]
fn click_ctrl_click_shift_click_and_escape() {
    let mut h = harness(browsing());
    h.run();

    h.get_by_label("📄  a.txt").click();
    h.run();
    assert_eq!(h.state().selected.len(), 1);
    h.get_by_label("1 of 3 selected · 10 B");

    h.get_by_label("📄  b.jpg")
        .click_modifiers(Modifiers::COMMAND);
    h.run();
    assert_eq!(h.state().selected.len(), 2);

    h.get_by_label("📁  2024").click();
    h.run();
    h.get_by_label("📄  b.jpg")
        .click_modifiers(Modifiers::SHIFT);
    h.run();
    assert_eq!(h.state().selected.len(), 3, "shift selects the range");

    h.key_press(Key::Escape);
    h.run();
    assert!(h.state().selected.is_empty());
    h.get_by_label("3 items");
}

#[test]
fn filter_hides_non_matching_rows() {
    let mut h = harness(browsing());
    h.run();
    let filter = h.get_by_role(Role::TextInput);
    filter.focus();
    filter.type_text("JPG");
    h.run();
    assert!(h.query_by_label("📄  a.txt").is_none());
    h.get_by_label("📄  b.jpg");

    h.get_by_role(Role::TextInput).type_text("zzz");
    h.run();
    h.get_by_label("No matches");
}

#[test]
fn delete_asks_for_confirmation() {
    let mut h = harness(browsing());
    h.run();
    h.get_by_label("📁  2024").click();
    h.run();
    h.key_press(Key::Delete);
    h.run();

    h.get_by_label("Delete 1 item(s)? This cannot be undone.");
    h.get_by_label("Folders are deleted with everything inside them.");
    h.get_by_label("Cancel").click();
    h.run();
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().entries.len(), 3, "nothing deleted");
}

#[test]
fn rename_needs_a_changed_key() {
    let mut h = harness(browsing());
    h.run();
    h.get_by_label("📄  a.txt").click();
    h.run();
    h.key_press(Key::F2);
    h.run();

    assert!(matches!(&h.state().dialog, Some(Dialog::Rename { to, .. }) if to == "a.txt"));
    assert!(
        h.get_by_label("Rename").accesskit_node().is_disabled(),
        "unchanged key"
    );
    h.get_by(|n| n.role() == Role::TextInput && n.value().as_deref() == Some("a.txt"))
        .type_text("x");
    h.run();
    assert!(!h.get_by_label("Rename").accesskit_node().is_disabled());

    h.key_press(Key::Escape);
    h.run();
    assert!(h.state().dialog.is_none());
}

#[test]
fn enter_opens_folder_and_backspace_goes_up() {
    let mut h = harness(browsing());
    h.run();
    h.get_by_label("📁  2024").click();
    h.run();
    h.key_press(Key::Enter);
    h.run_ok(); // listing the folder fails offline; we only care about navigation
    assert_eq!(h.state().prefix, "2024/");
    h.get_by_label("2024"); // breadcrumb

    h.key_press(Key::Backspace);
    h.run_ok();
    assert_eq!(h.state().prefix, "");
}

#[test]
fn bucket_context_menu_offers_delete() {
    let mut h = harness(browsing());
    h.run();
    h.get_by_label("🗄  photos").click_secondary();
    h.run();
    h.get_by_label("🗑  Delete bucket…").click();
    h.run();
    h.get_by_label("Delete bucket “photos”? This cannot be undone.");
    assert!(
        matches!(&h.state().dialog, Some(Dialog::DeleteBucket { name, empty_first: false }) if name == "photos")
    );
}

/// Regression: sizing sidebar widgets from `available_width()` made the resizable panel grow
/// every frame, shifting the whole layout and making clicks miss.
#[test]
fn layout_is_stable_between_frames() {
    let mut h = harness(browsing());
    let before = h.get_by_value("Size").rect();
    h.run_steps(5);
    assert_eq!(h.get_by_value("Size").rect(), before);
}

/// Regression: a button overlaid on the image pushed the properties up over it and, being wider
/// than its slot, widened the resizable panel on every repaint.
#[test]
fn image_preview_keeps_its_place() {
    let mut app = browsing();
    app.props = Some(("b.jpg".into(), Ok(vec![("ETag".into(), "abc".into())])));
    let png = include_bytes!("../assets/icon.png").to_vec();
    app.preview = Some(("b.jpg".into(), Preview::Image(png.into())));
    let mut h = harness(app);
    let image = h.get_by_role(Role::Image).rect();
    assert!(h.get_by_label("ETag").rect().top() >= image.bottom());
    assert!(h.get_by_label("↗").rect().bottom() <= image.top());
    for x in [900.0, 950.0, 1000.0] {
        h.hover_at(egui::pos2(x, 200.0));
        h.run_steps(3);
    }
    assert_eq!(h.get_by_role(Role::Image).rect(), image);
}
