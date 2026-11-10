//! Bug hunt pass 6, storing: writing files safely, the clipboard, the cut and art libraries.

use std::path::PathBuf;

use darkburn_core::kurbo::Rect;
use darkburn_core::{Shape, ShapeId};
use serde_json::Value;

use crate::DarkBurn;
use crate::api::execute_headless;
use crate::prefs::Prefs;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let d = std::env::temp_dir().join(format!("darkburn-hunt6-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TempDir(d)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

#[allow(dead_code)]
fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn add_rect(a: &mut DarkBurn) -> ShapeId {
    let mut id = ShapeId(0);
    a.editor.edit("Add", |d, _| {
        id = d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 1));
        true
    });
    id
}

/// Saving `plan.dbrn` goes through a temporary file. That file must not be one the person
/// already has: a `plan.dbrn.tmp` beside the project is theirs and stays as it is.
#[test]
fn saving_leaves_a_file_named_like_the_project_with_tmp_after_it_alone() {
    let dir = TempDir::new("tmpname");
    let mut a = app();
    add_rect(&mut a);
    let project = dir.path("plan.dbrn");
    let neighbour = dir.path("plan.dbrn.tmp");
    std::fs::write(&neighbour, b"mine").unwrap();
    crate::files::save_to(&mut a, &project).unwrap();
    assert!(project.is_file());
    assert_eq!(std::fs::read(&neighbour).ok().as_deref(), Some(&b"mine"[..]), "the neighbour file is untouched");
    // Nothing else is left in the folder.
    let mut names: Vec<String> = std::fs::read_dir(&dir.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["plan.dbrn", "plan.dbrn.tmp"]);
    // The same for the two libraries.
    for name in ["cuts.dbclb", "art.dbart"] {
        let file = dir.path(name);
        let tmp = dir.path(&format!("{name}.tmp"));
        std::fs::write(&tmp, b"mine").unwrap();
        if name.ends_with("dbclb") {
            crate::library::Library::default().save(&file).unwrap();
        } else {
            crate::art_library::ArtLibrary::default().save(&file).unwrap();
        }
        assert!(file.is_file());
        assert_eq!(std::fs::read(&tmp).ok().as_deref(), Some(&b"mine"[..]), "{name}.tmp is untouched");
    }
}

/// A file name may be as long as the file system allows (255 characters on Windows and most
/// others). The temporary file's name must not be longer than the name asked for.
#[test]
fn a_project_with_the_longest_file_name_the_system_allows_can_be_saved() {
    let dir = TempDir::new("longname");
    let mut a = app();
    add_rect(&mut a);
    let name = format!("{}.dbrn", "n".repeat(250));
    assert_eq!(name.len(), 255);
    let project = dir.path(&name);
    // The system takes the name as it is.
    std::fs::write(&project, b"x").expect("the file system allows this name");
    std::fs::remove_file(&project).unwrap();
    crate::files::save_to(&mut a, &project).expect("the project is saved under the long name");
    assert!(project.is_file());
    assert!(!a.editor.is_modified());
}
