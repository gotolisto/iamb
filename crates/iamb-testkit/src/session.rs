//! Scripted typing sessions: a TOML format, a loader and a runner (docs/test-strategy.md).
//!
//! This file is also compiled into `iamb-core`'s integration tests with `#[path]` (so that
//! `cargo llvm-cov -p iamb-core` counts the sessions); it refers to the rest of the testkit
//! only through `super::env`.
//!
//! # Format
//!
//! ```toml
//! description = "What the session checks"
//! auto_inject = true    # default: send the injector's markers right after every expansion
//! focus_normal = true   # default: start with SecureState(normal)
//!
//! [settings]            # optional
//! delimiters = " ."     # every character is a delimiter
//! date_format = "%d/%m/%Y"
//! time_format = "%H:%M"
//! locale = "de"
//!
//! [env]                 # optional
//! now = "2026-03-18T14:05:09"
//! clipboard = "text"
//!
//! [[snippet]]
//! abbr = "brb"
//! body = "be right back"
//! trigger = "delimiter" # or "immediate"; default delimiter
//! case_mode = "insensitive" # "exact", "insensitive" (default) or "adaptive"
//! word_start = true     # default: true for delimiter, false for immediate
//! enabled = true        # default
//! kind = "plain"        # or "rich"
//!
//! [[excluded]]          # optional: the snapshot's exclusion report, in order
//! abbr = "bad"
//! reason = "E_BAD_SYNTAX" # a template error code, or "duplicate"
//!
//! [[step]]
//! type = "brb "         # one action per step, see below
//! expect = [{ delete = 4 }, { text = "be right back" }, { text = " " }, { expanded = "brb" }]
//! ```
//!
//! Step actions: `type = "…"` (one `Char` per character), `backspace = n`,
//! `key = "enter" | "tab" | "escape" | "arrow" | "navigation" | "click" | "shortcut" | "other"`,
//! `focus = "app"`, `secure = "normal" | "secure" | "excluded" | "own_window" | "unknown"`,
//! `pause = bool`, `compose = bool`, `overflow = true`, `submit = { Field = "value" }`,
//! `cancel = true`, `inject = "start" | "finish" | "cancel"`, `snapshot_changed = true` (same
//! library, new snapshot), `snapshot = [{ abbr = …, body = … }]` (new library), `expand = "abbr"`
//! (an unknown abbreviation requests an id that is in no library), `reopen = true`,
//! `forget = true`, `set_clipboard = "…"`, `clear_clipboard = true`, `set_now = "…"`.
//! `submit` and `cancel` answer the last `ShowForm` unless `session = n` names a raw session id;
//! `inject` names the last expansion unless `expansion = n` does.
//!
//! Expected actions (`expect`, default none; compared in full and in order):
//! `{ delete = n }`, `{ text = "…", verbatim = [[start, end]] }` (verbatim default none),
//! `{ key = "enter", shift = false, times = 1 }`, `{ caret_back = n }`, `{ wait = ms }`,
//! `{ paste = "plain text", html = "<p>…</p>" }` (a rich snippet's `Paste`; `html` unchecked
//! when absent),
//! `{ form = "abbr", fields = ["Name"], last_values = { Name = "Kim" } }` (`fields` unchecked
//! when absent; `last_values` default none), `{ expanded = "abbr", chars_saved = n }`
//! (`chars_saved` unchecked when absent), `{ diagnostic = "code", snippet = "abbr", app = "x" }`
//! (`snippet` default none; `app` unchecked when absent).
//!
//! Optional checks after a step: `now_calls = n` and `clipboard_calls = n` (totals so far),
//! `suspended = bool`, `buffered = n` (buffer length), `state = "idle" | "pending" |
//! "injecting"`, `form_open = bool`, `held = bool` (an explicit request waits for the focus
//! verdict).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use iamb_core::datetime::{Format, LocalDateTime, Locale};
use iamb_core::engine::{
    Action, AppId, BoundaryKind, Engine, EngineSettings, Event, ExclusionReason, ExpansionId,
    FormSessionId, InjectionState, SecureState, Snapshot,
};
use iamb_core::matcher::Delimiters;
use iamb_core::model::{
    Abbreviation, CaseMode, Id, Snippet, SnippetKind, SyncMeta, Timestamp, Trigger,
};
use iamb_core::sensitive::Sensitive;
use iamb_core::template::{FormValues, KeyName};
use serde::Deserialize;

use super::env::FakeEnv;

/// One scripted session, as read from a TOML file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// What the session checks.
    #[serde(default)]
    pub description: String,
    /// Send `InjectionStarted` and `InjectionFinished` right after every expansion.
    #[serde(default = "yes")]
    pub auto_inject: bool,
    /// Start with `SecureState(normal)`.
    #[serde(default = "yes")]
    pub focus_normal: bool,
    /// Engine settings.
    #[serde(default)]
    pub settings: SettingsSpec,
    /// The fake environment.
    #[serde(default)]
    pub env: EnvSpec,
    /// The library, in precedence order.
    #[serde(default, rename = "snippet")]
    pub snippets: Vec<SnippetSpec>,
    /// The expected exclusion report of the first snapshot.
    #[serde(default)]
    pub excluded: Vec<ExcludedSpec>,
    /// The steps.
    #[serde(default, rename = "step")]
    pub steps: Vec<Step>,
}

fn yes() -> bool {
    true
}

/// Engine settings of a session.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsSpec {
    /// Every character is a delimiter.
    pub delimiters: Option<String>,
    /// Date format setting.
    pub date_format: Option<String>,
    /// Time format setting.
    pub time_format: Option<String>,
    /// Locale tag.
    pub locale: Option<String>,
}

/// The initial fake environment.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvSpec {
    /// ISO local date and time.
    pub now: Option<String>,
    /// Clipboard text.
    pub clipboard: Option<String>,
}

/// One library snippet.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnippetSpec {
    /// Abbreviation.
    pub abbr: String,
    /// Template body.
    pub body: String,
    /// Trigger (default delimiter).
    pub trigger: Option<Trigger>,
    /// Case mode (default insensitive).
    pub case_mode: Option<CaseMode>,
    /// Word start (default: true for delimiter, false for immediate).
    pub word_start: Option<bool>,
    /// Enabled (default true).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Kind (default plain).
    pub kind: Option<SnippetKind>,
}

/// One expected exclusion.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExcludedSpec {
    /// The excluded snippet.
    pub abbr: String,
    /// `duplicate` or a template error code.
    pub reason: String,
}

/// One step: exactly one action, then the checks.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(rename = "type")]
    /// `type` (see the module docs).
    pub typed: Option<String>,
    /// `backspace` (see the module docs).
    pub backspace: Option<u32>,
    /// `key` (see the module docs).
    pub key: Option<String>,
    /// `focus` (see the module docs).
    pub focus: Option<String>,
    /// `secure` (see the module docs).
    pub secure: Option<String>,
    /// `pause` (see the module docs).
    pub pause: Option<bool>,
    /// `compose` (see the module docs).
    pub compose: Option<bool>,
    /// `overflow` (see the module docs).
    pub overflow: Option<bool>,
    /// `submit` (see the module docs).
    pub submit: Option<BTreeMap<String, String>>,
    /// `cancel` (see the module docs).
    pub cancel: Option<bool>,
    /// `inject` (see the module docs).
    pub inject: Option<String>,
    /// `snapshot_changed` (see the module docs).
    pub snapshot_changed: Option<bool>,
    /// `snapshot` (see the module docs).
    pub snapshot: Option<Vec<SnippetSpec>>,
    /// `expand` (see the module docs).
    pub expand: Option<String>,
    /// `reopen` (see the module docs).
    pub reopen: Option<bool>,
    /// `forget` (see the module docs).
    pub forget: Option<bool>,
    /// `set_clipboard` (see the module docs).
    pub set_clipboard: Option<String>,
    /// `clear_clipboard` (see the module docs).
    pub clear_clipboard: Option<bool>,
    /// `set_now` (see the module docs).
    pub set_now: Option<String>,

    /// `session` (see the module docs).
    pub session: Option<u64>,
    /// `expansion` (see the module docs).
    pub expansion: Option<u64>,

    #[serde(default)]
    /// `expect` (see the module docs).
    pub expect: Vec<Expect>,
    /// `now_calls` (see the module docs).
    pub now_calls: Option<usize>,
    /// `clipboard_calls` (see the module docs).
    pub clipboard_calls: Option<usize>,
    /// `suspended` (see the module docs).
    pub suspended: Option<bool>,
    /// `buffered` (see the module docs).
    pub buffered: Option<usize>,
    /// `state` (see the module docs).
    pub state: Option<String>,
    /// `form_open` (see the module docs).
    pub form_open: Option<bool>,
    /// `held` (see the module docs).
    pub held: Option<bool>,
}

/// One expected action. Exactly one of the leading keys is set.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// `delete` (see the module docs).
    pub delete: Option<usize>,
    /// `text` (see the module docs).
    pub text: Option<String>,
    /// `key` (see the module docs).
    pub key: Option<String>,
    /// `caret_back` (see the module docs).
    pub caret_back: Option<usize>,
    /// `wait` (see the module docs).
    pub wait: Option<u32>,
    /// `form` (see the module docs).
    pub form: Option<String>,
    /// `expanded` (see the module docs).
    pub expanded: Option<String>,
    /// `diagnostic` (see the module docs).
    pub diagnostic: Option<String>,
    /// `paste` (see the module docs).
    pub paste: Option<String>,
    /// `html` (see the module docs).
    pub html: Option<String>,

    /// `verbatim` (see the module docs).
    pub verbatim: Option<Vec<[usize; 2]>>,
    /// `shift` (see the module docs).
    pub shift: Option<bool>,
    /// `times` (see the module docs).
    pub times: Option<u8>,
    /// `fields` (see the module docs).
    pub fields: Option<Vec<String>>,
    /// `last_values` (see the module docs).
    pub last_values: Option<BTreeMap<String, String>>,
    /// `chars_saved` (see the module docs).
    pub chars_saved: Option<usize>,
    /// `snippet` (see the module docs).
    pub snippet: Option<String>,
    /// `app` (see the module docs).
    pub app: Option<String>,
}

/// An action in comparable form, with text shown (these are fixtures, not logs).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Canon {
    Delete(usize),
    Text(String, Vec<[usize; 2]>),
    Key(String, bool, u8),
    CaretBack(usize),
    Wait(u32),
    Paste(String, Option<String>),
    Form {
        snippet: String,
        fields: Option<Vec<String>>,
        last_values: Option<BTreeMap<String, String>>,
    },
    Expanded {
        snippet: String,
        chars_saved: Option<usize>,
    },
    Diagnostic {
        code: String,
        snippet: Option<String>,
        app: Option<String>,
    },
}

/// Reads every `*.toml` file in `dir`, sorted by file name.
///
/// # Errors
///
/// A message naming the file that cannot be read or parsed.
pub fn load_dir(dir: &Path) -> Result<Vec<(String, Session)>, String> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let session: Session =
                toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
            let name = p
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            Ok((name, session))
        })
        .collect()
}

/// The directory of the scripted sessions, valid from both `iamb-core` and `iamb-testkit`
/// tests (each crate's manifest directory is a sibling of the other's).
#[must_use]
pub fn sessions_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../iamb-testkit/sessions")
}

/// Runs one session.
///
/// # Errors
///
/// A report of the first failing step, with what was expected and what the engine did.
pub fn run(session: &Session) -> Result<(), String> {
    Runner::new(session)?.run(session)
}

struct Runner {
    engine: Engine,
    env: FakeEnv,
    settings: EngineSettings,
    library: Vec<SnippetSpec>,
    ids: HashMap<String, Id>,
    next_id: u64,
    last_session: Option<FormSessionId>,
    last_expansion: Option<ExpansionId>,
    auto_inject: bool,
    seq: u64,
}

impl Runner {
    fn new(session: &Session) -> Result<Self, String> {
        let mut settings = EngineSettings::default();
        let s = &session.settings;
        if let Some(d) = &s.delimiters {
            settings.delimiters = Delimiters::new(d.chars());
        }
        if let Some(f) = &s.date_format {
            settings.date_format = Format::parse(f).map_err(|e| format!("date_format: {e}"))?;
        }
        if let Some(f) = &s.time_format {
            settings.time_format = Format::parse(f).map_err(|e| format!("time_format: {e}"))?;
        }
        if let Some(l) = &s.locale {
            settings.locale = Locale::from_tag(l);
        }
        let env = match &session.env.now {
            Some(now) => FakeEnv::new(parse_now(now)?),
            None => FakeEnv::default(),
        };
        env.set_clipboard(session.env.clipboard.clone());

        let mut runner = Self {
            engine: Engine::new(Snapshot::empty()),
            env,
            settings,
            library: session.snippets.clone(),
            ids: HashMap::new(),
            next_id: 1,
            last_session: None,
            last_expansion: None,
            auto_inject: session.auto_inject,
            seq: 0,
        };
        let snapshot = runner.build()?;
        let excluded: Vec<String> = snapshot
            .excluded()
            .iter()
            .map(|e| {
                let reason = match &e.reason {
                    ExclusionReason::Duplicate { .. } => "duplicate".to_owned(),
                    ExclusionReason::Template(t) => t.code().to_owned(),
                };
                format!("{} {reason}", runner.name(e.snippet))
            })
            .collect();
        let want: Vec<String> = session
            .excluded
            .iter()
            .map(|e| format!("{} {}", e.abbr, e.reason))
            .collect();
        if excluded != want {
            return Err(format!(
                "exclusions differ\n  expected: {want:?}\n  got:      {excluded:?}"
            ));
        }
        runner.engine = Engine::new(snapshot);
        if session.focus_normal {
            let out = runner
                .engine
                .handle(Event::SecureState(SecureState::Normal), &runner.env);
            if !out.is_empty() {
                return Err("SecureState(normal) at start produced actions".to_owned());
            }
        }
        Ok(runner)
    }

    fn id_for(&mut self, abbr: &str) -> Id {
        if let Some(id) = self.ids.get(abbr) {
            return *id;
        }
        let mut bytes = [0u8; 16];
        bytes[8..].copy_from_slice(&self.next_id.to_be_bytes());
        self.next_id += 1;
        let id = Id::from_bytes(bytes);
        self.ids.insert(abbr.to_owned(), id);
        id
    }

    fn name(&self, id: Id) -> String {
        self.ids
            .iter()
            .find(|(_, v)| **v == id)
            .map_or_else(|| format!("?{id:?}"), |(k, _)| k.clone())
    }

    fn build(&mut self) -> Result<Snapshot, String> {
        let mut snippets = Vec::new();
        for spec in self.library.clone() {
            let id = self.id_for(&spec.abbr);
            let abbreviation = Abbreviation::new(&spec.abbr)
                .map_err(|e| format!("abbreviation {:?}: {e}", spec.abbr))?;
            let trigger = spec.trigger.unwrap_or(Trigger::Delimiter);
            snippets.push(Snippet {
                meta: SyncMeta::new(id, Timestamp(0)),
                folder_id: Id::from_bytes([0; 16]),
                abbreviation,
                label: String::new(),
                kind: spec.kind.unwrap_or(SnippetKind::Plain),
                body: spec.body.clone(),
                trigger,
                case_mode: spec.case_mode.unwrap_or(CaseMode::Insensitive),
                word_start: spec.word_start.unwrap_or(trigger == Trigger::Delimiter),
                enabled: spec.enabled,
            });
        }
        Ok(Snapshot::build(snippets, self.settings.clone()))
    }

    fn run(mut self, session: &Session) -> Result<(), String> {
        for (i, step) in session.steps.iter().enumerate() {
            let got = self
                .step(step)
                .map_err(|e| format!("step {}: {e}", i + 1))?;
            let want: Vec<Canon> = step
                .expect
                .iter()
                .map(canon_expect)
                .collect::<Result<_, _>>()
                .map_err(|e| format!("step {}: {e}", i + 1))?;
            let got: Vec<Canon> = got
                .iter()
                .zip(want.iter().map(Some).chain(std::iter::repeat(None)))
                .map(|(a, w)| self.canon_actual(a, w))
                .collect();
            if got != want {
                let mut msg = format!(
                    "step {} ({}): actions differ\n  expected:",
                    i + 1,
                    summary(step)
                );
                for w in &want {
                    let _ = write!(msg, "\n    {w:?}");
                }
                msg.push_str("\n  got:");
                for g in &got {
                    let _ = write!(msg, "\n    {g:?}");
                }
                return Err(msg);
            }
            self.check(step)
                .map_err(|e| format!("step {} ({}): {e}", i + 1, summary(step)))?;
        }
        Ok(())
    }

    fn check(&self, step: &Step) -> Result<(), String> {
        let checks = [
            ("now_calls", step.now_calls, self.env.now_calls()),
            (
                "clipboard_calls",
                step.clipboard_calls,
                self.env.clipboard_calls(),
            ),
            ("buffered", step.buffered, self.engine.buffered_len()),
        ];
        for (what, want, got) in checks {
            if let Some(want) = want
                && want != got
            {
                return Err(format!("{what}: expected {want}, got {got}"));
            }
        }
        if let Some(want) = step.suspended
            && want != self.engine.is_suspended()
        {
            return Err(format!("suspended: expected {want}"));
        }
        if let Some(want) = step.form_open
            && want != self.engine.open_form().is_some()
        {
            return Err(format!("form_open: expected {want}"));
        }
        if let Some(want) = step.held
            && want != self.engine.holds_request()
        {
            return Err(format!("held: expected {want}"));
        }
        if let Some(want) = &step.state {
            let got = match self.engine.injection_state() {
                InjectionState::Idle => "idle",
                InjectionState::Pending(_) => "pending",
                InjectionState::Injecting(_) => "injecting",
            };
            if want != got {
                return Err(format!("state: expected {want}, got {got}"));
            }
        }
        Ok(())
    }

    /// Sends the step's events and returns every action.
    fn step(&mut self, step: &Step) -> Result<Vec<Action>, String> {
        let actions = count_actions(step);
        if actions != 1 {
            return Err(format!("a step needs exactly one action, found {actions}"));
        }
        let mut events = Vec::new();
        if let Some(text) = &step.typed {
            for ch in text.chars() {
                events.push(Event::char(ch, self.next_seq()));
            }
        } else if let Some(n) = step.backspace {
            for _ in 0..n {
                events.push(Event::Backspace {
                    seq: self.next_seq(),
                });
            }
        } else if let Some(key) = &step.key {
            events.push(Event::Boundary {
                kind: boundary(key)?,
                seq: self.next_seq(),
            });
        } else if let Some(app) = &step.focus {
            events.push(Event::FocusChanged(AppId(app.clone())));
        } else if let Some(state) = &step.secure {
            events.push(Event::SecureState(secure(state)?));
        } else if let Some(on) = step.pause {
            events.push(Event::Paused(on));
        } else if let Some(on) = step.compose {
            events.push(Event::Composing(on));
        } else if step.overflow.is_some() {
            events.push(Event::Overflow);
        } else if let Some(values) = &step.submit {
            let session = self.session(step)?;
            let values: FormValues = values.iter().collect();
            events.push(Event::FormSubmitted {
                session,
                values: Sensitive::new(values),
            });
        } else if step.cancel.is_some() {
            events.push(Event::FormCancelled {
                session: self.session(step)?,
            });
        } else if let Some(what) = &step.inject {
            let expansion = match step.expansion {
                Some(raw) => ExpansionId::from_raw(raw),
                None => self.last_expansion.ok_or("no expansion to inject")?,
            };
            let seq = self.next_seq();
            events.push(match what.as_str() {
                "start" => Event::InjectionStarted { expansion, seq },
                "finish" => Event::InjectionFinished { expansion, seq },
                "cancel" => Event::InjectionCancelled { expansion },
                other => return Err(format!("unknown inject {other:?}")),
            });
        } else if step.snapshot_changed.is_some() {
            events.push(Event::SnapshotChanged(self.build()?));
        } else if let Some(library) = &step.snapshot {
            self.library.clone_from(library);
            events.push(Event::SnapshotChanged(self.build()?));
        } else if let Some(abbr) = &step.expand {
            events.push(Event::ExpandRequest {
                snippet: self.id_for(abbr),
            });
        } else if step.reopen.is_some() {
            events.push(Event::ReopenLastForm);
        } else if step.forget.is_some() {
            self.engine.forget_form_values();
        } else if let Some(text) = &step.set_clipboard {
            self.env.set_clipboard(Some(text.clone()));
        } else if step.clear_clipboard.is_some() {
            self.env.set_clipboard(None);
        } else if let Some(now) = &step.set_now {
            self.env.set_now(parse_now(now)?);
        }

        let mut out = Vec::new();
        for event in events {
            let actions = self.engine.handle(event, &self.env);
            self.after(&actions, &mut out);
        }
        Ok(out)
    }

    /// Records ids from `actions`, appends them to `out`, and sends auto-injection markers.
    fn after(&mut self, actions: &[Action], out: &mut Vec<Action>) {
        out.extend_from_slice(actions);
        for action in actions {
            match action {
                Action::ShowForm { session, .. } => self.last_session = Some(*session),
                Action::Expanded { expansion, .. } => {
                    self.last_expansion = Some(*expansion);
                    if self.auto_inject {
                        for event in [
                            Event::InjectionStarted {
                                expansion: *expansion,
                                seq: self.next_seq(),
                            },
                            Event::InjectionFinished {
                                expansion: *expansion,
                                seq: self.next_seq(),
                            },
                        ] {
                            let more = self.engine.handle(event, &self.env);
                            self.after(&more, out);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn session(&self, step: &Step) -> Result<FormSessionId, String> {
        match step.session {
            Some(raw) => Ok(FormSessionId::from_raw(raw)),
            None => self
                .last_session
                .ok_or_else(|| "no form session".to_owned()),
        }
    }

    const fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn canon_actual(&self, action: &Action, want: Option<&Canon>) -> Canon {
        match action {
            Action::DeleteBack(n) => Canon::Delete(*n),
            Action::TypeText { text, verbatim } => Canon::Text(
                text.clone(),
                verbatim.iter().map(|r| [r.start, r.end]).collect(),
            ),
            Action::Paste {
                plain,
                markdown_html,
                ..
            } => Canon::Paste(
                plain.clone(),
                matches!(want, Some(Canon::Paste(_, Some(_)))).then(|| markdown_html.clone()),
            ),
            Action::PressKey { key, shift, times } => {
                Canon::Key(key.as_str().to_owned(), *shift, *times)
            }
            Action::MoveCaretBack(n) => Canon::CaretBack(*n),
            Action::Wait(ms) => Canon::Wait(*ms),
            Action::ShowForm {
                snippet,
                spec,
                last_values,
                ..
            } => {
                let check_fields = matches!(
                    want,
                    Some(Canon::Form {
                        fields: Some(_),
                        ..
                    })
                );
                Canon::Form {
                    snippet: self.name(*snippet),
                    fields: check_fields.then(|| spec.iter().map(|f| f.name.clone()).collect()),
                    last_values: last_values.as_ref().map(|v| {
                        spec.iter()
                            .filter_map(|f| {
                                v.expose()
                                    .get(&f.name)
                                    .map(|x| (f.name.clone(), x.to_owned()))
                            })
                            .collect()
                    }),
                }
            }
            Action::Expanded {
                snippet,
                chars_saved,
                ..
            } => {
                let check = matches!(
                    want,
                    Some(Canon::Expanded {
                        chars_saved: Some(_),
                        ..
                    })
                );
                Canon::Expanded {
                    snippet: self.name(*snippet),
                    chars_saved: check.then_some(*chars_saved),
                }
            }
            Action::Diagnostic(d) => {
                let check_app = matches!(want, Some(Canon::Diagnostic { app: Some(_), .. }));
                Canon::Diagnostic {
                    code: d.reason.code().to_owned(),
                    snippet: d.snippet.map(|s| self.name(s)),
                    app: if check_app {
                        d.app.as_ref().map(|a| a.0.clone())
                    } else {
                        None
                    },
                }
            }
        }
    }
}

fn count_actions(step: &Step) -> usize {
    [
        step.typed.is_some(),
        step.backspace.is_some(),
        step.key.is_some(),
        step.focus.is_some(),
        step.secure.is_some(),
        step.pause.is_some(),
        step.compose.is_some(),
        step.overflow.is_some(),
        step.submit.is_some(),
        step.cancel.is_some(),
        step.inject.is_some(),
        step.snapshot_changed.is_some(),
        step.snapshot.is_some(),
        step.expand.is_some(),
        step.reopen.is_some(),
        step.forget.is_some(),
        step.set_clipboard.is_some(),
        step.clear_clipboard.is_some(),
        step.set_now.is_some(),
    ]
    .into_iter()
    .filter(|b| *b)
    .count()
}

fn summary(step: &Step) -> String {
    let flags = [
        ("pause", step.pause),
        ("compose", step.compose),
        ("overflow", step.overflow),
        ("cancel", step.cancel),
        ("snapshot_changed", step.snapshot_changed),
        ("reopen", step.reopen),
        ("forget", step.forget),
        ("clear_clipboard", step.clear_clipboard),
    ];
    let texts = [
        ("type", &step.typed),
        ("key", &step.key),
        ("focus", &step.focus),
        ("secure", &step.secure),
        ("inject", &step.inject),
        ("expand", &step.expand),
        ("set_clipboard", &step.set_clipboard),
        ("set_now", &step.set_now),
    ];
    let mut parts: Vec<String> = Vec::new();
    parts.extend(
        flags
            .iter()
            .filter_map(|(k, v)| v.map(|v| format!("{k} = {v}"))),
    );
    parts.extend(
        texts
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k} = {v:?}"))),
    );
    if let Some(n) = step.backspace {
        parts.push(format!("backspace = {n}"));
    }
    if let Some(v) = &step.submit {
        parts.push(format!("submit = {v:?}"));
    }
    if let Some(v) = &step.snapshot {
        parts.push(format!("snapshot of {} snippets", v.len()));
    }
    parts.join(", ")
}

fn canon_expect(e: &Expect) -> Result<Canon, String> {
    let set = [
        e.delete.is_some(),
        e.text.is_some(),
        e.key.is_some(),
        e.caret_back.is_some(),
        e.wait.is_some(),
        e.form.is_some(),
        e.expanded.is_some(),
        e.diagnostic.is_some(),
        e.paste.is_some(),
    ]
    .into_iter()
    .filter(|b| *b)
    .count();
    if set != 1 {
        return Err(format!("an expected action needs exactly one kind: {e:?}"));
    }
    Ok(if let Some(n) = e.delete {
        Canon::Delete(n)
    } else if let Some(t) = &e.text {
        Canon::Text(t.clone(), e.verbatim.clone().unwrap_or_default())
    } else if let Some(k) = &e.key {
        if KeyName::from_name(k).is_none() {
            return Err(format!("unknown key {k:?}"));
        }
        Canon::Key(k.clone(), e.shift.unwrap_or(false), e.times.unwrap_or(1))
    } else if let Some(n) = e.caret_back {
        Canon::CaretBack(n)
    } else if let Some(ms) = e.wait {
        Canon::Wait(ms)
    } else if let Some(p) = &e.paste {
        Canon::Paste(p.clone(), e.html.clone())
    } else if let Some(s) = &e.form {
        Canon::Form {
            snippet: s.clone(),
            fields: e.fields.clone(),
            last_values: e.last_values.clone(),
        }
    } else if let Some(s) = &e.expanded {
        Canon::Expanded {
            snippet: s.clone(),
            chars_saved: e.chars_saved,
        }
    } else {
        Canon::Diagnostic {
            code: e.diagnostic.clone().unwrap_or_default(),
            snippet: e.snippet.clone(),
            app: e.app.clone(),
        }
    })
}

fn boundary(key: &str) -> Result<BoundaryKind, String> {
    Ok(match key {
        "enter" => BoundaryKind::Enter,
        "tab" => BoundaryKind::Tab,
        "escape" => BoundaryKind::Escape,
        "arrow" => BoundaryKind::Arrow,
        "navigation" => BoundaryKind::Navigation,
        "click" => BoundaryKind::Click,
        "shortcut" => BoundaryKind::Shortcut,
        "other" => BoundaryKind::Other,
        other => return Err(format!("unknown key {other:?}")),
    })
}

fn secure(state: &str) -> Result<SecureState, String> {
    Ok(match state {
        "normal" => SecureState::Normal,
        "secure" => SecureState::Secure,
        "excluded" => SecureState::Excluded,
        "own_window" => SecureState::OwnWindow,
        "unknown" => SecureState::Unknown,
        other => return Err(format!("unknown secure state {other:?}")),
    })
}

fn parse_now(s: &str) -> Result<LocalDateTime, String> {
    LocalDateTime::parse_iso(s).map_err(|e| format!("now {s:?}: {e}"))
}
