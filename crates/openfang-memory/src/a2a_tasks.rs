//! SQLite-backed A2A task persistence.
//!
//! Stores A2A task lifecycle data so tasks survive daemon restarts.

use crate::DbPool;
use openfang_types::error::{OpenFangError, OpenFangResult};

/// A single A2A task row as stored in SQLite.
#[derive(Debug, Clone)]
pub struct A2aTaskRow {
    pub id: String,
    pub agent_url: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub request_json: String,
    pub response_json: String,
}

/// SQLite-backed store for A2A task persistence.
#[derive(Clone)]
pub struct A2aTaskSqlStore {
    pool: DbPool,
}

impl A2aTaskSqlStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub fn insert(&self, row: &A2aTaskRow) -> OpenFangResult<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        conn.execute(
            "INSERT INTO a2a_tasks (id, agent_url, status, created_at, updated_at, request_json, response_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 agent_url = excluded.agent_url,
                 status = excluded.status,
                 updated_at = excluded.updated_at,
                 request_json = excluded.request_json,
                 response_json = excluded.response_json",
            rusqlite::params![
                row.id,
                row.agent_url,
                row.status,
                row.created_at,
                row.updated_at,
                row.request_json,
                row.response_json,
            ],
        )
        .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> OpenFangResult<Option<A2aTaskRow>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_url, status, created_at, updated_at, request_json, response_json
                 FROM a2a_tasks WHERE id = ?1",
            )
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let result = stmt.query_row(rusqlite::params![id], |row| {
            Ok(A2aTaskRow {
                id: row.get(0)?,
                agent_url: row.get(1)?,
                status: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                request_json: row.get(5)?,
                response_json: row.get(6)?,
            })
        });
        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(OpenFangError::Memory(e.to_string())),
        }
    }

    pub fn update_status(&self, id: &str, status: &str) -> OpenFangResult<bool> {
        let conn = self
            .pool
            .get()
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let now = chrono::Utc::now().to_rfc3339();
        let rows = conn
            .execute(
                "UPDATE a2a_tasks SET status = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, status, now],
            )
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        Ok(rows > 0)
    }

    pub fn update_response(
        &self,
        id: &str,
        status: &str,
        response_json: &str,
    ) -> OpenFangResult<bool> {
        let conn = self
            .pool
            .get()
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let now = chrono::Utc::now().to_rfc3339();
        let rows = conn
            .execute(
                "UPDATE a2a_tasks SET status = ?2, response_json = ?3, updated_at = ?4 WHERE id = ?1",
                rusqlite::params![id, status, response_json, now],
            )
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        Ok(rows > 0)
    }

    pub fn delete(&self, id: &str) -> OpenFangResult<bool> {
        let conn = self
            .pool
            .get()
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let rows = conn
            .execute(
                "DELETE FROM a2a_tasks WHERE id = ?1",
                rusqlite::params![id],
            )
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        Ok(rows > 0)
    }

    pub fn list(&self) -> OpenFangResult<Vec<A2aTaskRow>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_url, status, created_at, updated_at, request_json, response_json
                 FROM a2a_tasks ORDER BY created_at DESC",
            )
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(A2aTaskRow {
                    id: row.get(0)?,
                    agent_url: row.get(1)?,
                    status: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    request_json: row.get(5)?,
                    response_json: row.get(6)?,
                })
            })
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| OpenFangError::Memory(e.to_string()))?);
        }
        Ok(result)
    }

    pub fn count(&self) -> OpenFangResult<usize> {
        let conn = self
            .pool
            .get()
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM a2a_tasks", [], |row| row.get(0))
            .map_err(|e| OpenFangError::Memory(e.to_string()))?;
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn test_store() -> A2aTaskSqlStore {
        let manager = r2d2_sqlite::SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(manager).unwrap();
        {
            let conn = pool.get().unwrap();
            run_migrations(&conn).unwrap();
        }
        A2aTaskSqlStore::new(pool)
    }

    #[test]
    fn test_insert_and_get() {
        let store = test_store();
        let row = A2aTaskRow {
            id: "task-1".to_string(),
            agent_url: "https://example.com/a2a".to_string(),
            status: "working".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            request_json: r#"{"role":"user","parts":[{"type":"text","text":"hello"}]}"#.to_string(),
            response_json: "{}".to_string(),
        };
        store.insert(&row).unwrap();

        let got = store.get("task-1").unwrap().unwrap();
        assert_eq!(got.id, "task-1");
        assert_eq!(got.status, "working");
        assert_eq!(got.agent_url, "https://example.com/a2a");
    }

    #[test]
    fn test_get_nonexistent() {
        let store = test_store();
        assert!(store.get("nope").unwrap().is_none());
    }

    #[test]
    fn test_update_status() {
        let store = test_store();
        let row = A2aTaskRow {
            id: "task-2".to_string(),
            agent_url: String::new(),
            status: "submitted".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            request_json: "{}".to_string(),
            response_json: "{}".to_string(),
        };
        store.insert(&row).unwrap();

        assert!(store.update_status("task-2", "completed").unwrap());
        let got = store.get("task-2").unwrap().unwrap();
        assert_eq!(got.status, "completed");
        assert_ne!(got.updated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn test_update_response() {
        let store = test_store();
        let row = A2aTaskRow {
            id: "task-3".to_string(),
            agent_url: String::new(),
            status: "working".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            request_json: "{}".to_string(),
            response_json: "{}".to_string(),
        };
        store.insert(&row).unwrap();

        let resp = r#"{"role":"agent","parts":[{"type":"text","text":"done"}]}"#;
        assert!(store.update_response("task-3", "completed", resp).unwrap());
        let got = store.get("task-3").unwrap().unwrap();
        assert_eq!(got.status, "completed");
        assert!(got.response_json.contains("done"));
    }

    #[test]
    fn test_delete() {
        let store = test_store();
        let row = A2aTaskRow {
            id: "task-4".to_string(),
            agent_url: String::new(),
            status: "working".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            request_json: "{}".to_string(),
            response_json: "{}".to_string(),
        };
        store.insert(&row).unwrap();
        assert!(store.delete("task-4").unwrap());
        assert!(store.get("task-4").unwrap().is_none());
        assert!(!store.delete("task-4").unwrap());
    }

    #[test]
    fn test_list_and_count() {
        let store = test_store();
        assert_eq!(store.count().unwrap(), 0);
        for i in 0..3 {
            let row = A2aTaskRow {
                id: format!("task-{i}"),
                agent_url: String::new(),
                status: "working".to_string(),
                created_at: format!("2026-01-0{i}T00:00:00Z"),
                updated_at: format!("2026-01-0{i}T00:00:00Z"),
                request_json: "{}".to_string(),
                response_json: "{}".to_string(),
            };
            store.insert(&row).unwrap();
        }
        assert_eq!(store.count().unwrap(), 3);
        let all = store.list().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_persistence_across_store_recreation() {
        let manager = r2d2_sqlite::SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(manager).unwrap();
        {
            let conn = pool.get().unwrap();
            run_migrations(&conn).unwrap();
        }

        let store1 = A2aTaskSqlStore::new(pool.clone());
        let row = A2aTaskRow {
            id: "persist-1".to_string(),
            agent_url: "https://example.com".to_string(),
            status: "completed".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:01Z".to_string(),
            request_json: r#"{"msg":"hello"}"#.to_string(),
            response_json: r#"{"msg":"world"}"#.to_string(),
        };
        store1.insert(&row).unwrap();
        drop(store1);

        let store2 = A2aTaskSqlStore::new(pool);
        let got = store2.get("persist-1").unwrap().unwrap();
        assert_eq!(got.id, "persist-1");
        assert_eq!(got.status, "completed");
        assert_eq!(got.agent_url, "https://example.com");
        assert!(got.request_json.contains("hello"));
        assert!(got.response_json.contains("world"));
    }
}
