use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{Json, Router, routing::get};
use bts_cli::{
    cli::{Cli, Command, GroupCommand, StateCommand, TerminalCommand, TerminalTagCommand},
    config::{ColourMode, Environment, OutputMode, ResolvedConfiguration},
    output::OutputStreams,
};
use clap::Parser;
use serde_json::{Value, json};
use tokio::sync::oneshot;

#[test]
fn installed_executable_is_named_btscli_and_help_is_successful() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_btscli"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage: btscli"));
    assert!(stdout.contains("status"));
    assert!(stdout.contains("state"));
    assert!(stdout.contains("terminal"));
    assert!(stdout.contains("group"));

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_btscli"))
        .args(["--output", "json", "state", "watch"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = serde_json::from_slice::<Value>(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "invalid_usage");
}

struct Fixture {
    core_url: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn spawn(app: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_receiver.await;
                })
                .await
                .unwrap();
        });
        Self {
            core_url: format!("http://{address}"),
            shutdown: Some(shutdown),
            task,
        }
    }

    async fn stop(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        self.task.await.unwrap();
    }
}

fn discovery(version: u16) -> Value {
    json!({
        "product": "bts-core",
        "product_version": "0.3.0",
        "administrative_api": {
            "current": version,
            "supported": BTreeSet::from([version]),
            "base_path": format!("/api/v{version}/admin")
        }
    })
}

fn status_resource() -> Value {
    json!({
        "status": "ready",
        "product_version": "0.3.0",
        "administrative_api_version": 1,
        "started_at": "2026-08-07T08:00:00Z"
    })
}

fn state_resource() -> Value {
    json!({
        "captured_at": "2026-08-07T09:00:00Z",
        "state": {
            "display": {
                "screen": "message",
                "title": "Kitchen",
                "body": "Lunch is ready"
            },
            "display_lease": null
        },
        "terminals": {
            "registered": 2,
            "online": 1,
            "groups": 1
        }
    })
}

fn terminal_resource() -> Value {
    json!({
        "id": "bedroom-display",
        "name": "Bedroom",
        "description": "Upstairs display",
        "implementation": "bts-display",
        "approved_capabilities": ["render_text"],
        "tags": ["private"],
        "groups": ["all-displays"],
        "first_seen": "2026-08-07T08:00:00Z",
        "last_seen": "2026-08-07T09:00:00Z"
    })
}

fn group_resource() -> Value {
    json!({
        "id": "all-displays",
        "name": "All displays",
        "members": ["bedroom-display"]
    })
}

fn administrative_app(deletes: Arc<AtomicUsize>) -> Router {
    let delete_terminal = deletes.clone();
    Router::new()
        .route("/api", get(|| async { Json(discovery(1)) }))
        .route(
            "/api/v1/admin/terminals",
            get(|| async { Json(json!({ "terminals": [terminal_resource()] })) }),
        )
        .route(
            "/api/v1/admin/terminals/{terminal}",
            get(|| async { Json(terminal_resource()) }).delete(move || {
                let deletes = delete_terminal.clone();
                async move {
                    deletes.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "deleted": terminal_resource() }))
                }
            }),
        )
        .route(
            "/api/v1/admin/terminals/{terminal}/name",
            axum::routing::put(|| async {
                Json(json!({ "changed": true, "resource": terminal_resource() }))
            }),
        )
        .route(
            "/api/v1/admin/terminals/{terminal}/tags",
            axum::routing::patch(|| async {
                Json(json!({ "changed": false, "resource": terminal_resource() }))
            }),
        )
        .route(
            "/api/v1/admin/groups",
            get(|| async { Json(json!({ "groups": [group_resource()] })) })
                .post(|| async { (axum::http::StatusCode::CREATED, Json(group_resource())) }),
        )
        .route(
            "/api/v1/admin/groups/{group}",
            get(|| async { Json(group_resource()) })
                .delete(|| async { Json(json!({ "deleted": group_resource() })) }),
        )
        .route(
            "/api/v1/admin/groups/{group}/name",
            axum::routing::put(|| async {
                Json(json!({ "changed": true, "resource": group_resource() }))
            }),
        )
        .route(
            "/api/v1/admin/groups/{group}/members",
            axum::routing::patch(|| async {
                Json(json!({ "changed": false, "resource": group_resource() }))
            }),
        )
}

fn compatible_app() -> Router {
    Router::new()
        .route("/api", get(|| async { Json(discovery(1)) }))
        .route(
            "/api/v1/admin/status",
            get(|| async { Json(status_resource()) }),
        )
        .route(
            "/api/v1/admin/state",
            get(|| async { Json(state_resource()) }),
        )
}

async fn invoke(
    args: &[&str],
    environment: &Environment,
    stdout_is_terminal: bool,
    stderr_is_terminal: bool,
) -> (u8, String, String) {
    invoke_with_input(
        args,
        environment,
        "",
        false,
        stdout_is_terminal,
        stderr_is_terminal,
    )
    .await
}

async fn invoke_with_input(
    args: &[&str],
    environment: &Environment,
    input: &str,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
    stderr_is_terminal: bool,
) -> (u8, String, String) {
    let cli = Cli::try_parse_from(args).unwrap();
    let mut stdin = std::io::Cursor::new(input.as_bytes().to_vec());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = bts_cli::execute(
        cli,
        environment,
        OutputStreams {
            stdin: &mut stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_is_terminal,
            stdout_is_terminal,
            stderr_is_terminal,
        },
    )
    .await;
    (
        code,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

#[test]
fn parsing_exposes_the_frozen_administrative_grammar_and_global_options() {
    let status = Cli::try_parse_from([
        "btscli",
        "status",
        "--core",
        "http://core.example",
        "--output",
        "json",
        "--timeout",
        "3s",
        "--colour",
        "never",
        "-vv",
    ])
    .unwrap();
    assert!(matches!(status.command, Command::Status));
    assert_eq!(status.verbosity, 2);

    let state = Cli::try_parse_from(["btscli", "--quiet", "state", "show"]).unwrap();
    assert!(matches!(
        state.command,
        Command::State {
            command: StateCommand::Show
        }
    ));

    assert!(Cli::try_parse_from(["btscli", "state", "watch"]).is_err());
    assert!(matches!(
        Cli::try_parse_from(["btscli", "terminal", "list"])
            .unwrap()
            .command,
        Command::Terminal {
            command: TerminalCommand::List
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["btscli", "terminal", "tag", "add", "bedroom", "private"])
            .unwrap()
            .command,
        Command::Terminal {
            command: TerminalCommand::Tag {
                command: TerminalTagCommand::Add { .. }
            }
        }
    ));
    assert!(matches!(
        Cli::try_parse_from([
            "btscli",
            "group",
            "create",
            "all-displays",
            "--name",
            "All displays"
        ])
        .unwrap()
        .command,
        Command::Group {
            command: GroupCommand::Create { .. }
        }
    ));
    assert!(Cli::try_parse_from(["btscli", "terminal", "tag", "add", "bedroom"]).is_err());
    assert!(Cli::try_parse_from(["btscli", "group", "add", "all-displays"]).is_err());
    assert!(Cli::try_parse_from(["btscli", "--quiet", "-v", "status"]).is_err());
}

#[test]
fn configuration_uses_cli_then_environment_then_defaults() {
    let defaults = Cli::try_parse_from(["btscli", "status"]).unwrap();
    let configuration = ResolvedConfiguration::resolve(&defaults, &Environment::default()).unwrap();
    assert_eq!(configuration.core_url, "http://127.0.0.1:3100");
    assert_eq!(configuration.timeout, Duration::from_secs(10));
    assert_eq!(configuration.output, OutputMode::Human);
    assert_eq!(configuration.colour, ColourMode::Auto);

    let environment = Environment::from_pairs([
        ("BTS_CORE_URL", "http://environment.example"),
        ("BTSCLI_TIMEOUT", "2m"),
        ("BTSCLI_OUTPUT", "json"),
        ("BTSCLI_COLOUR", "always"),
    ]);
    let cli = Cli::try_parse_from([
        "btscli",
        "--core",
        "http://argument.example",
        "--timeout",
        "250ms",
        "--output",
        "human",
        "--colour",
        "never",
        "status",
    ])
    .unwrap();
    let configuration = ResolvedConfiguration::resolve(&cli, &environment).unwrap();
    assert_eq!(configuration.core_url, "http://argument.example");
    assert_eq!(configuration.timeout, Duration::from_millis(250));
    assert_eq!(configuration.output, OutputMode::Human);
    assert_eq!(configuration.colour, ColourMode::Never);

    let no_colour = Environment::from_pairs([("NO_COLOR", "")]);
    let configuration = ResolvedConfiguration::resolve(&defaults, &no_colour).unwrap();
    assert_eq!(configuration.colour, ColourMode::Never);
    let explicit_auto = Cli::try_parse_from(["btscli", "--colour", "auto", "status"]).unwrap();
    let configuration = ResolvedConfiguration::resolve(&explicit_auto, &no_colour).unwrap();
    assert_eq!(configuration.colour, ColourMode::Auto);
}

#[tokio::test]
async fn status_has_semantic_human_and_exact_json_output() {
    let fixture = Fixture::spawn(compatible_app()).await;
    let environment = Environment::from_pairs([("BTS_CORE_URL", &fixture.core_url)]);

    let (code, stdout, stderr) = invoke(&["btscli", "status"], &environment, false, false).await;
    assert_eq!(code, 0);
    assert!(stderr.is_empty());
    assert!(stdout.contains("Core status: ready"));
    assert!(stdout.contains("Version: 0.3.0"));
    assert!(stdout.contains("Administrative API: v1"));
    assert!(!stdout.contains("\u{1b}["));

    let (code, stdout, stderr) = invoke(
        &["btscli", "--output", "json", "status"],
        &environment,
        true,
        true,
    )
    .await;
    assert_eq!(code, 0);
    assert!(stderr.is_empty());
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap(),
        status_resource()
    );
    assert!(!stdout.contains("\u{1b}["));
    assert!(stdout.ends_with('\n'));
    fixture.stop().await;
}

#[tokio::test]
async fn state_show_human_output_is_useful_and_json_is_exact() {
    let fixture = Fixture::spawn(compatible_app()).await;
    let environment = Environment::from_pairs([("BTS_CORE_URL", &fixture.core_url)]);

    let (code, stdout, stderr) =
        invoke(&["btscli", "state", "show"], &environment, false, false).await;
    assert_eq!(code, 0);
    assert!(stderr.is_empty());
    assert!(stdout.contains("Display: message — Kitchen: Lunch is ready"));
    assert!(stdout.contains("Terminals: 2 registered, 1 online"));
    assert!(stdout.contains("Groups: 1"));

    let (code, stdout, _) = invoke(
        &["btscli", "state", "show", "--output", "json"],
        &environment,
        false,
        false,
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap(),
        state_resource()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn terminal_and_group_commands_have_human_and_exact_json_output() {
    let fixture = Fixture::spawn(administrative_app(Arc::new(AtomicUsize::new(0)))).await;
    let environment = Environment::from_pairs([("BTS_CORE_URL", &fixture.core_url)]);

    let (code, stdout, stderr) =
        invoke(&["btscli", "terminal", "list"], &environment, false, false).await;
    assert_eq!(code, 0);
    assert!(stderr.is_empty());
    assert!(stdout.contains("bedroom-display\tBedroom\toffline\tbts-display"));

    let (code, stdout, stderr) = invoke(
        &["btscli", "--output", "json", "terminal", "show", "Bedroom"],
        &environment,
        false,
        false,
    )
    .await;
    assert_eq!(code, 0);
    assert!(stderr.is_empty());
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap(),
        terminal_resource()
    );

    let (code, stdout, _) = invoke(
        &["btscli", "group", "show", "all-displays"],
        &environment,
        false,
        false,
    )
    .await;
    assert_eq!(code, 0);
    assert!(stdout.contains("Terminal group: All displays (all-displays)"));

    let (code, stdout, _) = invoke(
        &[
            "btscli",
            "--output",
            "json",
            "terminal",
            "tag",
            "add",
            "bedroom-display",
            "private",
        ],
        &environment,
        false,
        false,
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap()["changed"],
        false
    );
    fixture.stop().await;
}

#[tokio::test]
async fn destructive_commands_require_confirmation_and_yes_only_skips_the_prompt() {
    let deletes = Arc::new(AtomicUsize::new(0));
    let fixture = Fixture::spawn(administrative_app(deletes.clone())).await;
    let environment = Environment::from_pairs([("BTS_CORE_URL", &fixture.core_url)]);

    let (code, stdout, stderr) = invoke(
        &["btscli", "terminal", "forget", "bedroom-display"],
        &environment,
        false,
        false,
    )
    .await;
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("requires --yes"));
    assert_eq!(deletes.load(Ordering::SeqCst), 0);

    let (code, stdout, stderr) = invoke(
        &[
            "btscli",
            "--output",
            "json",
            "--yes",
            "terminal",
            "forget",
            "bedroom-display",
        ],
        &environment,
        false,
        false,
    )
    .await;
    assert_eq!(code, 0);
    assert!(stderr.is_empty());
    assert_eq!(deletes.load(Ordering::SeqCst), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap()["deleted"]["id"],
        "bedroom-display"
    );

    let (code, _, stderr) = invoke_with_input(
        &["btscli", "group", "delete", "all-displays"],
        &environment,
        "no\n",
        true,
        false,
        true,
    )
    .await;
    assert_eq!(code, 2);
    assert!(stderr.contains("Delete terminal group All displays (all-displays)?"));
    assert!(stderr.contains("was not confirmed"));
    fixture.stop().await;
}

#[tokio::test]
async fn quiet_verbosity_and_colour_respect_stream_policy() {
    let fixture = Fixture::spawn(compatible_app()).await;
    let environment = Environment::from_pairs([("BTS_CORE_URL", &fixture.core_url)]);

    let (code, stdout, stderr) =
        invoke(&["btscli", "--quiet", "status"], &environment, false, false).await;
    assert_eq!(code, 0);
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());

    let (code, stdout, stderr) = invoke(
        &["btscli", "--colour", "always", "status"],
        &environment,
        false,
        false,
    )
    .await;
    assert_eq!(code, 0);
    assert!(stdout.contains("\u{1b}["));
    assert!(stderr.is_empty());

    let (code, stdout, stderr) =
        invoke(&["btscli", "-v", "status"], &environment, false, false).await;
    assert_eq!(code, 0);
    assert!(!stdout.is_empty());
    assert_eq!(stderr, "Requesting status from Core\n");
    fixture.stop().await;
}

#[tokio::test]
async fn invalid_unavailable_and_incompatible_failures_have_stable_exit_codes() {
    let invalid_environment = Environment::from_pairs([("BTSCLI_TIMEOUT", "0s")]);
    let (code, stdout, stderr) = invoke(
        &["btscli", "--output", "json", "status"],
        &invalid_environment,
        false,
        false,
    )
    .await;
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    let error = serde_json::from_str::<Value>(&stderr).unwrap();
    assert_eq!(error["error"]["category"], "invalid_input");
    assert_eq!(error["error"]["code"], "invalid_configuration");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let unavailable = Environment::from_pairs([("BTS_CORE_URL", format!("http://{address}"))]);
    let (code, stdout, stderr) = invoke(&["btscli", "status"], &unavailable, false, false).await;
    assert_eq!(code, 3);
    assert!(stdout.is_empty());
    assert_eq!(stderr, "Error: Core is unavailable\n");
    assert!(!stderr.contains("reqwest"));

    let fixture =
        Fixture::spawn(Router::new().route("/api", get(|| async { Json(discovery(2)) }))).await;
    let incompatible = Environment::from_pairs([("BTS_CORE_URL", &fixture.core_url)]);
    let (code, stdout, stderr) = invoke(
        &["btscli", "--output", "json", "status"],
        &incompatible,
        false,
        false,
    )
    .await;
    assert_eq!(code, 4);
    assert!(stdout.is_empty());
    let error = serde_json::from_str::<Value>(&stderr).unwrap();
    assert_eq!(error["error"]["category"], "incompatible_api");
    assert_eq!(error["error"]["code"], "unsupported_administrative_api");
    fixture.stop().await;
}

#[tokio::test]
async fn quiet_json_and_invalid_environment_values_are_configuration_errors() {
    let (code, stdout, stderr) = invoke(
        &["btscli", "--quiet", "--output", "json", "status"],
        &Environment::default(),
        false,
        false,
    )
    .await;
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert_eq!(
        serde_json::from_str::<Value>(&stderr).unwrap()["error"]["code"],
        "invalid_configuration"
    );

    let (code, _, stderr) = invoke(
        &["btscli", "--colour", "always", "--timeout", "0s", "status"],
        &Environment::default(),
        true,
        true,
    )
    .await;
    assert_eq!(code, 2);
    assert!(stderr.contains("\u{1b}["));

    for (key, value) in [
        ("BTS_CORE_URL", ""),
        ("BTSCLI_TIMEOUT", "ten seconds"),
        ("BTSCLI_OUTPUT", "yaml"),
        ("BTSCLI_COLOUR", "sometimes"),
    ] {
        let cli = Cli::try_parse_from(["btscli", "status"]).unwrap();
        assert!(
            ResolvedConfiguration::resolve(&cli, &Environment::from_pairs([(key, value)])).is_err(),
            "{key}={value:?}"
        );
    }
}
