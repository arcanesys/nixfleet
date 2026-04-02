use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::db::Db;

/// Hash a raw API key to its stored form.
pub fn hash_key(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

/// Actor identity extracted from authentication.
#[derive(Debug, Clone)]
pub enum Actor {
    ApiKey { name: String, role: String },
    Machine { machine_id: String },
}

impl Actor {
    pub fn identifier(&self) -> String {
        match self {
            Actor::ApiKey { name, .. } => format!("apikey:{name}"),
            Actor::Machine { machine_id } => format!("machine:{machine_id}"),
        }
    }

    /// Check whether the actor has one of the allowed roles.
    /// Machines do not have roles and always return false.
    pub fn has_role(&self, allowed: &[&str]) -> bool {
        match self {
            Actor::ApiKey { role, .. } => allowed.contains(&role.as_str()),
            Actor::Machine { .. } => false,
        }
    }
}

/// Middleware: require valid API key in Authorization: Bearer header.
/// If an Actor is already set (e.g. from mTLS), pass through.
pub async fn require_api_key(
    headers: HeaderMap,
    db: Arc<Db>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if request.extensions().get::<Actor>().is_some() {
        return Ok(next.run(request).await);
    }

    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let key_hash = hash_key(token);
    let role = db
        .verify_api_key(&key_hash)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let name = db
        .get_api_key_name(&key_hash)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .unwrap_or_else(|| "unknown".to_string());

    request
        .extensions_mut()
        .insert(Actor::ApiKey { name, role });
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_key_deterministic() {
        assert_eq!(hash_key("my-secret"), hash_key("my-secret"));
    }

    #[test]
    fn test_hash_key_different_inputs() {
        assert_ne!(hash_key("key-a"), hash_key("key-b"));
    }

    #[test]
    fn test_actor_identifier_apikey() {
        let actor = Actor::ApiKey {
            name: "deploy-key".into(),
            role: "deploy".into(),
        };
        assert_eq!(actor.identifier(), "apikey:deploy-key");
    }

    #[test]
    fn test_actor_identifier_machine() {
        let actor = Actor::Machine {
            machine_id: "web-01".into(),
        };
        assert_eq!(actor.identifier(), "machine:web-01");
    }
}
