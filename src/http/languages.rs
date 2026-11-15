//! Communication language discovery and account language configuration HTTP handlers.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::languages::LanguageSummary;

use super::{
    account::match_native_session,
    state::AppState,
    transport::{error_response, request_id, unauthorized_response, with_request_id},
};

/// Public DTO representing the list of available communication languages.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LanguageListDto {
    /// Available languages sorted alphabetically by language code.
    pub languages: Vec<LanguageSummary>,
}

/// DTO representing the current preferred communication language of an account.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountLanguageDto {
    /// 2-3 letter lowercase language code, or `null` when not set.
    pub language: Option<String>,
}

/// Request payload for setting or clearing an account's preferred communication language.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAccountLanguageDto {
    /// 2-3 letter lowercase language code, or `null` to clear preference.
    pub language: Option<String>,
}

/// Returns the sorted list of communication languages supported across the server and AI services.
pub async fn list_languages(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    let languages = state.language_intersection.published_languages().await;
    with_request_id(
        Json(LanguageListDto { languages }).into_response(),
        &request_id,
    )
}

/// Returns the preferred communication language for the authenticated account owner.
pub async fn get_account_language(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    let Some(session) = match_native_session(&state, &headers, &request_id).await else {
        return unauthorized_response(&request_id);
    };
    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };
    match store.user_language(session.user_id).await {
        Ok(language) => with_request_id(
            Json(AccountLanguageDto { language }).into_response(),
            &request_id,
        ),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

/// Updates or clears the preferred communication language for the authenticated account owner.
pub async fn update_account_language(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateAccountLanguageDto>,
) -> Response {
    let request_id = request_id(&headers);
    let Some(session) = match_native_session(&state, &headers, &request_id).await else {
        return unauthorized_response(&request_id);
    };

    if let Some(ref lang) = payload.language {
        let is_valid = state.language_intersection.is_published(lang).await
            || crate::languages::bundled_registry().is_valid(lang);
        if !is_valid {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_language",
                "Specified language is not supported or not published.",
                &request_id,
                json!({ "language": lang }),
            );
        }
    }

    let Some(store) = state.account_store.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        );
    };

    match store
        .set_user_language(session.user_id, payload.language.as_deref())
        .await
    {
        Ok(true) => with_request_id(
            Json(AccountLanguageDto {
                language: payload.language,
            })
            .into_response(),
            &request_id,
        ),
        Ok(false) => unauthorized_response(&request_id),
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Account service is unavailable.",
            &request_id,
            json!({}),
        ),
    }
}

#[cfg(test)]
#[path = "languages/tests.rs"]
mod tests;
