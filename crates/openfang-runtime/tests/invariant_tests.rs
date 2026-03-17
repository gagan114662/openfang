use openfang_runtime::drivers::known_providers;
use openfang_runtime::model_catalog::ModelCatalog;
use std::collections::HashSet;

#[test]
fn test_catalog_providers_match_known() {
    let catalog = ModelCatalog::new();
    let known: HashSet<&str> = known_providers().iter().copied().collect();
    let cli_only: HashSet<&str> = ["codex-cli", "claude-code"].into_iter().collect();

    let catalog_ids: HashSet<String> = catalog
        .list_providers()
        .iter()
        .map(|p| p.id.clone())
        .collect();

    for id in &catalog_ids {
        if cli_only.contains(id.as_str()) {
            continue;
        }
        assert!(
            known.contains(id.as_str()),
            "Provider '{}' is in builtin_providers() (model catalog) but missing from known_providers()",
            id
        );
    }

    for name in &known {
        if cli_only.contains(*name) {
            continue;
        }
        assert!(
            catalog_ids.contains(*name),
            "Provider '{}' is in known_providers() but missing from builtin_providers() (model catalog)",
            name
        );
    }
}
