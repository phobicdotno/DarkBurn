//! The coverage helpers: they must pass for a healthy registry and fail for each kind of hole.

use darkburn_mcp::{Backend, CommandError, CommandInfo, CommandPage, HardwareClass, ListQuery, MockBackend, assert_full_coverage, coverage_report};
use serde_json::{Value, json};

/// The mock plus extra (possibly broken) commands.
struct Extra {
    inner: MockBackend,
    extra: Vec<CommandInfo>,
    /// Make `describe_command` disagree with the listing for this id.
    lie_about: Option<String>,
}

impl Extra {
    fn new(extra: Vec<CommandInfo>) -> Self {
        Self { inner: MockBackend::new(), extra, lie_about: None }
    }
}

impl Backend for Extra {
    fn list_commands(&mut self, q: &ListQuery) -> Result<CommandPage, CommandError> {
        let mut all = self.inner.list_commands(&ListQuery::default())?.commands;
        all.extend(self.extra.clone());
        Ok(q.apply(all))
    }
    fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError> {
        if let Some(c) = self.extra.iter().find(|c| c.id == id) {
            let mut c = c.clone();
            if self.lie_about.as_deref() == Some(id) {
                c.title.push_str(" (different)");
            }
            return Ok(Some(c));
        }
        self.inner.describe_command(id)
    }
    fn execute(&mut self, id: &str, args: Value) -> Result<Value, CommandError> {
        self.inner.execute(id, args)
    }
    fn inspect(&mut self, p: &str, d: u32) -> Result<Value, CommandError> {
        self.inner.inspect(p, d)
    }
}

fn gui(m: &MockBackend) -> Vec<String> {
    m.gui_actions()
}

#[test]
fn a_healthy_registry_passes() {
    let mock = MockBackend::new();
    let actions = gui(&mock);
    let mut b = MockBackend::new();
    let r = coverage_report(&mut b, &actions);
    assert!(r.ok(), "{r:?}");
    assert_eq!(r.registered, actions.len());
    assert!(r.missing.is_empty() && r.schema_errors.is_empty());
    assert_full_coverage(&mut b, &actions);
}

#[test]
#[should_panic(expected = "edit.align_left")]
fn a_gui_action_without_a_command_fails() {
    let mut b = MockBackend::new();
    let mut actions = gui(&b);
    actions.push("edit.align_left".into());
    assert_full_coverage(&mut b, &actions);
}

#[test]
fn missing_actions_are_named_and_extra_commands_are_informational() {
    let mut b = Extra::new(vec![CommandInfo::new("script.only", "Script only", "Tools")]);
    let mut actions = gui(&MockBackend::new());
    actions.push("view.zoom_fit".into());
    let r = coverage_report(&mut b, &actions);
    assert_eq!(r.missing, ["view.zoom_fit"]);
    assert_eq!(r.not_in_gui, ["script.only"]);
    assert!(!r.ok());
    actions.pop();
    assert!(coverage_report(&mut b, &actions).ok(), "a command with no GUI action is allowed");
}

#[test]
fn invalid_json_schemas_are_reported() {
    let mut bad = CommandInfo::new("bad.schema", "Bad", "Tools");
    bad.params_schema = json!({"type": "object", "properties": {"x": {"type": "nonsense"}}});
    let mut not_obj = CommandInfo::new("bad.root", "Bad root", "Tools");
    not_obj.params_schema = json!({"type": "string"});
    let mut bad_result = CommandInfo::new("bad.result", "Bad result", "Tools");
    bad_result.result_schema = json!({"minimum": "ten"});
    let mut b = Extra::new(vec![bad, not_obj, bad_result]);
    let r = coverage_report(&mut b, &[] as &[String]);
    assert!(r.schema_errors.iter().any(|e| e.contains("bad.schema")), "{r:?}");
    assert!(r.schema_errors.iter().any(|e| e.contains("bad.root")), "{r:?}");
    assert!(r.schema_errors.iter().any(|e| e.contains("bad.result")), "{r:?}");
    assert!(!r.ok());
}

#[test]
fn describe_must_match_list() {
    let mut b = Extra::new(vec![CommandInfo::new("tools.one", "One", "Tools")]);
    b.lie_about = Some("tools.one".into());
    let r = coverage_report(&mut b, &[] as &[String]);
    assert_eq!(r.describe_mismatches.len(), 1, "{r:?}");
    assert!(r.describe_mismatches[0].contains("tools.one"));
}

#[test]
fn id_and_flag_problems_are_reported() {
    let mut upper = CommandInfo::new("File.Exit", "Exit", "File");
    upper.enabled = true;
    let nodot = CommandInfo::new("exit", "Exit", "File");
    let mut class_only = CommandInfo::new("machine.sneaky", "Sneaky", "Laser");
    class_only.hardware_class = Some(HardwareClass::Motion); // hardware flag left false
    let mut stop = CommandInfo::new("machine.halt", "Halt", "Laser");
    stop.hardware_class = Some(HardwareClass::Stop);
    let dup = CommandInfo::new("file.new", "Duplicate of new", "File");
    let mut b = Extra::new(vec![upper, nodot, class_only, stop, dup]);
    let r = coverage_report(&mut b, &[] as &[String]);
    let all = r.id_problems.join("\n");
    for needle in ["File.Exit", "`exit`", "machine.sneaky", "machine.halt", "duplicate command id `file.new`"] {
        assert!(all.contains(needle), "missing {needle} in:\n{all}");
    }
}

#[test]
fn required_commands_must_exist() {
    struct Bare;
    impl Backend for Bare {
        fn list_commands(&mut self, q: &ListQuery) -> Result<CommandPage, CommandError> {
            Ok(q.apply(vec![CommandInfo::new("file.new", "New", "File")]))
        }
        fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError> {
            Ok((id == "file.new").then(|| CommandInfo::new("file.new", "New", "File")))
        }
        fn execute(&mut self, _: &str, _: Value) -> Result<Value, CommandError> {
            Ok(Value::Null)
        }
        fn inspect(&mut self, _: &str, _: u32) -> Result<Value, CommandError> {
            Ok(Value::Null)
        }
    }
    let r = coverage_report(&mut Bare, &["file.new"]);
    assert!(r.required_missing.contains(&"file.save".to_string()) && r.required_missing.contains(&"edit.undo".to_string()));
    assert!(!r.ok());
}

#[test]
fn the_mocks_own_catalog_has_valid_schemas_and_consistent_describe() {
    // The helper the app calls from its own test: every command's schema compiles.
    let mut b = MockBackend::new();
    let r = coverage_report(&mut b, &[] as &[String]);
    assert!(r.schema_errors.is_empty(), "{:?}", r.schema_errors);
    assert!(r.describe_mismatches.is_empty());
    assert!(r.id_problems.is_empty(), "{:?}", r.id_problems);
}
