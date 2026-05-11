use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HttpAuthConfig {
    pub bearer_token: String,
}

impl HttpAuthConfig {
    pub fn validate(&self, scope: &str) -> anyhow::Result<()> {
        let prefix = if scope.is_empty() {
            String::new()
        } else {
            format!("{scope} ")
        };
        anyhow::ensure!(
            !self.bearer_token.is_empty(),
            "{prefix}http_auth.bearer_token must not be empty"
        );
        anyhow::ensure!(
            !self.bearer_token.chars().any(char::is_whitespace),
            "{prefix}http_auth.bearer_token must not contain whitespace"
        );
        Ok(())
    }

    pub fn authorization_header_value(&self) -> String {
        format!("Bearer {}", self.bearer_token)
    }
}

#[cfg(test)]
mod tests {
    use super::HttpAuthConfig;

    #[test]
    fn authorization_header_value_uses_bearer_scheme() {
        let config = HttpAuthConfig {
            bearer_token: "shared-secret".to_string(),
        };
        assert_eq!(config.authorization_header_value(), "Bearer shared-secret");
    }

    #[test]
    fn validate_can_emit_scoped_and_unscoped_messages() {
        let config = HttpAuthConfig {
            bearer_token: String::new(),
        };
        assert_eq!(
            config.validate("").unwrap_err().to_string(),
            "http_auth.bearer_token must not be empty"
        );
        assert_eq!(
            config.validate("target `builder`").unwrap_err().to_string(),
            "target `builder` http_auth.bearer_token must not be empty"
        );
    }
}
