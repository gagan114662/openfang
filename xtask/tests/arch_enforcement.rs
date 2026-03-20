//! Architecture layer enforcement tests.
//!
//! Validates that crate dependencies flow downward per the documented
//! layer hierarchy. Catches dependency violations at build time rather
//! than relying on human code review.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

/// A structured layer violation with agent-targeted remediation steps.
#[derive(Debug)]
struct LayerViolation {
    crate_name: String,
    crate_layer: u8,
    dep_name: String,
    dep_layer: u8,
    rule_text: String,
    remediation_steps: Vec<String>,
}

impl fmt::Display for LayerViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "VIOLATION: {} (L{}) → {} (L{})",
            self.crate_name, self.crate_layer, self.dep_name, self.dep_layer
        )?;
        writeln!(f, "RULE: {}", self.rule_text)?;
        writeln!(f, "FIX:")?;
        for (i, step) in self.remediation_steps.iter().enumerate() {
            writeln!(f, "  {}. {step}", i + 1)?;
        }
        Ok(())
    }
}

/// Parse openfang-* dependencies from a Cargo.toml file.
fn parse_openfang_deps(cargo_toml_path: &Path) -> Vec<String> {
    let content = std::fs::read_to_string(cargo_toml_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", cargo_toml_path.display()));

    let parsed: toml::Value = content
        .parse()
        .unwrap_or_else(|e| panic!("Failed to parse {}: {e}", cargo_toml_path.display()));

    let mut deps = Vec::new();

    for section in ["dependencies", "dev-dependencies"] {
        if let Some(dep_table) = parsed.get(section).and_then(|v| v.as_table()) {
            for key in dep_table.keys() {
                if key.starts_with("openfang-") {
                    deps.push(key.clone());
                }
            }
        }
    }

    deps
}

/// Build the layer map from the documented architecture.
fn layer_map() -> HashMap<&'static str, u8> {
    let mut m = HashMap::new();
    // Layer 0: types only
    m.insert("openfang-types", 0);
    // Layer 1: memory
    m.insert("openfang-memory", 1);
    // Layer 2: runtime and peer modules
    m.insert("openfang-runtime", 2);
    m.insert("openfang-wire", 2);
    m.insert("openfang-channels", 2);
    m.insert("openfang-skills", 2);
    m.insert("openfang-hands", 2);
    m.insert("openfang-extensions", 2);
    m.insert("openfang-migrate", 2);
    // Layer 3: kernel
    m.insert("openfang-kernel", 3);
    // Layer 4: API
    m.insert("openfang-api", 4);
    // Layer 5: frontends
    m.insert("openfang-cli", 5);
    m.insert("openfang-desktop", 5);
    m.insert("openfang-telegram", 5);
    m
}

/// Find the workspace root (where root Cargo.toml lives).
fn workspace_root() -> std::path::PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // xtask is at <workspace>/xtask, so parent is workspace root
    manifest_dir
        .parent()
        .expect("xtask must be inside workspace")
        .to_path_buf()
}

#[test]
fn test_no_upward_dependencies() {
    let root = workspace_root();
    let layers = layer_map();
    let mut violations = Vec::new();

    for (&crate_name, &crate_layer) in &layers {
        let cargo_path = root.join("crates").join(crate_name).join("Cargo.toml");
        if !cargo_path.exists() {
            continue;
        }

        let deps = parse_openfang_deps(&cargo_path);
        for dep in &deps {
            if let Some(&dep_layer) = layers.get(dep.as_str()) {
                if dep_layer > crate_layer {
                    violations.push(LayerViolation {
                        crate_name: crate_name.to_string(),
                        crate_layer,
                        dep_name: dep.clone(),
                        dep_layer,
                        rule_text: "Crates may only depend on same or lower layers".to_string(),
                        remediation_steps: vec![
                            format!("Remove {dep} from {crate_name}/Cargo.toml [dependencies]"),
                            format!(
                                "If you need {dep} functionality, extract it to a shared L{} crate",
                                crate_layer.min(dep_layer)
                            ),
                            "Re-run: cargo xtask check-layers".to_string(),
                        ],
                    });
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Architecture layer violations found:\n{}",
        violations
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn test_no_circular_dependencies_between_runtime_and_kernel() {
    let root = workspace_root();
    let runtime_cargo = root
        .join("crates")
        .join("openfang-runtime")
        .join("Cargo.toml");

    let deps = parse_openfang_deps(&runtime_cargo);
    assert!(
        !deps.contains(&"openfang-kernel".to_string()),
        "openfang-runtime must NOT depend on openfang-kernel. \
         Use the KernelHandle trait instead to avoid circular dependencies."
    );
}

#[test]
fn test_violation_format_includes_remediation() {
    let v = LayerViolation {
        crate_name: "openfang-api".to_string(),
        crate_layer: 4,
        dep_name: "openfang-cli".to_string(),
        dep_layer: 5,
        rule_text: "Crates may only depend on same or lower layers".to_string(),
        remediation_steps: vec![
            "Remove openfang-cli from openfang-api/Cargo.toml [dependencies]".to_string(),
            "If you need CLI functionality, extract it to a shared L4 crate".to_string(),
            "Re-run: cargo xtask check-layers".to_string(),
        ],
    };
    let formatted = v.to_string();
    assert!(formatted.contains("VIOLATION: openfang-api (L4)"));
    assert!(formatted.contains("openfang-cli (L5)"));
    assert!(formatted.contains("RULE:"));
    assert!(formatted.contains("FIX:"));
    assert!(formatted.contains("1. Remove openfang-cli"));
    assert!(formatted.contains("2. If you need CLI"));
    assert!(formatted.contains("3. Re-run: cargo xtask check-layers"));
}
