use std::str::FromStr;
use std::sync::Arc;

use crate::{AppState, AppType, Database, ProviderService};

const HELP: &str = "\
CC Switch command-line provider switcher

Usage:
  ccs list <app>
  ccs use <app> <provider-id>

Commands:
  list    List configured providers and mark the current one
  use     Switch to a configured provider by ID

Apps:
  claude, claude-desktop, codex, gemini, grokbuild, opencode, openclaw, hermes

Options:
  -h, --help       Show this help
  -V, --version    Show the version";

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Version,
    List { app: AppType },
    Use { app: AppType, provider_id: String },
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Runtime(String),
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, CliError> {
    let args: Vec<String> = args.into_iter().collect();
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Command::Help);
    };

    match command {
        "help" | "-h" | "--help" if args.len() == 1 => Ok(Command::Help),
        "version" | "-V" | "--version" if args.len() == 1 => Ok(Command::Version),
        "list" if args.len() == 2 => Ok(Command::List {
            app: parse_app(&args[1])?,
        }),
        "use" if args.len() == 3 => Ok(Command::Use {
            app: parse_app(&args[1])?,
            provider_id: args[2].clone(),
        }),
        "list" => Err(CliError::Usage(
            "list requires exactly one <app> argument".to_string(),
        )),
        "use" => Err(CliError::Usage(
            "use requires <app> and <provider-id> arguments".to_string(),
        )),
        _ => Err(CliError::Usage(format!("unknown command: {command}"))),
    }
}

fn parse_app(value: &str) -> Result<AppType, CliError> {
    AppType::from_str(value).map_err(|err| CliError::Usage(err.to_string()))
}

fn execute(command: Command) -> Result<String, CliError> {
    match command {
        Command::Help => return Ok(HELP.to_string()),
        Command::Version => return Ok(env!("CARGO_PKG_VERSION").to_string()),
        Command::List { .. } | Command::Use { .. } => {}
    }

    crate::app_store::refresh_app_config_dir_override_from_disk();
    let db = Database::init().map_err(|err| CliError::Runtime(err.to_string()))?;
    let state = AppState::new(Arc::new(db));

    match command {
        Command::List { app } => list_providers(&state, app),
        Command::Use { app, provider_id } => switch_provider(&state, app, &provider_id),
        Command::Help | Command::Version => unreachable!(),
    }
}

fn list_providers(state: &AppState, app: AppType) -> Result<String, CliError> {
    let providers = ProviderService::list(state, app.clone())
        .map_err(|err| CliError::Runtime(err.to_string()))?;
    let current = ProviderService::current(state, app.clone())
        .map_err(|err| CliError::Runtime(err.to_string()))?;

    if providers.is_empty() {
        return Ok(format!("No providers configured for {}.", app.as_str()));
    }

    let mut lines = Vec::with_capacity(providers.len() + 1);
    lines.push("CURRENT\tID\tNAME".to_string());
    for (id, provider) in providers {
        let marker = if id == current { "*" } else { "" };
        lines.push(format!("{marker}\t{id}\t{}", provider.name));
    }
    Ok(lines.join("\n"))
}

fn switch_provider(state: &AppState, app: AppType, provider_id: &str) -> Result<String, CliError> {
    let providers = ProviderService::list(state, app.clone())
        .map_err(|err| CliError::Runtime(err.to_string()))?;
    let provider = providers.get(provider_id).ok_or_else(|| {
        CliError::Runtime(format!(
            "provider '{provider_id}' is not configured for {}",
            app.as_str()
        ))
    })?;
    let provider_name = provider.name.clone();

    let result = ProviderService::switch(state, app.clone(), provider_id)
        .map_err(|err| CliError::Runtime(err.to_string()))?;

    let mut output = format!(
        "Switched {} to {} ({}).",
        app.as_str(),
        provider_name,
        provider_id
    );
    for warning in result.warnings {
        output.push_str("\nwarning: ");
        output.push_str(&warning);
    }
    Ok(output)
}

pub fn run_cli() -> std::process::ExitCode {
    match parse_args(std::env::args().skip(1)).and_then(execute) {
        Ok(output) => {
            println!("{output}");
            std::process::ExitCode::SUCCESS
        }
        Err(CliError::Usage(message)) => {
            eprintln!("error: {message}\n\n{HELP}");
            std::process::ExitCode::from(2)
        }
        Err(CliError::Runtime(message)) => {
            eprintln!("error: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Provider;
    use serde_json::json;

    struct TestHome {
        dir: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }

    impl TestHome {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("create temporary home");
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            Self { dir, previous }
        }

        fn path(&self) -> &std::path::Path {
            self.dir.path()
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parses_list_command() {
        assert_eq!(
            parse_args(args(&["list", "codex"])).ok(),
            Some(Command::List {
                app: AppType::Codex
            })
        );
    }

    #[test]
    fn parses_use_command_and_app_alias() {
        assert_eq!(
            parse_args(args(&["use", "claude_desktop", "official"])).ok(),
            Some(Command::Use {
                app: AppType::ClaudeDesktop,
                provider_id: "official".to_string()
            })
        );
    }

    #[test]
    fn rejects_missing_use_argument() {
        assert!(matches!(
            parse_args(args(&["use", "codex"])),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn rejects_unknown_app() {
        assert!(matches!(
            parse_args(args(&["list", "unknown"])),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    #[serial_test::serial]
    fn switches_provider_through_cli_path() {
        let home = TestHome::new();

        let db = Arc::new(Database::memory().expect("create in-memory database"));
        let provider = Provider::with_id(
            "test-provider".to_string(),
            "Test Provider".to_string(),
            json!({
                "baseUrl": "https://api.example.com",
                "apiKey": "test-key",
                "api": "openai-completions",
                "models": []
            }),
            None,
        );
        db.save_provider(AppType::OpenClaw.as_str(), &provider)
            .expect("save provider");
        let state = AppState::new(db);

        let output =
            switch_provider(&state, AppType::OpenClaw, "test-provider").expect("switch provider");

        assert_eq!(
            output,
            "Switched openclaw to Test Provider (test-provider)."
        );
        assert!(home.path().join(".openclaw/openclaw.json").exists());
    }
}
