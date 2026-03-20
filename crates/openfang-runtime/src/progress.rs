//! Structured PROGRESS.md parser and orientation summary.
//!
//! Parses GFM task lists into a `ProgressSnapshot` that agents can use
//! to orient across context window boundaries.

/// Status of a single progress item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStatus {
    Done,
    Pending,
    Blocked,
}

/// A single parsed progress item from a GFM task list.
#[derive(Debug, Clone)]
pub struct ProgressItem {
    /// The item label text.
    pub label: String,
    /// Current status.
    pub status: ProgressStatus,
    /// Optional phase heading the item belongs to.
    pub phase: Option<String>,
}

/// A snapshot of all progress items parsed from PROGRESS.md.
#[derive(Debug, Clone, Default)]
pub struct ProgressSnapshot {
    pub items: Vec<ProgressItem>,
}

impl ProgressSnapshot {
    /// Parse a PROGRESS.md markdown string into a snapshot.
    pub fn parse(markdown: &str) -> Self {
        let mut items = Vec::new();
        let mut current_phase: Option<String> = None;

        for line in markdown.lines() {
            let trimmed = line.trim();

            // Detect phase headings (## Phase X or # Phase X)
            if let Some(heading) = trimmed.strip_prefix("## ") {
                current_phase = Some(heading.trim().to_string());
                continue;
            }
            if let Some(heading) = trimmed.strip_prefix("# ") {
                current_phase = Some(heading.trim().to_string());
                continue;
            }

            // Parse GFM task list items
            let content = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "));
            let Some(content) = content else {
                continue;
            };

            if let Some(label) = content
                .strip_prefix("[x] ")
                .or_else(|| content.strip_prefix("[X] "))
            {
                items.push(ProgressItem {
                    label: label.trim().to_string(),
                    status: ProgressStatus::Done,
                    phase: current_phase.clone(),
                });
            } else if let Some(label) = content.strip_prefix("[ ] ") {
                // Check for blocked marker
                let label_trimmed = label.trim();
                if label_trimmed.starts_with("[BLOCKED]")
                    || label_trimmed.starts_with("(BLOCKED)")
                    || label_trimmed.contains("🚫")
                {
                    items.push(ProgressItem {
                        label: label_trimmed.to_string(),
                        status: ProgressStatus::Blocked,
                        phase: current_phase.clone(),
                    });
                } else {
                    items.push(ProgressItem {
                        label: label_trimmed.to_string(),
                        status: ProgressStatus::Pending,
                        phase: current_phase.clone(),
                    });
                }
            }
        }

        Self { items }
    }

    /// Count of done items.
    pub fn done_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == ProgressStatus::Done)
            .count()
    }

    /// Count of pending items.
    pub fn pending_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == ProgressStatus::Pending)
            .count()
    }

    /// Count of blocked items.
    pub fn blocked_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == ProgressStatus::Blocked)
            .count()
    }

    /// First pending item (the next task to work on).
    pub fn next_pending(&self) -> Option<&ProgressItem> {
        self.items
            .iter()
            .find(|i| i.status == ProgressStatus::Pending)
    }

    /// Build a one-line orientation summary for injection into the system prompt.
    pub fn orientation_summary(&self) -> String {
        let total = self.items.len();
        if total == 0 {
            return String::new();
        }

        let done = self.done_count();
        let pending = self.pending_count();
        let blocked = self.blocked_count();

        let mut parts = vec![format!(
            "Progress: {done}/{total} done, {pending} pending, {blocked} blocked."
        )];

        if let Some(next) = self.next_pending() {
            parts.push(format!("Next task: {}.", next.label));
            if let Some(ref phase) = next.phase {
                parts.push(format!("Phase: {phase}"));
            }
        }

        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MD: &str = "\
# Project Progress

## Phase 1
- [x] Set up project
- [x] Add database
- [ ] Add tests
- [x] Write docs

## Phase 2
- [ ] Deploy to staging
- [x] Code review
- [ ] [BLOCKED] Waiting on API key

## Phase 3
- [ ] Launch
";

    #[test]
    fn test_parse_mixed_task_list() {
        let snap = ProgressSnapshot::parse(SAMPLE_MD);
        assert_eq!(snap.items.len(), 8);
        assert_eq!(snap.done_count(), 4);
        assert_eq!(snap.pending_count(), 3);
        assert_eq!(snap.blocked_count(), 1);
    }

    #[test]
    fn test_next_pending_returns_first_pending() {
        let snap = ProgressSnapshot::parse(SAMPLE_MD);
        let next = snap.next_pending().unwrap();
        assert_eq!(next.label, "Add tests");
        assert_eq!(next.phase.as_deref(), Some("Phase 1"));
    }

    #[test]
    fn test_orientation_summary_format() {
        let snap = ProgressSnapshot::parse(SAMPLE_MD);
        let summary = snap.orientation_summary();
        assert!(summary.contains("Progress: 4/8 done"));
        assert!(summary.contains("3 pending"));
        assert!(summary.contains("1 blocked"));
        assert!(summary.contains("Next task: Add tests."));
        assert!(summary.contains("Phase: Phase 1"));
    }

    #[test]
    fn test_empty_markdown_returns_empty_snapshot() {
        let snap = ProgressSnapshot::parse("");
        assert!(snap.items.is_empty());
        assert_eq!(snap.done_count(), 0);
        assert_eq!(snap.pending_count(), 0);
        assert_eq!(snap.blocked_count(), 0);
        assert!(snap.orientation_summary().is_empty());
    }
}
