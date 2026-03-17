use super::computer::{parse_agent_id, NitroComputerManager};
use openfang_types::nitro::NitroEvent;

impl NitroComputerManager {
    /// Recovery path: mark all known computers as recovering then ready.
    ///
    /// Nitro bindings are persisted in SQLite and become authoritative at boot,
    /// so recovery is deterministic by reading durable tables.
    pub fn recover_all(&self) -> Result<usize, String> {
        let agent_ids: Vec<String> = {
            let conn = self.lock_conn();
            let mut stmt = conn
                .prepare("SELECT agent_id FROM agent_computers")
                .map_err(|e| format!("Failed to prepare recover_all query: {e}"))?;
            let rows = stmt
                .query_map([], |row: &rusqlite::Row<'_>| row.get::<_, String>(0))
                .map_err(|e| format!("Failed to iterate agent_computers rows: {e}"))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| format!("Failed to decode agent_computers row: {e}"))?);
            }
            out
        };

        for agent_id_str in &agent_ids {
            let aid = parse_agent_id(agent_id_str);
            {
                let conn = self.lock_conn();
                conn.execute(
                    "UPDATE agent_computers SET status = 'recovering', updated_at = ?2 WHERE agent_id = ?1",
                    rusqlite::params![agent_id_str, Self::now_rfc3339()],
                )
                .map_err(|e| format!("Failed to mark computer recovering: {e}"))?;
            }
            let _ = self.record_event(NitroEvent::ComputerRecovered { agent_id: aid });
            {
                let conn = self.lock_conn();
                conn.execute(
                    "UPDATE agent_computers SET status = 'ready', updated_at = ?2 WHERE agent_id = ?1",
                    rusqlite::params![agent_id_str, Self::now_rfc3339()],
                )
                .map_err(|e| format!("Failed to mark computer ready after recovery: {e}"))?;
            }
            {
                let conn = self.lock_conn();
                let _ = conn.execute(
                    "UPDATE agent_computers_v2
                     SET status = 'recovering', updated_at = ?2
                     WHERE agent_id = ?1",
                    rusqlite::params![agent_id_str, Self::now_rfc3339()],
                );
                let _ = conn.execute(
                    "UPDATE agent_computers_v2
                     SET status = 'ready', updated_at = ?2
                     WHERE agent_id = ?1",
                    rusqlite::params![agent_id_str, Self::now_rfc3339()],
                );
            }
        }

        Ok(agent_ids.len())
    }
}
