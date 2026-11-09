//! File dialogs with a test seam. The app depends on this crate under the name `rfd`
//! (see `apps/darkburn/Cargo.toml`), so `rfd::FileDialog::new()...pick_file()` in the app code is
//! this type. Normally it opens the system dialog. A UI test calls [`script_begin`] on its thread
//! first: from then on no dialog opens; each pick takes the next answer queued with
//! [`script_answer`] (or is cancelled when none is queued) and is recorded in the log.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// What a dialog was asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    /// "pick_file", "pick_files" or "save_file".
    pub kind: &'static str,
    pub filters: Vec<(String, Vec<String>)>,
    pub file_name: Option<String>,
    pub directory: Option<PathBuf>,
}

#[derive(Default)]
struct Script {
    answers: VecDeque<Vec<PathBuf>>,
    log: Vec<Request>,
}

thread_local! {
    static SCRIPT: RefCell<Option<Script>> = const { RefCell::new(None) };
}

/// Stop opening system dialogs on this thread; record requests and answer from the queue.
pub fn script_begin() {
    SCRIPT.with(|s| *s.borrow_mut() = Some(Script::default()));
}

/// Back to the system dialogs on this thread.
pub fn script_end() {
    SCRIPT.with(|s| *s.borrow_mut() = None);
}

/// Queue the answer to the next dialog: the chosen paths (an empty list is a cancel).
pub fn script_answer(paths: Vec<PathBuf>) {
    SCRIPT.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut() {
            s.answers.push_back(paths);
        }
    });
}

/// The dialogs requested since the last call.
pub fn script_take_log() -> Vec<Request> {
    SCRIPT.with(|s| s.borrow_mut().as_mut().map(|s| std::mem::take(&mut s.log)).unwrap_or_default())
}

fn scripted(req: &Request) -> Option<Vec<PathBuf>> {
    SCRIPT.with(|s| {
        s.borrow_mut().as_mut().map(|s| {
            s.log.push(req.clone());
            s.answers.pop_front().unwrap_or_default()
        })
    })
}

/// A file dialog, built like `rfd::FileDialog`.
#[derive(Clone, Debug, Default)]
pub struct FileDialog {
    req: Request,
}

impl FileDialog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_filter(mut self, name: impl Into<String>, extensions: &[impl ToString]) -> Self {
        self.req.filters.push((name.into(), extensions.iter().map(ToString::to_string).collect()));
        self
    }

    pub fn set_file_name(mut self, name: impl Into<String>) -> Self {
        self.req.file_name = Some(name.into());
        self
    }

    pub fn set_directory<P: AsRef<Path>>(mut self, dir: P) -> Self {
        self.req.directory = Some(dir.as_ref().to_path_buf());
        self
    }

    fn real(&self) -> rfd::FileDialog {
        let mut d = rfd::FileDialog::new();
        for (name, exts) in &self.req.filters {
            d = d.add_filter(name, exts);
        }
        if let Some(n) = &self.req.file_name {
            d = d.set_file_name(n);
        }
        if let Some(dir) = &self.req.directory {
            d = d.set_directory(dir);
        }
        d
    }

    pub fn pick_file(self) -> Option<PathBuf> {
        let req = Request { kind: "pick_file", ..self.req.clone() };
        match scripted(&req) {
            Some(answer) => answer.into_iter().next(),
            None => self.real().pick_file(),
        }
    }

    pub fn pick_files(self) -> Option<Vec<PathBuf>> {
        let req = Request { kind: "pick_files", ..self.req.clone() };
        match scripted(&req) {
            Some(answer) => (!answer.is_empty()).then_some(answer),
            None => self.real().pick_files(),
        }
    }

    pub fn save_file(self) -> Option<PathBuf> {
        let req = Request { kind: "save_file", ..self.req.clone() };
        match scripted(&req) {
            Some(answer) => answer.into_iter().next(),
            None => self.real().save_file(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scripted_dialog_answers_from_the_queue_and_logs() {
        script_begin();
        script_answer(vec![PathBuf::from("a.dbrn")]);
        let got = FileDialog::new().add_filter("Project", &["dbrn"]).set_file_name("x").save_file();
        assert_eq!(got, Some(PathBuf::from("a.dbrn")));
        assert_eq!(FileDialog::new().pick_file(), None, "nothing queued: cancelled");
        let log = script_take_log();
        assert_eq!(log.len(), 2);
        assert_eq!((log[0].kind, log[0].file_name.as_deref()), ("save_file", Some("x")));
        script_end();
    }
}
