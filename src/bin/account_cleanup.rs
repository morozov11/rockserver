//! One-shot, preview-first operator command for staging account cleanup.

use std::{env, process, str::FromStr};

use rockserver::{
    account_cleanup::{CleanupAction, CleanupError, confirmation_for, validate_confirmation},
    persistence::{DATABASE_URL_ENV, PostgresAccountStore},
};
use uuid::Uuid;

#[derive(Debug)]
enum Command {
    Preview,
    Apply {
        action: CleanupAction,
        id: Uuid,
        confirmation: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("account cleanup failed: {message}");
        process::exit(2);
    }
}

async fn run() -> Result<(), String> {
    let command = parse_args(env::args().skip(1))?;
    if !is_staging_environment(env::var("ROCKSERVER_CLEANUP_ENV").ok().as_deref()) {
        return Err(
            "ROCKSERVER_CLEANUP_ENV=staging is required; cleanup is staging-only".to_owned(),
        );
    }
    let database_url = env::var(DATABASE_URL_ENV)
        .map_err(|_| "DATABASE_URL is required in the protected runtime environment".to_owned())?;
    let store = PostgresAccountStore::connect(&database_url)
        .await
        .map_err(|_| "database operation failed".to_owned())?;
    match command {
        Command::Preview => {
            let preview = store
                .account_cleanup_preview()
                .await
                .map_err(|_| "database operation failed".to_owned())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&preview)
                    .map_err(|_| "could not serialize cleanup preview".to_owned())?
            );
        }
        Command::Apply {
            action,
            id,
            confirmation,
        } => {
            validate_confirmation(action, id, confirmation.as_deref())
                .map_err(|error| format_confirmation_error(error, action, id))?;
            let result = match action {
                CleanupAction::DeactivateAccount => store
                    .deactivate_account_for_operator(
                        id,
                        confirmation.as_deref().unwrap_or_default(),
                    )
                    .await
                    .map_err(format_cleanup_error)?,
                CleanupAction::RevokeDevice => store
                    .revoke_device_for_operator(id, confirmation.as_deref().unwrap_or_default())
                    .await
                    .map_err(format_cleanup_error)?,
                CleanupAction::RevokeCredential => store
                    .revoke_credential_for_operator(id, confirmation.as_deref().unwrap_or_default())
                    .await
                    .map_err(format_cleanup_error)?,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&result)
                    .map_err(|_| "could not serialize cleanup result".to_owned())?
            );
        }
    }
    store.close().await;
    Ok(())
}

fn is_staging_environment(value: Option<&str>) -> bool {
    value == Some("staging")
}

fn parse_args<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    if args.is_empty() || (args.len() == 1 && args[0] == "preview") {
        return Ok(Command::Preview);
    }
    if args.first().map(String::as_str) != Some("apply") {
        return Err(usage());
    }
    let action = args
        .get(1)
        .and_then(|value| CleanupAction::from_str(value).ok())
        .ok_or_else(usage)?;
    if args.len() != 6 || args[2] != "--id" || args[4] != "--confirm" {
        return Err(usage());
    }
    let id = Uuid::parse_str(&args[3]).map_err(|_| "--id must be one UUID".to_owned())?;
    Ok(Command::Apply {
        action,
        id,
        confirmation: Some(args[5].clone()),
    })
}

fn usage() -> String {
    "usage: account_cleanup [preview] | apply <account|device|credential> --id <UUID> --confirm '<ACTION> <UUID>'".to_owned()
}

fn format_confirmation_error(
    error: rockserver::account_cleanup::ConfirmationError,
    action: CleanupAction,
    id: Uuid,
) -> String {
    format!("{error}; expected `{}`", confirmation_for(action, id))
}

fn format_cleanup_error(error: CleanupError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::{Command, is_staging_environment, parse_args};
    use rockserver::account_cleanup::CleanupAction;

    #[test]
    fn no_arguments_are_preview_only() {
        assert!(matches!(
            parse_args(Vec::<String>::new()),
            Ok(Command::Preview)
        ));
    }

    #[test]
    fn cleanup_requires_the_explicit_staging_environment_marker() {
        assert!(is_staging_environment(Some("staging")));
        assert!(!is_staging_environment(Some("production")));
        assert!(!is_staging_environment(None));
    }

    #[test]
    fn apply_parser_never_turns_a_wildcard_into_a_target() {
        assert!(
            parse_args(vec![
                "apply".to_owned(),
                "account".to_owned(),
                "--id".to_owned(),
                "*".to_owned(),
                "--confirm".to_owned(),
                "DEACTIVATE ACCOUNT *".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn apply_requires_one_exact_uuid_target_and_confirmation_flag() {
        let args = [
            "apply",
            "device",
            "--id",
            "00000000-0000-0000-0000-000000000000",
            "--confirm",
            "REVOKE DEVICE 00000000-0000-0000-0000-000000000000",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        assert!(matches!(
            parse_args(args),
            Ok(Command::Apply {
                action: CleanupAction::RevokeDevice,
                ..
            })
        ));
        assert!(
            parse_args(vec![
                "apply".to_owned(),
                "device".to_owned(),
                "--id".to_owned(),
                "*".to_owned()
            ])
            .is_err()
        );
    }
}
