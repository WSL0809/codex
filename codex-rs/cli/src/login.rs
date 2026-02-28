use codex_core::CodexAuth;
use codex_core::auth::AuthCredentialsStoreMode;
use codex_core::auth::AuthMode;
use codex_core::auth::CLIENT_ID;
use codex_core::auth::login_with_api_key;
use codex_core::auth::logout;
use codex_core::auth::read_openai_api_key_from_env;
use codex_core::config::Config;
use codex_login::ServerOptions;
use codex_login::run_device_code_login;
use codex_login::run_login_server;
use codex_protocol::config_types::ForcedLoginMethod;
use codex_utils_cli::CliConfigOverrides;
use std::io::IsTerminal;
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;

const CHATGPT_LOGIN_DISABLED_MESSAGE: &str =
    "ChatGPT login is disabled. Use API key login instead.";
const API_KEY_LOGIN_DISABLED_MESSAGE: &str =
    "API key login is disabled. Use ChatGPT login instead.";
const LOGIN_SUCCESS_MESSAGE: &str = "Successfully logged in";

fn print_login_server_start(actual_port: u16, auth_url: &str) {
    eprintln!(
        "Starting local login server on http://localhost:{actual_port}.\nIf your browser did not open, navigate to this URL to authenticate:\n\n{auth_url}\n\nOn a remote or headless machine? Use `codex login` and choose `Sign in with Device Code` (or run `codex login --device-auth`)."
    );
}

pub async fn login_with_chatgpt(
    codex_home: PathBuf,
    forced_chatgpt_workspace_id: Option<String>,
    cli_auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> std::io::Result<()> {
    let opts = ServerOptions::new(
        codex_home,
        CLIENT_ID.to_string(),
        forced_chatgpt_workspace_id,
        cli_auth_credentials_store_mode,
    );
    let server = run_login_server(opts)?;

    print_login_server_start(server.actual_port, &server.auth_url);

    server.block_until_done().await
}

pub async fn run_login_with_chatgpt(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;

    if matches!(config.forced_login_method, Some(ForcedLoginMethod::Api)) {
        eprintln!("{CHATGPT_LOGIN_DISABLED_MESSAGE}");
        std::process::exit(1);
    }

    let forced_chatgpt_workspace_id = config.forced_chatgpt_workspace_id.clone();

    match login_with_chatgpt(
        config.codex_home,
        forced_chatgpt_workspace_id,
        config.cli_auth_credentials_store_mode,
    )
    .await
    {
        Ok(_) => {
            eprintln!("{LOGIN_SUCCESS_MESSAGE}");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error logging in: {e}");
            std::process::exit(1);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InteractiveLoginChoice {
    Browser,
    DeviceCode,
    ApiKeyFromEnv,
    Cancel,
}

fn parse_interactive_login_choice(input: &str) -> Option<InteractiveLoginChoice> {
    match input.trim().to_ascii_lowercase().as_str() {
        "" | "1" | "browser" | "chatgpt" => Some(InteractiveLoginChoice::Browser),
        "2" | "device" | "device-code" | "devicecode" => Some(InteractiveLoginChoice::DeviceCode),
        "3" | "api" | "api-key" | "apikey" => Some(InteractiveLoginChoice::ApiKeyFromEnv),
        "q" | "quit" | "exit" => Some(InteractiveLoginChoice::Cancel),
        _ => None,
    }
}

pub async fn run_login_with_interactive_menu(
    cli_config_overrides: CliConfigOverrides,
    issuer_base_url: Option<String>,
    client_id: Option<String>,
) -> ! {
    let config = load_config_or_exit(cli_config_overrides.clone()).await;
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        run_login_with_chatgpt(cli_config_overrides).await;
    }

    let chatgpt_login_allowed = !matches!(config.forced_login_method, Some(ForcedLoginMethod::Api));
    let api_key_login_allowed =
        !matches!(config.forced_login_method, Some(ForcedLoginMethod::Chatgpt));

    loop {
        eprintln!();
        eprintln!("Choose a login method:");
        if chatgpt_login_allowed {
            eprintln!("  1) Sign in with ChatGPT (browser)");
            eprintln!("  2) Sign in with Device Code");
        } else {
            eprintln!("  1) Sign in with ChatGPT (browser) [disabled]");
            eprintln!("  2) Sign in with Device Code [disabled]");
        }
        if api_key_login_allowed {
            eprintln!("  3) Use OPENAI_API_KEY from environment");
        } else {
            eprintln!("  3) Use OPENAI_API_KEY from environment [disabled]");
        }
        eprintln!("  q) Cancel");
        eprint!("Selection [1]: ");
        if let Err(err) = std::io::stderr().flush() {
            eprintln!("Failed to flush prompt: {err}");
            std::process::exit(1);
        }

        let mut input = String::new();
        match std::io::stdin().read_line(&mut input) {
            Ok(0) => {
                eprintln!("No input received.");
                std::process::exit(1);
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("Failed to read selection: {err}");
                std::process::exit(1);
            }
        }

        let Some(choice) = parse_interactive_login_choice(&input) else {
            eprintln!("Invalid selection. Choose 1, 2, 3, or q.");
            continue;
        };

        match choice {
            InteractiveLoginChoice::Browser => {
                if !chatgpt_login_allowed {
                    eprintln!("{CHATGPT_LOGIN_DISABLED_MESSAGE}");
                    continue;
                }
                run_login_with_chatgpt(cli_config_overrides.clone()).await;
            }
            InteractiveLoginChoice::DeviceCode => {
                if !chatgpt_login_allowed {
                    eprintln!("{CHATGPT_LOGIN_DISABLED_MESSAGE}");
                    continue;
                }
                run_login_with_device_code_fallback_to_browser(
                    cli_config_overrides.clone(),
                    issuer_base_url.clone(),
                    client_id.clone(),
                )
                .await;
            }
            InteractiveLoginChoice::ApiKeyFromEnv => {
                if !api_key_login_allowed {
                    eprintln!("{API_KEY_LOGIN_DISABLED_MESSAGE}");
                    continue;
                }

                let Some(api_key) = read_openai_api_key_from_env() else {
                    eprintln!(
                        "OPENAI_API_KEY is not set. Set it first, then run `codex login` again or pipe the key with `printenv OPENAI_API_KEY | codex login --with-api-key`."
                    );
                    continue;
                };
                run_login_with_api_key(cli_config_overrides.clone(), api_key).await;
            }
            InteractiveLoginChoice::Cancel => {
                eprintln!("Login canceled.");
                std::process::exit(1);
            }
        }
    }
}

pub async fn run_login_with_api_key(
    cli_config_overrides: CliConfigOverrides,
    api_key: String,
) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;

    if matches!(config.forced_login_method, Some(ForcedLoginMethod::Chatgpt)) {
        eprintln!("{API_KEY_LOGIN_DISABLED_MESSAGE}");
        std::process::exit(1);
    }

    match login_with_api_key(
        &config.codex_home,
        &api_key,
        config.cli_auth_credentials_store_mode,
    ) {
        Ok(_) => {
            eprintln!("{LOGIN_SUCCESS_MESSAGE}");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error logging in: {e}");
            std::process::exit(1);
        }
    }
}

pub fn read_api_key_from_stdin() -> String {
    let mut stdin = std::io::stdin();

    if stdin.is_terminal() {
        eprintln!(
            "--with-api-key expects the API key on stdin. Try piping it, e.g. `printenv OPENAI_API_KEY | codex login --with-api-key`."
        );
        std::process::exit(1);
    }

    eprintln!("Reading API key from stdin...");

    let mut buffer = String::new();
    if let Err(err) = stdin.read_to_string(&mut buffer) {
        eprintln!("Failed to read API key from stdin: {err}");
        std::process::exit(1);
    }

    let api_key = buffer.trim().to_string();
    if api_key.is_empty() {
        eprintln!("No API key provided via stdin.");
        std::process::exit(1);
    }

    api_key
}

/// Login using the OAuth device code flow.
pub async fn run_login_with_device_code(
    cli_config_overrides: CliConfigOverrides,
    issuer_base_url: Option<String>,
    client_id: Option<String>,
) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    if matches!(config.forced_login_method, Some(ForcedLoginMethod::Api)) {
        eprintln!("{CHATGPT_LOGIN_DISABLED_MESSAGE}");
        std::process::exit(1);
    }
    let forced_chatgpt_workspace_id = config.forced_chatgpt_workspace_id.clone();
    let mut opts = ServerOptions::new(
        config.codex_home,
        client_id.unwrap_or(CLIENT_ID.to_string()),
        forced_chatgpt_workspace_id,
        config.cli_auth_credentials_store_mode,
    );
    if let Some(iss) = issuer_base_url {
        opts.issuer = iss;
    }
    match run_device_code_login(opts).await {
        Ok(()) => {
            eprintln!("{LOGIN_SUCCESS_MESSAGE}");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error logging in with device code: {e}");
            std::process::exit(1);
        }
    }
}

/// Prefers device-code login (with `open_browser = false`) when headless environment is detected, but keeps
/// `codex login` working in environments where device-code may be disabled/feature-gated.
/// If `run_device_code_login` returns `ErrorKind::NotFound` ("device-code unsupported"), this
/// falls back to starting the local browser login server.
pub async fn run_login_with_device_code_fallback_to_browser(
    cli_config_overrides: CliConfigOverrides,
    issuer_base_url: Option<String>,
    client_id: Option<String>,
) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    if matches!(config.forced_login_method, Some(ForcedLoginMethod::Api)) {
        eprintln!("{CHATGPT_LOGIN_DISABLED_MESSAGE}");
        std::process::exit(1);
    }

    let forced_chatgpt_workspace_id = config.forced_chatgpt_workspace_id.clone();
    let mut opts = ServerOptions::new(
        config.codex_home,
        client_id.unwrap_or(CLIENT_ID.to_string()),
        forced_chatgpt_workspace_id,
        config.cli_auth_credentials_store_mode,
    );
    if let Some(iss) = issuer_base_url {
        opts.issuer = iss;
    }
    opts.open_browser = false;

    match run_device_code_login(opts.clone()).await {
        Ok(()) => {
            eprintln!("{LOGIN_SUCCESS_MESSAGE}");
            std::process::exit(0);
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!("Device code login is not enabled; falling back to browser login.");
                match run_login_server(opts) {
                    Ok(server) => {
                        print_login_server_start(server.actual_port, &server.auth_url);
                        match server.block_until_done().await {
                            Ok(()) => {
                                eprintln!("{LOGIN_SUCCESS_MESSAGE}");
                                std::process::exit(0);
                            }
                            Err(e) => {
                                eprintln!("Error logging in: {e}");
                                std::process::exit(1);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error logging in: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                eprintln!("Error logging in with device code: {e}");
                std::process::exit(1);
            }
        }
    }
}

pub async fn run_login_status(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;

    match CodexAuth::from_auth_storage(&config.codex_home, config.cli_auth_credentials_store_mode) {
        Ok(Some(auth)) => match auth.auth_mode() {
            AuthMode::ApiKey => match auth.get_token() {
                Ok(api_key) => {
                    eprintln!("Logged in using an API key - {}", safe_format_key(&api_key));
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("Unexpected error retrieving API key: {e}");
                    std::process::exit(1);
                }
            },
            AuthMode::Chatgpt => {
                eprintln!("Logged in using ChatGPT");
                std::process::exit(0);
            }
        },
        Ok(None) => {
            eprintln!("Not logged in");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error checking login status: {e}");
            std::process::exit(1);
        }
    }
}

pub async fn run_logout(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;

    match logout(&config.codex_home, config.cli_auth_credentials_store_mode) {
        Ok(true) => {
            eprintln!("Successfully logged out");
            std::process::exit(0);
        }
        Ok(false) => {
            eprintln!("Not logged in");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error logging out: {e}");
            std::process::exit(1);
        }
    }
}

async fn load_config_or_exit(cli_config_overrides: CliConfigOverrides) -> Config {
    let cli_overrides = match cli_config_overrides.parse_overrides() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing -c overrides: {e}");
            std::process::exit(1);
        }
    };

    match Config::load_with_cli_overrides(cli_overrides).await {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Error loading configuration: {e}");
            std::process::exit(1);
        }
    }
}

fn safe_format_key(key: &str) -> String {
    if key.len() <= 13 {
        return "***".to_string();
    }
    let prefix = &key[..8];
    let suffix = &key[key.len() - 5..];
    format!("{prefix}***{suffix}")
}

#[cfg(test)]
mod tests {
    use super::InteractiveLoginChoice;
    use super::parse_interactive_login_choice;
    use super::safe_format_key;
    use pretty_assertions::assert_eq;

    #[test]
    fn formats_long_key() {
        let key = "sk-proj-1234567890ABCDE";
        assert_eq!(safe_format_key(key), "sk-proj-***ABCDE");
    }

    #[test]
    fn short_key_returns_stars() {
        let key = "sk-proj-12345";
        assert_eq!(safe_format_key(key), "***");
    }

    #[test]
    fn parse_interactive_login_choice_handles_supported_values() {
        assert_eq!(
            parse_interactive_login_choice(""),
            Some(InteractiveLoginChoice::Browser)
        );
        assert_eq!(
            parse_interactive_login_choice("1"),
            Some(InteractiveLoginChoice::Browser)
        );
        assert_eq!(
            parse_interactive_login_choice("2"),
            Some(InteractiveLoginChoice::DeviceCode)
        );
        assert_eq!(
            parse_interactive_login_choice("3"),
            Some(InteractiveLoginChoice::ApiKeyFromEnv)
        );
        assert_eq!(
            parse_interactive_login_choice("q"),
            Some(InteractiveLoginChoice::Cancel)
        );
        assert_eq!(
            parse_interactive_login_choice("exit"),
            Some(InteractiveLoginChoice::Cancel)
        );
        assert_eq!(parse_interactive_login_choice("not-a-choice"), None);
    }
}
