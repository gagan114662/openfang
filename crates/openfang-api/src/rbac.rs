//! Role-based access control for API endpoints.

use axum::http::Method;
use openfang_types::config::{ApiRole, ApiTokenConfig};

/// Extension methods for ApiRole in the API layer.
pub trait ApiRoleExt {
    fn allows_method(&self, method: &Method) -> bool;
}

impl ApiRoleExt for ApiRole {
    /// Check whether this role permits the given HTTP method.
    fn allows_method(&self, method: &Method) -> bool {
        match self {
            ApiRole::Viewer => {
                method == Method::GET || method == Method::HEAD || method == Method::OPTIONS
            }
            ApiRole::Operator => method != Method::DELETE,
            ApiRole::Admin => true,
        }
    }
}

/// Shared auth state passed to the middleware layer.
#[derive(Clone)]
pub struct AuthConfig {
    /// Legacy single API key (always grants Admin).
    pub api_key: String,
    /// Additional tokens with per-token roles.
    pub tokens: Vec<ApiTokenConfig>,
}

impl AuthConfig {
    /// Look up the role for a given bearer token.
    ///
    /// Returns `Some(role)` if the token matches the legacy `api_key` or any
    /// entry in `tokens`. Returns `None` if no match found.
    pub fn resolve_role(&self, token: &str) -> Option<ApiRole> {
        use subtle::ConstantTimeEq;

        // Check legacy api_key first (always admin).
        if !self.api_key.is_empty()
            && token.len() == self.api_key.len()
            && bool::from(token.as_bytes().ct_eq(self.api_key.as_bytes()))
        {
            return Some(ApiRole::Admin);
        }

        // Check named tokens.
        for entry in &self.tokens {
            if token.len() == entry.token.len()
                && bool::from(token.as_bytes().ct_eq(entry.token.as_bytes()))
            {
                return Some(entry.role);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> AuthConfig {
        AuthConfig {
            api_key: "admin-secret".to_string(),
            tokens: vec![
                ApiTokenConfig {
                    token: "viewer-token".to_string(),
                    role: ApiRole::Viewer,
                },
                ApiTokenConfig {
                    token: "operator-token".to_string(),
                    role: ApiRole::Operator,
                },
                ApiTokenConfig {
                    token: "admin-token".to_string(),
                    role: ApiRole::Admin,
                },
            ],
        }
    }

    #[test]
    fn test_viewer_get_allowed() {
        assert!(ApiRole::Viewer.allows_method(&Method::GET));
        assert!(ApiRole::Viewer.allows_method(&Method::HEAD));
        assert!(ApiRole::Viewer.allows_method(&Method::OPTIONS));
    }

    #[test]
    fn test_viewer_post_forbidden() {
        assert!(!ApiRole::Viewer.allows_method(&Method::POST));
        assert!(!ApiRole::Viewer.allows_method(&Method::PUT));
        assert!(!ApiRole::Viewer.allows_method(&Method::DELETE));
    }

    #[test]
    fn test_operator_post_allowed() {
        assert!(ApiRole::Operator.allows_method(&Method::GET));
        assert!(ApiRole::Operator.allows_method(&Method::POST));
        assert!(ApiRole::Operator.allows_method(&Method::PUT));
        assert!(ApiRole::Operator.allows_method(&Method::PATCH));
    }

    #[test]
    fn test_operator_delete_forbidden() {
        assert!(!ApiRole::Operator.allows_method(&Method::DELETE));
    }

    #[test]
    fn test_admin_delete_allowed() {
        assert!(ApiRole::Admin.allows_method(&Method::DELETE));
        assert!(ApiRole::Admin.allows_method(&Method::POST));
        assert!(ApiRole::Admin.allows_method(&Method::GET));
    }

    #[test]
    fn test_legacy_token_is_admin() {
        let config = test_config();
        assert_eq!(config.resolve_role("admin-secret"), Some(ApiRole::Admin));
    }

    #[test]
    fn test_named_tokens_resolve() {
        let config = test_config();
        assert_eq!(config.resolve_role("viewer-token"), Some(ApiRole::Viewer));
        assert_eq!(
            config.resolve_role("operator-token"),
            Some(ApiRole::Operator)
        );
        assert_eq!(config.resolve_role("admin-token"), Some(ApiRole::Admin));
    }

    #[test]
    fn test_unknown_token_rejected() {
        let config = test_config();
        assert_eq!(config.resolve_role("wrong-token"), None);
    }

    #[test]
    fn test_empty_api_key_no_legacy_match() {
        let config = AuthConfig {
            api_key: String::new(),
            tokens: vec![ApiTokenConfig {
                token: "only-token".to_string(),
                role: ApiRole::Viewer,
            }],
        };
        assert_eq!(config.resolve_role("only-token"), Some(ApiRole::Viewer));
        assert_eq!(config.resolve_role(""), None);
    }
}
