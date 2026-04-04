use crate::app;
use anyhow::{Result, anyhow};
use axoupdater::AxoUpdater;
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const APP_NAME: &str = env!("CARGO_PKG_NAME");
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const APP_REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const GITHUB_TOKEN_ENV: &str = "HURL_GITHUB_TOKEN";

#[derive(Clone, Debug, Eq, PartialEq)]
enum Command {
    RunTui,
    Demo,
    Help,
    Version,
    Update,
    UpdateHelp,
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command = parse_command(env::args_os().skip(1))?;

    match command {
        Command::RunTui => app::run().await,
        Command::Demo => app::run_demo().await,
        Command::Help => {
            print_help();
            Ok(())
        }
        Command::Version => {
            print_version();
            Ok(())
        }
        Command::UpdateHelp => {
            print_update_help();
            Ok(())
        }
        Command::Update => run_update().await.map_err(Into::into),
    }
}

fn parse_command<I>(args: I) -> Result<Command, Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = OsString>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    let command = match args.as_slice() {
        [] => Command::RunTui,
        [arg] if arg == "demo" => Command::Demo,
        [arg] if matches!(arg.as_str(), "help" | "--help" | "-h") => Command::Help,
        [arg] if matches!(arg.as_str(), "version" | "--version" | "-V") => Command::Version,
        [arg] if arg == "update" => Command::Update,
        [command, flag] if command == "update" && matches!(flag.as_str(), "--help" | "-h") => {
            Command::UpdateHelp
        }
        [command, topic] if command == "help" && topic == "update" => Command::UpdateHelp,
        [unknown, ..] => {
            return Err(anyhow!(
                "Unknown command `{unknown}`.\n\nRun `hurl --help` to see available commands."
            )
            .into());
        }
    };

    Ok(command)
}

async fn run_update() -> Result<()> {
    let current_exe = current_executable_path();

    if looks_like_homebrew_install(current_exe.as_deref()) {
        print_homebrew_update_hint();
        return Ok(());
    }

    let mut updater = AxoUpdater::new_for(APP_NAME);
    if let Ok(token) = env::var(GITHUB_TOKEN_ENV) {
        updater.set_github_token(&token);
    }

    if updater.load_receipt().is_ok() {
        if updater.check_receipt_is_for_this_executable()? {
            println!("Checking for updates...");
            let update_result = updater.run().await?;
            if update_result.is_some() {
                println!("Installed a newer version of hurl.");
            } else {
                println!("hurl is already up to date.");
            }
            return Ok(());
        }
    }

    print_manual_update_hint(current_exe.as_deref());
    Ok(())
}

fn current_executable_path() -> Option<PathBuf> {
    let path = env::current_exe().ok()?;
    path.canonicalize().ok().or(Some(path))
}

fn looks_like_homebrew_install(path: Option<&Path>) -> bool {
    let Some(path) = path else {
        return false;
    };

    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.contains(&format!("/Cellar/{APP_NAME}/"))
}

fn print_help() {
    println!(
        "\
hurl {APP_VERSION}
A terminal UI API client for humans.

Usage:
  hurl
  hurl demo
  hurl <command>

Commands:
  help       Show help for hurl or a subcommand
  demo       Launch the built-in demo library against public test APIs
  version    Show the hurl version
  update     Update hurl when this install supports it

Flags:
  -h, --help       Show help
  -V, --version    Show version

Running `hurl` without a command launches the TUI.
Run `hurl demo` to explore an isolated demo workspace.
Repository: {APP_REPOSITORY}"
    );
}

fn print_update_help() {
    println!(
        "\
Usage:
  hurl update

Behavior:
  - Uses the built-in cargo-dist updater for supported shell and PowerShell installs
  - Tells Homebrew users to run `brew upgrade hurl`
  - Falls back to manual reinstall guidance for other installs

Environment:
  {GITHUB_TOKEN_ENV}    Optional GitHub token for updater API requests"
    );
}

fn print_version() {
    println!("hurl {APP_VERSION}");
}

fn print_homebrew_update_hint() {
    println!("This copy of hurl appears to be installed with Homebrew.");
    println!("Run `brew upgrade hurl` to update it.");
}

fn print_manual_update_hint(current_exe: Option<&Path>) {
    println!("This copy of hurl is not eligible for automatic in-place updates.");

    if let Some(path) = current_exe {
        println!("Current executable: {}", path.display());
    }

    #[cfg(windows)]
    {
        println!("If you installed via the PowerShell installer, rerun:");
        println!(
            "  powershell -ExecutionPolicy Bypass -c \"irm {APP_REPOSITORY}/releases/latest/download/hurl-installer.ps1 | iex\""
        );
    }

    #[cfg(not(windows))]
    {
        println!("If you installed via the shell installer, rerun:");
        println!(
            "  curl --proto '=https' --tlsv1.2 -LsSf {APP_REPOSITORY}/releases/latest/download/hurl-installer.sh | sh"
        );
    }

    println!("Otherwise, download the latest release from:");
    println!("  {APP_REPOSITORY}/releases/latest");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Command {
        parse_command(args.iter().map(OsString::from)).unwrap()
    }

    #[test]
    fn parses_default_tui_launch() {
        assert_eq!(parse(&[]), Command::RunTui);
    }

    #[test]
    fn parses_help_forms() {
        assert_eq!(parse(&["help"]), Command::Help);
        assert_eq!(parse(&["--help"]), Command::Help);
        assert_eq!(parse(&["-h"]), Command::Help);
        assert_eq!(parse(&["help", "update"]), Command::UpdateHelp);
        assert_eq!(parse(&["update", "--help"]), Command::UpdateHelp);
    }

    #[test]
    fn parses_version_forms() {
        assert_eq!(parse(&["version"]), Command::Version);
        assert_eq!(parse(&["--version"]), Command::Version);
        assert_eq!(parse(&["-V"]), Command::Version);
    }

    #[test]
    fn parses_update_command() {
        assert_eq!(parse(&["update"]), Command::Update);
    }

    #[test]
    fn parses_demo_command() {
        assert_eq!(parse(&["demo"]), Command::Demo);
    }

    #[test]
    fn detects_homebrew_paths() {
        assert!(looks_like_homebrew_install(Some(Path::new(
            "/opt/homebrew/Cellar/hurl/0.3.1/bin/hurl"
        ))));
        assert!(looks_like_homebrew_install(Some(Path::new(
            "/home/linuxbrew/.linuxbrew/Cellar/hurl/0.3.1/bin/hurl"
        ))));
        assert!(!looks_like_homebrew_install(Some(Path::new(
            "/Users/me/.cargo/bin/hurl"
        ))));
    }
}
