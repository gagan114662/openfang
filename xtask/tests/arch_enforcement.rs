//! Architecture layer enforcement tests.
//!
//! Validates that crate dependencies flow downward per the documented
//! layer hierarchy. Catches dependency violations at build time rather
//! than relying on human code review.

use std::collections::HashMap;
use std::path::Path;

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
                    violations.push(format!(
                        "{crate_name} (layer {crate_layer}) depends on {dep} (layer {dep_layer})"
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Architecture layer violations found:\n{}",
        violations.join("\n")
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
