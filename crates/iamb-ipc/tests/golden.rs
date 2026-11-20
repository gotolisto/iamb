//! Contract tests: one golden JSON file per message type, in `tests/golden/`.
//!
//! For every sample below: the sample's encoding equals its golden file (compared as JSON
//! values, so formatting does not matter), and decoding the golden file gives the sample back.
//! Every request, response and event name has a sample. Run with `IAMB_BLESS=1` to rewrite the
//! files after a deliberate change to the protocol, and review the diff.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use iamb_core::model::{
    Abbreviation, Folder, FolderDefaults, Id, Sha256, Snippet, SyncMeta, Tag, Timestamp,
};
use iamb_ipc::{
    AttachmentRef, ChangedRows, ConflictPolicy, ErrorBody, ErrorCode, EventBody, ExcludedEntry,
    FocusRef, GrantState, HealthReport, Hello, InjectionStatus, PROTOCOL_VERSION, PanelKind,
    PendingReview, PermissionName, PreviewBlank, PreviewCursor, PreviewPart, RemoteRow, Request,
    RequestBody, Response, ResponseBody, Role, SearchField, SearchHit, Secret, ServiceStatus,
    ShadowedEntry, SnippetQuery, SnippetSummary, SyncStatus, TagEntry, TemplateAttribute,
    TemplateErrorInfo, TemplateToken, TransferFormat, UnusedEntry, Welcome, Wire, WireAction,
    WireAttachment, WireDiagnostic, WireExclusion, WireField, WireOutboxEntry,
};

fn id(n: u8) -> Id {
    Id::from_bytes([n; 16])
}

fn meta(n: u8) -> SyncMeta {
    SyncMeta::new(id(n), Timestamp(1_763_400_000_000))
}

fn snippet() -> Snippet {
    Snippet::new_defaults(
        meta(1),
        id(2),
        Abbreviation::new("brb").expect("abbreviation"),
        "Back soon",
        "be right back",
        None,
    )
}

fn folder() -> Folder {
    Folder {
        meta: meta(2),
        profile_id: id(9),
        parent_id: None,
        name: "Snippets".to_owned(),
        position: 0,
        member_permission: None,
        defaults: FolderDefaults::default(),
    }
}

fn tag() -> Tag {
    Tag {
        meta: meta(3),
        profile_id: id(9),
        name: "work".to_owned(),
        color: Some("blue".to_owned()),
    }
}

fn sha() -> Sha256 {
    Sha256::from_bytes([0xab; 32])
}

fn values() -> Secret<BTreeMap<String, String>> {
    Secret::new(BTreeMap::from([("Name".to_owned(), "Ada".to_owned())]))
}

fn status() -> ServiceStatus {
    ServiceStatus {
        version: "0.0.0".to_owned(),
        pid: 4242,
        paused: false,
        suspended: true,
        injection: InjectionStatus::Idle,
        snippets: 12,
        revision: 3,
        listener: true,
        memory_locked: true,
        clients: 1,
        ui_connected: false,
    }
}

fn diagnostic() -> WireDiagnostic {
    WireDiagnostic {
        code: "injection_cancelled".to_owned(),
        snippet: Some(id(1)),
        expansion: Some(7),
        app: None,
        at: Timestamp(1_763_400_000_500),
    }
}

fn field() -> WireField {
    WireField {
        name: "Size".to_owned(),
        kind: "choice".to_owned(),
        options: vec!["S".to_owned(), "M".to_owned()],
        default: "M".to_owned(),
    }
}

#[allow(clippy::too_many_lines, reason = "one sample per message")]
fn requests() -> Vec<RequestBody> {
    vec![
        RequestBody::ListFolders {},
        RequestBody::ListSnippets {
            query: SnippetQuery {
                folder: Some(id(2)),
                tag: None,
            },
        },
        RequestBody::GetSnippet { id: id(1) },
        RequestBody::PutSnippet {
            snippet: snippet(),
            tags: Some(vec![id(3)]),
        },
        RequestBody::DeleteSnippet { id: id(1) },
        RequestBody::PutFolder { folder: folder() },
        RequestBody::DeleteFolder { id: id(2) },
        RequestBody::PutTag { tag: tag() },
        RequestBody::ListTags {},
        RequestBody::DeleteTag { id: id(3) },
        RequestBody::ListSnippetSummaries {
            query: SnippetQuery {
                folder: None,
                tag: Some(id(3)),
            },
            offset: 200,
            limit: 100,
        },
        RequestBody::CheckAbbreviation {
            abbreviation: ";rl".to_owned(),
            trigger: "immediate".to_owned(),
            snippet: Some(id(1)),
        },
        RequestBody::Search {
            text: Secret::new("right".to_owned()),
            limit: 20,
        },
        RequestBody::Import {
            path: PathBuf::from("/tmp/library.json"),
            format: TransferFormat::Json,
            on_conflict: ConflictPolicy::Skip,
        },
        RequestBody::Export {
            path: PathBuf::from("/tmp/library.csv"),
            format: TransferFormat::Csv,
        },
        RequestBody::Health {},
        RequestBody::ListPendingReviews {},
        RequestBody::ApproveSnippet { id: id(1), rev: 5 },
        RequestBody::GetStatus {},
        RequestBody::SetPaused { paused: true },
        RequestBody::ExpandSnippet {
            id: id(1),
            focus: Some(FocusRef {
                pid: 100,
                window: 3,
            }),
        },
        RequestBody::DryRun {
            typed: Secret::new("brb ".to_owned()),
            now: Some("2026-03-18T14:05:09".to_owned()),
            clipboard: Some(Secret::new("copied".to_owned())),
            values: Some(values()),
        },
        RequestBody::GetPermissions {},
        RequestBody::OpenPermissionSettings {
            kind: PermissionName::InputMonitoring,
        },
        RequestBody::SetTestField {
            window: Some(FocusRef { pid: 7, window: 8 }),
        },
        RequestBody::FormAck { session: 2 },
        RequestBody::FormSubmit {
            session: 2,
            values: values(),
        },
        RequestBody::FormCancel { session: 2 },
        RequestBody::ReopenLastForm {},
        RequestBody::PanelFocus {
            panel: PanelKind::Form,
            shown: true,
        },
        RequestBody::ParseTemplate {
            source: "Hi {field name=Name}".to_owned(),
        },
        RequestBody::RenderPreview {
            source: "Hi {field name=Name}".to_owned(),
            values: values(),
            rich: false,
        },
        RequestBody::RecentDiagnostics {},
        RequestBody::GetSettings {},
        RequestBody::PutSetting {
            key: "date_format".to_owned(),
            value: Some("%d.%m.%Y".to_owned()),
        },
        RequestBody::ListExclusions {},
        RequestBody::PutExclusion {
            app_id: "org.example.bank".to_owned(),
            display_name: "Bank".to_owned(),
            excluded: true,
        },
        RequestBody::OutboxRead { limit: 100 },
        RequestBody::OutboxAck { ids: vec![1, 2, 3] },
        RequestBody::ApplyRemote {
            batch: vec![RemoteRow::Snippet(snippet()), RemoteRow::Tag(tag())],
        },
        RequestBody::PurgeFolder { id: id(2) },
        RequestBody::AttachmentAvailable {
            profile: id(9),
            sha256: sha(),
        },
        RequestBody::MissingAttachments {},
        RequestBody::SetSyncStatus {
            status: SyncStatus {
                signed_in: true,
                last_sync: Some(Timestamp(1_763_400_000_000)),
                error: None,
            },
        },
        RequestBody::SetEntitlements {
            entitlements: BTreeMap::from([("shared_folders".to_owned(), "10".to_owned())]),
        },
        RequestBody::PutAttachment {
            path: PathBuf::from("/tmp/iamb-drop/logo.png"),
            file_name: "logo.png".to_owned(),
            folder: id(2),
        },
        RequestBody::RecentSnippets { limit: 8 },
        RequestBody::ResetUsage {},
    ]
}

#[allow(clippy::too_many_lines, reason = "one sample per message")]
fn responses() -> Vec<ResponseBody> {
    vec![
        ResponseBody::Done {},
        ResponseBody::Folders {
            profile: id(9),
            folders: vec![folder()],
        },
        ResponseBody::Snippets {
            snippets: vec![snippet()],
            tags: BTreeMap::from([(id(1), vec![id(3)])]),
        },
        ResponseBody::Snippet {
            snippet: snippet(),
            tags: vec![id(3)],
        },
        ResponseBody::Saved { meta: meta(1) },
        ResponseBody::SearchResults {
            hits: vec![SearchHit {
                snippet: id(1),
                score: 90,
                field: Some(SearchField::Label),
                ranges: vec![[0, 4]],
            }],
        },
        ResponseBody::Imported {
            snippets_imported: 10,
            snippets_skipped: 1,
            folders_created: 2,
            tags_created: 0,
        },
        ResponseBody::Exported { snippets: 11 },
        ResponseBody::Health {
            report: HealthReport {
                shadowed: vec![ShadowedEntry {
                    by: id(4),
                    hidden: id(5),
                }],
                excluded: vec![
                    ExcludedEntry {
                        snippet: id(6),
                        reason: "duplicate".to_owned(),
                        kept: Some(id(1)),
                        reference: None,
                        included: None,
                    },
                    ExcludedEntry {
                        snippet: id(7),
                        reason: "E_SNIPPET_NOT_FOUND".to_owned(),
                        kept: None,
                        reference: Some(";sig2".to_owned()),
                        included: Some(";sig".to_owned()),
                    },
                ],
                unused: vec![UnusedEntry {
                    snippet: id(8),
                    last_used: Some(Timestamp(1_755_000_000_000)),
                }],
            },
        },
        ResponseBody::PendingReviews {
            reviews: vec![PendingReview {
                snippet: id(1),
                rev: 4,
                author: id(8),
            }],
        },
        ResponseBody::Status { status: status() },
        ResponseBody::DryRun {
            actions: vec![
                WireAction::DeleteBack { n: 4 },
                WireAction::TypeText {
                    text: "be right back".to_owned(),
                    verbatim: vec![[0, 2]],
                },
                WireAction::Paste {
                    plain: "x".to_owned(),
                    markdown_html: "<p>x</p>".to_owned(),
                    attachments: vec![sha()],
                },
                WireAction::PressKey {
                    key: "enter".to_owned(),
                    shift: false,
                    times: 1,
                },
                WireAction::MoveCaretBack { n: 2 },
                WireAction::Wait { ms: 100 },
                WireAction::ShowForm {
                    session: 1,
                    snippet: id(1),
                    fields: vec![field()],
                },
                WireAction::Expanded {
                    expansion: 1,
                    snippet: id(1),
                    chars_saved: 10,
                },
                WireAction::Diagnostic {
                    diagnostic: diagnostic(),
                },
            ],
            hints: vec![WireDiagnostic::code(
                "E_UNKNOWN_ATTR",
                Timestamp(1_763_400_000_600),
            )],
        },
        ResponseBody::Permissions {
            accessibility: GrantState::Granted,
            input_monitoring: GrantState::Denied,
        },
        ResponseBody::TemplateTokens {
            tokens: vec![
                TemplateToken {
                    kind: "text".to_owned(),
                    start: 0,
                    end: 3,
                    attributes: Vec::new(),
                },
                TemplateToken {
                    kind: "field".to_owned(),
                    start: 3,
                    end: 20,
                    attributes: vec![TemplateAttribute {
                        name: "name".to_owned(),
                        value: "Name".to_owned(),
                    }],
                },
            ],
            error: Some(TemplateErrorInfo {
                code: "E_UNKNOWN_ATTR".to_owned(),
                start: Some(10),
                end: Some(20),
                snippet: None,
            }),
        },
        ResponseBody::Preview {
            parts: vec![
                PreviewPart::Text {
                    text: "Hi Name, Clipboard".to_owned(),
                    blanks: vec![PreviewBlank {
                        start: 3,
                        end: 7,
                        field: "Name".to_owned(),
                        sample: true,
                    }],
                    clipboard: vec![[9, 18]],
                },
                PreviewPart::Key {
                    key: "tab".to_owned(),
                    shift: false,
                    times: 1,
                },
                PreviewPart::Wait { ms: 500 },
                PreviewPart::Formatted {
                    html: format!(
                        "<p><strong>Hi</strong> <img src=\"attachment:{}\" alt=\"logo\"></p>",
                        sha()
                    ),
                    plain: "Hi logo".to_owned(),
                    attachments: vec![WireAttachment {
                        sha256: sha(),
                        mime: "image/png".to_owned(),
                        size: 1234,
                        file_name: "logo.png".to_owned(),
                        path: None,
                    }],
                },
            ],
            cursor: Some(PreviewCursor { part: 0, offset: 7 }),
            fields: vec![field()],
            error: None,
        },
        ResponseBody::Diagnostics {
            entries: vec![diagnostic()],
        },
        ResponseBody::Settings {
            settings: BTreeMap::from([("locale".to_owned(), "de".to_owned())]),
        },
        ResponseBody::Exclusions {
            exclusions: vec![WireExclusion {
                app_id: "org.example.bank".to_owned(),
                display_name: "Bank".to_owned(),
                created_at: Timestamp(1_763_400_000_000),
            }],
        },
        ResponseBody::Outbox {
            entries: vec![WireOutboxEntry {
                seq: 1,
                entity: "snippet".to_owned(),
                row_id: id(1),
                op: "put".to_owned(),
                base_rev: 0,
                created_at: Timestamp(1_763_400_000_000),
            }],
        },
        ResponseBody::Acked { count: 3 },
        ResponseBody::MissingAttachments {
            attachments: vec![AttachmentRef {
                profile: id(9),
                sha256: sha(),
            }],
        },
        ResponseBody::Tags {
            tags: vec![TagEntry {
                tag: tag(),
                snippets: 12,
            }],
        },
        ResponseBody::SnippetSummaries {
            revision: 4,
            total: 250,
            summaries: vec![SnippetSummary {
                id: id(1),
                folder: id(2),
                abbreviation: "brb".to_owned(),
                label: "Back soon".to_owned(),
                first_line: "be right back".to_owned(),
                kind: "plain".to_owned(),
                trigger: "delimiter".to_owned(),
                enabled: true,
                tags: vec![id(3)],
                broken: None,
                uses: Some(31),
                last_used: Some(Timestamp(1_763_390_000_000)),
                updated_at: Timestamp(1_763_400_000_000),
            }],
            next: Some(201),
        },
        ResponseBody::AbbreviationCheck {
            conflict: Some(id(4)),
            hides: vec![id(5)],
            hidden_by: Vec::new(),
        },
        ResponseBody::Attachment {
            attachment: WireAttachment {
                sha256: sha(),
                mime: "image/png".to_owned(),
                size: 1234,
                file_name: "logo.png".to_owned(),
                path: Some(PathBuf::from("/data/attachments/p/abab")),
            },
        },
    ]
}

fn events() -> Vec<EventBody> {
    vec![
        EventBody::LibraryChanged {
            revision: 4,
            changed: Some(ChangedRows {
                snippets: vec![id(1)],
                folders: Vec::new(),
                tags: vec![id(3)],
            }),
        },
        EventBody::StatusChanged { status: status() },
        EventBody::PaletteRequested {
            focus: FocusRef {
                pid: 100,
                window: 3,
            },
        },
        EventBody::FormRequested {
            session: 2,
            snippet: id(1),
            spec: vec![field()],
            focus: Some(FocusRef {
                pid: 100,
                window: 3,
            }),
            last_values: Some(values()),
        },
        EventBody::DiagnosticAdded {
            entry: diagnostic(),
        },
        EventBody::SettingsChanged {},
        EventBody::OutboxChanged { pending: 2 },
    ]
}

/// Every sample with the name of its golden file.
fn samples() -> Vec<(String, Wire)> {
    let mut out = vec![
        (
            "hello".to_owned(),
            Wire::Hello(Hello {
                protocol: PROTOCOL_VERSION,
                role: Role::Ui,
                client_version: "0.0.0".to_owned(),
            }),
        ),
        (
            "welcome".to_owned(),
            Wire::Welcome(Welcome {
                protocol: PROTOCOL_VERSION,
                server_version: "0.0.0".to_owned(),
            }),
        ),
        (
            "refused".to_owned(),
            Wire::Refused {
                error: ErrorBody::new(ErrorCode::UnsupportedProtocol, "update Iamb"),
            },
        ),
        (
            "response_error".to_owned(),
            Wire::Response(Response::new(
                9,
                Err(ErrorBody::new(ErrorCode::NotPermitted, "not for this role")),
            )),
        ),
    ];
    for (i, body) in requests().into_iter().enumerate() {
        out.push((
            format!("request_{}", body.name()),
            Wire::Request(Request {
                id: u64::try_from(i).expect("small") + 1,
                body,
            }),
        ));
    }
    for (i, body) in responses().into_iter().enumerate() {
        out.push((
            format!("response_{}", body.name()),
            Wire::Response(Response::new(
                u64::try_from(i).expect("small") + 1,
                Ok(body),
            )),
        ));
    }
    for body in events() {
        out.push((format!("event_{}", body.name()), Wire::Event(body)));
    }
    out
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[test]
fn every_message_matches_its_golden_file_and_round_trips() {
    let bless = std::env::var_os("IAMB_BLESS").is_some();
    let dir = golden_dir();
    let mut problems = Vec::new();
    for (name, wire) in samples() {
        let path = dir.join(format!("{name}.json"));
        let encoded = serde_json::to_value(&wire).expect("encodes");
        if bless {
            let mut text = serde_json::to_string_pretty(&encoded).expect("pretty");
            text.push('\n');
            fs::write(&path, text).expect("golden written");
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            problems.push(format!("{name}: no golden file (run with IAMB_BLESS=1)"));
            continue;
        };
        let golden: serde_json::Value = serde_json::from_str(&text).expect("golden is JSON");
        if golden != encoded {
            problems.push(format!("{name}: encoding differs from the golden file"));
        }
        let decoded = Wire::decode(text.as_bytes()).expect("golden decodes");
        if decoded != wire {
            problems.push(format!(
                "{name}: decoding the golden file gives another message"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_message_name_has_a_sample() {
    let requests: Vec<&str> = requests().iter().map(RequestBody::name).collect();
    assert_eq!(requests, RequestBody::NAMES);
    let responses: Vec<&str> = responses().iter().map(ResponseBody::name).collect();
    assert_eq!(responses, ResponseBody::NAMES);
    let events: Vec<&str> = events().iter().map(EventBody::name).collect();
    assert_eq!(events, EventBody::NAMES);
}

#[test]
fn there_are_no_stale_golden_files() {
    let names: Vec<String> = samples().into_iter().map(|(n, _)| n).collect();
    for entry in fs::read_dir(golden_dir()).expect("golden dir") {
        let file = entry.expect("entry").file_name();
        let file = file.to_string_lossy();
        let stem = file.trim_end_matches(".json");
        assert!(
            names.iter().any(|n| n == stem),
            "{file} has no sample; remove it or add one"
        );
    }
}
