use openfang_runtime::sentry_logs::{capture_structured_log, configure};
use openfang_types::config::SentryConfig;
use serde_json::json;
use std::collections::BTreeMap;

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let dsn = std::env::var("SENTRY_DSN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| args.get(1).cloned())
        .expect("usage: SENTRY_DSN=... cargo run -p openfang-runtime --example sentry_tag_probe -- <event-kind>");
    let event_kind = args
        .get(2)
        .cloned()
        .or_else(|| args.get(1).cloned())
        .unwrap_or_else(|| "ops.guard.sentry_tag_probe".to_string());
    let probe_id = uuid::Uuid::new_v4().to_string();

    let config = SentryConfig {
        dsn: Some(dsn.clone()),
        environment: "production".to_string(),
        include_prompts: false,
        enable_logs: true,
        ..Default::default()
    };
    configure(&config);

    let guard = sentry::init((
        dsn,
        sentry::ClientOptions {
            environment: Some(config.environment.clone().into()),
            send_default_pii: config.include_prompts,
            attach_stacktrace: config.attach_stacktrace,
            traces_sample_rate: config.traces_sample_rate,
            ..Default::default()
        },
    ));

    let attrs = BTreeMap::from([
        ("event.kind".to_string(), json!(event_kind.clone())),
        ("request.id".to_string(), json!(probe_id.clone())),
        ("run.id".to_string(), json!(probe_id.clone())),
        ("session.id".to_string(), json!(probe_id.clone())),
        ("outcome".to_string(), json!("success")),
        ("payload.probe".to_string(), json!(true)),
        ("payload.probe_id".to_string(), json!(probe_id.clone())),
    ]);
    capture_structured_log(sentry::Level::Info, event_kind, attrs);

    let sent = guard.close(Some(std::time::Duration::from_secs(15)));
    if !sent {
        eprintln!("probe flush did not complete before timeout");
        std::process::exit(1);
    }

    println!("{probe_id}");
}
