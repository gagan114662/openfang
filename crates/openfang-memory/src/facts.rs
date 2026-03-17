//! SQLite-backed canonical fact and artifact stores.

use chrono::Utc;
use openfang_types::error::{OpenFangError, OpenFangResult};
use openfang_types::facts::{
    ArtifactRecord, CanonicalEvent, FactEventFilter, FactEventRecord, RunRecord,
};
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct FactEventStore {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Clone)]
pub struct ArtifactIndexStore {
    conn: Arc<Mutex<Connection>>,
}

impl FactEventStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    pub fn record(&self, event: &CanonicalEvent) -> OpenFangResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| OpenFangError::Internal(e.to_string()))?;
        let json = serde_json::to_string(event)
            .map_err(|e| OpenFangError::Serialization(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO fact_events (
                event_id, occurred_at, event_kind, agent_id, run_id, session_id, request_id,
                trace_id, outcome, event_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event.event.id,
                event.occurred_at,
                event.event.kind,
                event.agent.id,
                event.run.id,
                event.session.id,
                event.request.id,
                event.trace.id,
                event.outcome,
                json,
            ],
        )
        .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        Ok(())
    }

    pub fn exists(&self, event_id: &str) -> OpenFangResult<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| OpenFangError::Internal(e.to_string()))?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fact_events WHERE event_id = ?1",
                params![event_id],
                |row| row.get(0),
            )
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        Ok(count > 0)
    }

    pub fn has_event_kind_prefix(&self, prefix: &str) -> OpenFangResult<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| OpenFangError::Internal(e.to_string()))?;
        let pattern = format!("{prefix}%");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fact_events WHERE event_kind LIKE ?1",
                params![pattern],
                |row| row.get(0),
            )
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        Ok(count > 0)
    }

    pub fn query(
        &self,
        filter: &FactEventFilter,
        order_desc: bool,
    ) -> OpenFangResult<Vec<FactEventRecord>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| OpenFangError::Internal(e.to_string()))?;

        let mut sql = String::from(
            "SELECT event_id, event_kind, occurred_at, agent_id, run_id, session_id, request_id, trace_id, outcome, event_json
             FROM fact_events",
        );
        let mut clauses = Vec::new();
        let mut args: Vec<SqlValue> = Vec::new();

        if let Some(event_kind) = filter.event_kind.as_ref() {
            clauses.push("event_kind = ?");
            args.push(SqlValue::Text(event_kind.clone()));
        }
        if let Some(agent_id) = filter.agent_id.as_ref() {
            clauses.push("agent_id = ?");
            args.push(SqlValue::Text(agent_id.clone()));
        }
        if let Some(run_id) = filter.run_id.as_ref() {
            clauses.push("run_id = ?");
            args.push(SqlValue::Text(run_id.clone()));
        }
        if let Some(session_id) = filter.session_id.as_ref() {
            clauses.push("session_id = ?");
            args.push(SqlValue::Text(session_id.clone()));
        }
        if let Some(request_id) = filter.request_id.as_ref() {
            clauses.push("request_id = ?");
            args.push(SqlValue::Text(request_id.clone()));
        }
        if let Some(since) = filter.since.as_ref() {
            clauses.push("occurred_at >= ?");
            args.push(SqlValue::Text(since.clone()));
        }
        if let Some(until) = filter.until.as_ref() {
            clauses.push("occurred_at <= ?");
            args.push(SqlValue::Text(until.clone()));
        }
        if let Some(outcome) = filter.outcome.as_ref() {
            clauses.push("outcome = ?");
            args.push(SqlValue::Text(outcome.clone()));
        }
        if let Some(cursor) = filter.cursor.as_ref() {
            if let Some((cursor_time, cursor_event_id)) = decode_cursor(cursor) {
                clauses.push("(occurred_at < ? OR (occurred_at = ? AND event_id < ?))");
                args.push(SqlValue::Text(cursor_time.clone()));
                args.push(SqlValue::Text(cursor_time));
                args.push(SqlValue::Text(cursor_event_id));
            }
        }

        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }

        sql.push_str(if order_desc {
            " ORDER BY occurred_at DESC, event_id DESC"
        } else {
            " ORDER BY occurred_at ASC, event_id ASC"
        });

        let limit = filter.limit.unwrap_or(100).min(1000) as i64;
        sql.push_str(" LIMIT ?");
        args.push(SqlValue::Integer(limit));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map(params_from_iter(args.iter()), |row| {
                let event_json: String = row.get(9)?;
                let event: CanonicalEvent = serde_json::from_str(&event_json).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        event_json.len(),
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;
                Ok(FactEventRecord {
                    event_id: row.get(0)?,
                    event_kind: row.get(1)?,
                    occurred_at: row.get(2)?,
                    agent_id: row.get(3)?,
                    run_id: row.get(4)?,
                    session_id: row.get(5)?,
                    request_id: row.get(6)?,
                    trace_id: row.get(7)?,
                    outcome: row.get(8)?,
                    event,
                })
            })
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| OpenFangError::Memory(e.to_string()))?);
        }
        Ok(out)
    }

    pub fn get_run(&self, run_id: &str) -> OpenFangResult<RunRecord> {
        let mut filter = FactEventFilter {
            run_id: Some(run_id.to_string()),
            limit: Some(10_000),
            ..Default::default()
        };
        let events = self.query(&filter, false)?;
        let artifacts = ArtifactIndexStore::new(self.conn.clone()).list_for_run(run_id)?;
        let first = events.first().map(|event| event.occurred_at.clone());
        let last = events.last().map(|event| event.occurred_at.clone());
        let agent_id = events.iter().find_map(|event| event.agent_id.clone());
        let agent_name = events
            .iter()
            .find_map(|event| event.event.agent.name.clone());
        let mut outcomes = events
            .iter()
            .filter_map(|event| event.outcome.clone())
            .collect::<Vec<_>>();
        outcomes.sort();
        outcomes.dedup();
        filter.limit = Some(0);
        Ok(RunRecord {
            run_id: run_id.to_string(),
            first_occurred_at: first,
            last_occurred_at: last,
            event_count: events.len(),
            agent_id,
            agent_name,
            outcomes,
            events,
            artifacts,
        })
    }

    pub fn backfill_vacation_guard_history(
        &self,
        history_dir: &Path,
        limit: usize,
    ) -> OpenFangResult<usize> {
        if !history_dir.exists() {
            return Ok(0);
        }

        let mut entries = std::fs::read_dir(history_dir)
            .map_err(|e| OpenFangError::Memory(e.to_string()))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        if entries.len() > limit {
            entries = entries.split_off(entries.len() - limit);
        }

        let mut inserted = 0usize;
        for entry in entries {
            let path = entry.path();
            let raw = match std::fs::read_to_string(&path) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            let parsed: Value = match serde_json::from_str(&raw) {
                Ok(parsed) => parsed,
                Err(_) => continue,
            };
            let Some(mut event) = guard_artifact_to_event(&path, &parsed) else {
                continue;
            };
            if self.exists(&event.event.id)? {
                continue;
            }
            if event.occurred_at.is_empty() {
                event.occurred_at = Utc::now().to_rfc3339();
            }
            self.record(&event)?;
            inserted += 1;
        }

        Ok(inserted)
    }
}

impl ArtifactIndexStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    pub fn upsert(&self, record: &ArtifactRecord) -> OpenFangResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| OpenFangError::Internal(e.to_string()))?;
        let json = serde_json::to_string(&record.metadata_json)
            .map_err(|e| OpenFangError::Serialization(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO artifact_index (
                artifact_id, run_id, session_id, agent_id, artifact_kind, storage_path,
                content_type, created_at, metadata_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.artifact_id,
                record.run_id,
                record.session_id,
                record.agent_id,
                record.artifact_kind,
                record.storage_path,
                record.content_type,
                record.created_at,
                json,
            ],
        )
        .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        Ok(())
    }

    pub fn list_for_run(&self, run_id: &str) -> OpenFangResult<Vec<ArtifactRecord>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| OpenFangError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT artifact_id, run_id, session_id, agent_id, artifact_kind, storage_path,
                        content_type, created_at, metadata_json
                 FROM artifact_index
                 WHERE run_id = ?1
                 ORDER BY created_at ASC, artifact_id ASC",
            )
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                let metadata_json: String = row.get(8)?;
                let metadata_json = serde_json::from_str(&metadata_json).unwrap_or(Value::Null);
                Ok(ArtifactRecord {
                    artifact_id: row.get(0)?,
                    run_id: row.get(1)?,
                    session_id: row.get(2)?,
                    agent_id: row.get(3)?,
                    artifact_kind: row.get(4)?,
                    storage_path: row.get(5)?,
                    content_type: row.get(6)?,
                    created_at: row.get(7)?,
                    metadata_json,
                })
            })
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| OpenFangError::Memory(e.to_string()))?);
        }
        Ok(out)
    }
}

fn decode_cursor(cursor: &str) -> Option<(String, String)> {
    let (occurred_at, event_id) = cursor.split_once('|')?;
    Some((occurred_at.to_string(), event_id.to_string()))
}

fn guard_artifact_to_event(path: &Path, parsed: &Value) -> Option<CanonicalEvent> {
    let guard_body = parsed
        .get("sentry")
        .and_then(|sentry| sentry.get("guard_report_body"))
        .cloned()
        .unwrap_or(Value::Null);
    let event_kind = guard_body
        .get("event.kind")
        .and_then(Value::as_str)
        .or_else(|| guard_body.get("kind").and_then(Value::as_str))?;
    let file_stem = path.file_stem()?.to_string_lossy();
    let request_id = parsed
        .get("local")
        .and_then(|local| local.get("heartbeat_request_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let trace_id = guard_body
        .get("trace_id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let run_id = trace_id.clone().or_else(|| request_id.clone());
    let outcome = parsed
        .get("status")
        .and_then(Value::as_str)
        .map(|status| if status == "pass" { "success" } else { "error" }.to_string());
    let mut payload = parsed.clone();
    if let Some(map) = payload.as_object_mut() {
        map.insert(
            "_backfill".to_string(),
            Value::String("vacation_guard_history".to_string()),
        );
    }
    Some(CanonicalEvent {
        schema_version: openfang_types::facts::CANONICAL_EVENT_SCHEMA_VERSION,
        event: openfang_types::facts::CanonicalEventId {
            id: format!("backfill-{file_stem}-{event_kind}"),
            kind: event_kind.to_string(),
        },
        occurred_at: parsed
            .get("generated_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        trace: openfang_types::facts::CanonicalRef { id: trace_id },
        request: openfang_types::facts::CanonicalRef { id: request_id },
        run: openfang_types::facts::CanonicalRef { id: run_id },
        session: openfang_types::facts::CanonicalRef::default(),
        agent: openfang_types::facts::CanonicalAgentRef {
            id: None,
            name: Some("vacation-guard".to_string()),
        },
        channel: openfang_types::facts::CanonicalChannelRef {
            kind: Some("ops".to_string()),
            user_id: None,
        },
        artifact: openfang_types::facts::CanonicalArtifactRefs {
            ids: vec![format!("vacation-guard:{file_stem}")],
        },
        outcome,
        duration_ms: None,
        cost: openfang_types::facts::CanonicalCost::default(),
        model: openfang_types::facts::CanonicalModelRef::default(),
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_event() -> CanonicalEvent {
        CanonicalEvent {
            schema_version: 1,
            event: openfang_types::facts::CanonicalEventId {
                id: "evt-1".to_string(),
                kind: "runtime.agent_loop.completed".to_string(),
            },
            occurred_at: "2026-03-17T00:00:00Z".to_string(),
            run: openfang_types::facts::CanonicalRef {
                id: Some("run-1".to_string()),
            },
            session: openfang_types::facts::CanonicalRef {
                id: Some("session-1".to_string()),
            },
            request: openfang_types::facts::CanonicalRef {
                id: Some("request-1".to_string()),
            },
            trace: openfang_types::facts::CanonicalRef {
                id: Some("trace-1".to_string()),
            },
            agent: openfang_types::facts::CanonicalAgentRef {
                id: Some("agent-1".to_string()),
                name: Some("tester".to_string()),
            },
            channel: openfang_types::facts::CanonicalChannelRef {
                kind: Some("http".to_string()),
                user_id: Some("user-1".to_string()),
            },
            artifact: openfang_types::facts::CanonicalArtifactRefs::default(),
            outcome: Some("success".to_string()),
            duration_ms: Some(42),
            cost: openfang_types::facts::CanonicalCost { usd: Some(0.12) },
            model: openfang_types::facts::CanonicalModelRef {
                provider: Some("groq".to_string()),
                name: Some("llama".to_string()),
            },
            payload: serde_json::json!({"message": "ok"}),
        }
    }

    #[test]
    fn test_query_and_run_lookup() {
        let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        crate::migration::run_migrations(&conn.lock().unwrap()).unwrap();
        let store = FactEventStore::new(conn.clone());
        store.record(&sample_event()).unwrap();
        ArtifactIndexStore::new(conn.clone())
            .upsert(&ArtifactRecord {
                artifact_id: "artifact-1".to_string(),
                run_id: Some("run-1".to_string()),
                session_id: Some("session-1".to_string()),
                agent_id: Some("agent-1".to_string()),
                artifact_kind: "upload".to_string(),
                storage_path: "/tmp/a".to_string(),
                content_type: Some("text/plain".to_string()),
                created_at: "2026-03-17T00:00:01Z".to_string(),
                metadata_json: serde_json::json!({"size": 4}),
            })
            .unwrap();

        let rows = store
            .query(
                &FactEventFilter {
                    run_id: Some("run-1".to_string()),
                    limit: Some(10),
                    ..Default::default()
                },
                true,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event.event.kind, "runtime.agent_loop.completed");

        let run = store.get_run("run-1").unwrap();
        assert_eq!(run.event_count, 1);
        assert_eq!(run.artifacts.len(), 1);
    }

    #[test]
    fn test_backfill_guard_history() {
        let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        crate::migration::run_migrations(&conn.lock().unwrap()).unwrap();
        let store = FactEventStore::new(conn);
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("20260316T042501Z.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "generated_at": "2026-03-16T04:25:01.483540+00:00",
                "status": "pass",
                "local": {"heartbeat_request_id": "request-1"},
                "sentry": {"guard_report_body": {
                    "event.kind": "ops.guard.heartbeat",
                    "trace_id": "trace-1"
                }}
            })
            .to_string(),
        )
        .unwrap();

        let inserted = store
            .backfill_vacation_guard_history(tmp.path(), 10)
            .unwrap();
        assert_eq!(inserted, 1);
        assert!(store.has_event_kind_prefix("ops.guard").unwrap());
    }
}
