//! Configuring endpoints, probing models, and assigning roles.
//!
//! The one piece of real logic here is key resolution: a provider row stores
//! the *name* of an environment variable, and the value is read at call time
//! and never persisted, logged, or returned. Everything else is orchestration
//! between [`ovis_core::db::llm`] and [`ovis_llm`].

use ovis_core::db::llm as db;
use ovis_llm::{handshake, Provider, ProviderKind};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::AppError;
use crate::state::AppState;

/// 503 with a specific message when the tables could not be created.
pub fn guard(state: &AppState) -> Result<(), AppError> {
    if state.llm_enabled {
        Ok(())
    } else {
        Err(AppError::NotAvailable(
            "LLM features are unavailable: the ovis.llm_* tables could not be created at \
             startup (the database user lacks CREATE on the ovis schema); see the startup log"
                .into(),
        ))
    }
}

/// Build a live provider from a stored row.
///
/// Reads the API key from the environment variable the row names. A missing
/// variable is an error that says which name was expected — the most common
/// setup mistake, and one a vague message makes miserable.
pub fn connect(row: &db::ProviderRow) -> Result<Provider, AppError> {
    let kind = ProviderKind::parse(&row.kind)?;
    let api_key = match &row.api_key_ref {
        Some(name) if !name.is_empty() => {
            let value = std::env::var(name).map_err(|_| {
                AppError::BadRequest(format!(
                    "provider '{}' expects its key in the environment variable {name}, which is \
                     not set on this process",
                    row.name
                ))
            })?;
            Some(value)
        }
        _ => None,
    };
    Provider::new(kind, row.base_url.as_deref(), api_key).map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ProviderView {
    #[serde(flatten)]
    pub row: db::ProviderRow,
    /// Whether the named environment variable is actually set. Surfacing this
    /// turns "why does nothing work" into a visible, fixable state.
    pub key_present: bool,
    pub models: i64,
    pub probed: i64,
}

pub async fn list_providers(state: &AppState) -> Result<Vec<ProviderView>, AppError> {
    guard(state)?;
    let rows = db::list_providers(&state.db).await?;
    let models = db::list_models(&state.db, None).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mine: Vec<_> = models.iter().filter(|m| m.provider_id == row.id).collect();
            ProviderView {
                key_present: row
                    .api_key_ref
                    .as_ref()
                    .map(|name| std::env::var(name).is_ok())
                    .unwrap_or(true),
                models: mine.len() as i64,
                probed: mine.iter().filter(|m| m.capabilities.is_some()).count() as i64,
                row,
            }
        })
        .collect())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProviderRequest {
    pub name: String,
    pub kind: String,
    pub base_url: Option<String>,
    /// The **name** of an environment variable holding the key. Never the key
    /// itself — the API refuses anything that looks like one.
    pub api_key_ref: Option<String>,
}

/// Add a provider and immediately enumerate its models.
///
/// Discovery runs inline so a misconfigured endpoint fails at the moment of
/// configuring it, with a message, rather than silently much later.
pub async fn create_provider(
    state: &AppState,
    request: CreateProviderRequest,
) -> Result<ProviderView, AppError> {
    guard(state)?;
    let kind = ProviderKind::parse(&request.kind)?;

    if let Some(reference) = &request.api_key_ref {
        reject_secret_looking_reference(reference)?;
    }
    if kind.requires_key() && request.api_key_ref.as_deref().unwrap_or("").is_empty() {
        return Err(AppError::BadRequest(format!(
            "provider kind '{}' needs a key: set api_key_ref to the name of an environment \
             variable holding it",
            kind.code()
        )));
    }

    let row = db::create_provider(
        &state.db,
        &request.name,
        kind.code(),
        request.base_url.as_deref(),
        request.api_key_ref.as_deref(),
    )
    .await?;

    // Fail loudly and clean up rather than leaving a provider that cannot be
    // reached — a half-configured endpoint is worse than none.
    if let Err(err) = discover(state, row.id).await {
        let _ = db::delete_provider(&state.db, row.id).await;
        return Err(err);
    }

    list_providers(state)
        .await?
        .into_iter()
        .find(|p| p.row.id == row.id)
        .ok_or_else(|| AppError::NotFound {
            what: "llm provider",
            id: row.id.to_string(),
        })
}

/// Refuse a value that looks like a secret rather than a variable name.
///
/// People paste keys into fields labelled "key". The field is a *reference*,
/// and silently storing a pasted secret would defeat the entire arrangement.
fn reject_secret_looking_reference(reference: &str) -> Result<(), AppError> {
    let looks_like_key = reference.len() > 60
        || reference.starts_with("sk-")
        || reference.starts_with("AIza")
        || reference.contains('.')
        || reference.contains('/');
    if looks_like_key {
        return Err(AppError::BadRequest(
            "api_key_ref is the NAME of an environment variable (for example \
             OVIS_GEMINI_API_KEY), not the key itself. Keys are read from the environment at \
             call time and never stored."
                .into(),
        ));
    }
    Ok(())
}

pub async fn delete_provider(state: &AppState, id: i64) -> Result<(), AppError> {
    guard(state)?;
    if !db::delete_provider(&state.db, id).await? {
        return Err(AppError::NotFound {
            what: "llm provider",
            id: id.to_string(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// Enumerate a provider's models and store them.
pub async fn discover(state: &AppState, provider_id: i64) -> Result<i64, AppError> {
    guard(state)?;
    let row = db::get_provider(&state.db, provider_id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "llm provider",
            id: provider_id.to_string(),
        })?;
    let provider = connect(&row)?;
    let models = provider.list_models().await?;

    let payload: Vec<(String, Option<String>, serde_json::Value)> = models
        .iter()
        .map(|m| {
            (
                m.id.clone(),
                m.display_name.clone(),
                serde_json::to_value(&m.advertised).unwrap_or(json!({})),
            )
        })
        .collect();
    db::replace_models(&state.db, provider_id, &payload).await?;
    Ok(models.len() as i64)
}

pub async fn list_models(
    state: &AppState,
    provider_id: Option<i64>,
) -> Result<Vec<db::ModelRow>, AppError> {
    guard(state)?;
    Ok(db::list_models(&state.db, provider_id).await?)
}

/// Run the capability handshake and store the findings.
pub async fn probe(
    state: &AppState,
    provider_id: i64,
    model_id: &str,
) -> Result<serde_json::Value, AppError> {
    guard(state)?;
    let row = db::get_provider(&state.db, provider_id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "llm provider",
            id: provider_id.to_string(),
        })?;
    let provider = connect(&row)?;

    let capabilities = handshake::probe(&provider, model_id).await?;
    let value = serde_json::to_value(&capabilities)
        .map_err(|e| AppError::BadRequest(format!("unserialisable capabilities: {e}")))?;
    // A no-op write here previously looked like success and only surfaced
    // much later as "this model has not been probed".
    let recorded = db::record_capabilities(
        &state.db,
        provider_id,
        model_id,
        &value,
        handshake::PROBE_VERSION,
    )
    .await?;
    if !recorded {
        return Err(AppError::NotFound {
            what: "llm model",
            id: format!("{provider_id}/{model_id}"),
        });
    }

    tracing::info!(
        provider = %row.name,
        model = model_id,
        findings = %capabilities.summary(),
        "probed model"
    );
    Ok(json!({
        "provider_id": provider_id,
        "model_id": model_id,
        "capabilities": value,
        "summary": capabilities.summary(),
        "usable_as_judge": capabilities.usable_as_judge(),
        "calibratable": capabilities.calibratable(),
    }))
}

/// Probe every unprobed model on a provider.
///
/// Sequential on purpose: a probe is four completions, and firing them at a
/// single-slot local server in parallel would queue behind each other anyway
/// while looking like a stall.
pub async fn probe_all(state: &AppState, provider_id: i64) -> Result<serde_json::Value, AppError> {
    guard(state)?;
    let models = db::list_models(&state.db, Some(provider_id)).await?;
    let mut probed = 0i64;
    let mut usable = 0i64;
    let mut skipped = 0i64;
    let mut failed = Vec::new();

    for model in models {
        // Embedding models cannot judge and probing them just wastes calls.
        let is_embedding = model
            .advertised
            .as_ref()
            .and_then(|a| a.get("is_embedding"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if is_embedding {
            skipped += 1;
            continue;
        }
        match probe(state, provider_id, &model.model_id).await {
            Ok(result) => {
                probed += 1;
                if result["usable_as_judge"] == json!(true) {
                    usable += 1;
                }
            }
            Err(err) => failed.push(json!({
                "model_id": model.model_id,
                "error": err.to_string(),
            })),
        }
    }
    Ok(json!({
        "probed": probed,
        "usable_as_judge": usable,
        "skipped_embedding": skipped,
        "failed": failed,
    }))
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignRoleRequest {
    pub role: String,
    /// Both `None` clears the role.
    pub provider_id: Option<i64>,
    pub model_id: Option<String>,
}

pub async fn assign_role(
    state: &AppState,
    request: AssignRoleRequest,
) -> Result<serde_json::Value, AppError> {
    guard(state)?;
    match (request.provider_id, request.model_id.as_deref()) {
        (Some(provider_id), Some(model_id)) => {
            db::assign_role(&state.db, &request.role, provider_id, model_id).await?;
        }
        (None, None) => db::clear_role(&state.db, &request.role).await?,
        _ => {
            return Err(AppError::BadRequest(
                "pass both provider_id and model_id to assign a role, or neither to clear it"
                    .into(),
            ))
        }
    }
    roles(state).await
}

/// Which model holds each role, and what it can do.
pub async fn roles(state: &AppState) -> Result<serde_json::Value, AppError> {
    guard(state)?;
    let mut out = serde_json::Map::new();
    for role in db::ROLES {
        let assigned = db::model_for_role(&state.db, role).await?;
        out.insert(
            role.to_string(),
            match assigned {
                Some(model) => json!({
                    "provider_id": model.provider_id,
                    "provider_name": model.provider_name,
                    "model_id": model.model_id,
                    "display_name": model.display_name,
                    "capabilities": model.capabilities,
                }),
                None => serde_json::Value::Null,
            },
        );
    }
    Ok(serde_json::Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// People paste keys into anything labelled "key". Storing one would put a
    /// secret in the database and in every subsequent backup.
    #[test]
    fn a_pasted_secret_is_refused_in_place_of_a_variable_name() {
        for pasted in [
            "AIzaSyD-super-secret-looking-value-here",
            "sk-proj-abcdefghijklmnop",
            "some.key.with.dots",
            "path/to/key",
            &"x".repeat(80),
        ] {
            let err = reject_secret_looking_reference(pasted).unwrap_err();
            assert!(
                err.to_string().contains("NAME of an environment variable"),
                "{pasted} should be refused: {err}"
            );
        }
    }

    #[test]
    fn ordinary_variable_names_are_accepted() {
        for name in [
            "OVIS_GEMINI_API_KEY",
            "GEMINI_API_KEY",
            "ANTHROPIC_KEY",
            "MY_KEY_2",
        ] {
            assert!(reject_secret_looking_reference(name).is_ok(), "{name}");
        }
    }

    /// A missing variable is the most common setup mistake; the message has to
    /// name the variable it wanted.
    #[test]
    fn a_missing_environment_variable_names_itself_in_the_error() {
        let row = db::ProviderRow {
            id: 1,
            name: "gemini".into(),
            kind: "gemini".into(),
            base_url: None,
            api_key_ref: Some("DEFINITELY_NOT_SET_12345".into()),
            enabled: true,
            created_at: chrono::Utc::now(),
        };
        let err = connect(&row).unwrap_err();
        assert!(
            err.to_string().contains("DEFINITELY_NOT_SET_12345"),
            "{err}"
        );
        assert!(err.to_string().contains("not set on this process"), "{err}");
    }

    #[test]
    fn a_self_hosted_provider_needs_no_key_at_all() {
        let row = db::ProviderRow {
            id: 1,
            name: "local".into(),
            kind: "llamacpp".into(),
            base_url: Some("http://192.168.4.240:8082".into()),
            api_key_ref: None,
            enabled: true,
            created_at: chrono::Utc::now(),
        };
        assert!(connect(&row).is_ok());
    }
}
