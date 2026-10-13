//! vivido-mcp: terminal automation as MCP tools.
//!
//! A stdio MCP server that lets any MCP client — including ones with no shell,
//! like desktop chat apps and IDE panes — see and drive Vivido: screenshots
//! arrive as images the model looks at directly, and actions that type or run
//! things are gated by safety tiers chosen at launch with `--allow`.

use std::path::PathBuf;

use clap::Parser;

use vivido_mcp::image_tools::DEFAULT_MAX_DIM;
use vivido_mcp::server::{Server, ServerConfig};
use vivido_mcp::{mcp, tiers};

/// Stdio MCP server for Vivido terminal automation.
#[derive(Parser, Debug)]
#[command(
    name = "vivido-mcp",
    version,
    about,
    long_about = "Stdio MCP server for Vivido terminal automation.\n\nScreens and state are readable by default; typing and process control are gated by tiers:\n\n  vivido-mcp                         observe only: screenshots, reads, waits\n  vivido-mcp --allow input            plus typing, keys, paste, mouse, file drops\n  vivido-mcp --allow process          everything, incl. windows and headless sessions\n  vivido-mcp --allow input --target build\n\nTiers are cumulative and may be comma-separated (--allow input,process),\nor given via VIVIDO_MCP_ALLOW. Point at one session with --socket PATH or\n--target NAME; otherwise the server resolves like `vivido msg`."
)]
struct Args {
    /// Safety tiers to enable: observe (default), input, process, all.
    /// Repeatable and comma-separated; cumulative (process implies input).
    #[arg(long = "allow", value_name = "TIER", env = "VIVIDO_MCP_ALLOW")]
    allow: Vec<String>,

    /// Explicit endpoint path (Unix socket, Windows named pipe). A leading
    /// `~/` expands to the home directory.
    #[arg(long = "socket", value_name = "PATH")]
    socket: Option<PathBuf>,

    /// Named session to attach to (else VIVIDO_SESSION / VIVIDA_TARGET).
    #[arg(long = "target", value_name = "NAME")]
    target: Option<String>,

    /// Vivido binary for headless lifecycle.
    #[arg(long = "vivido-bin", value_name = "PATH", default_value = "vivido", env = "VIVIDO_BIN")]
    vivido_bin: String,

    /// Default longest-edge cap for screenshots (64-4096).
    #[arg(long = "max-image-dim", value_name = "PX", default_value_t = DEFAULT_MAX_DIM)]
    max_image_dim: u32,
}

fn main() {
    let args = Args::parse();
    let allowance = match tiers::parse_allow(&args.allow) {
        Ok(allowance) => allowance,
        Err(message) => {
            eprintln!("vivido-mcp: {message}");
            std::process::exit(1);
        }
    };
    if !(64..=4096).contains(&args.max_image_dim) {
        eprintln!("vivido-mcp: --max-image-dim must be 64 through 4096");
        std::process::exit(1);
    }
    if let Some(target) = &args.target
        && let Err(message) = vivido_mcp::endpoint::validate_session_name(target)
    {
        eprintln!("vivido-mcp: --target: {message}");
        std::process::exit(1);
    }
    let socket = args.socket.as_ref().map(|path| expand_tilde(path));
    let tiers = if allowance.process {
        "observe,input,process"
    } else if allowance.input {
        "observe,input"
    } else {
        "observe"
    };
    eprintln!("vivido-mcp {} (tiers: {tiers})", env!("CARGO_PKG_VERSION"));
    let mut server = Server::new(ServerConfig {
        allowance,
        socket,
        target: args.target,
        vivido_bin: args.vivido_bin,
        default_max_dim: args.max_image_dim,
    });
    mcp::serve(&mut server);
}

/// Expand a leading `~/` (or bare `~`) against `$HOME`. Anything else,
/// including `~user/`, passes through untouched.
fn expand_tilde(path: &std::path::Path) -> PathBuf {
    let text = path.to_string_lossy();
    let rest = if text == "~" { Some("") } else { text.strip_prefix("~/") };
    match (rest, std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expands_against_home() {
        // SAFETY: this binary's tests never run in parallel with code that
        // reads HOME; the mutation is scoped to this test process.
        unsafe { std::env::set_var("HOME", "/home/tester") };
        assert_eq!(expand_tilde(&PathBuf::from("~/a.sock")), PathBuf::from("/home/tester/a.sock"));
        assert_eq!(expand_tilde(&PathBuf::from("~")), PathBuf::from("/home/tester"));
        assert_eq!(expand_tilde(&PathBuf::from("/tmp/a.sock")), PathBuf::from("/tmp/a.sock"));
        assert_eq!(expand_tilde(&PathBuf::from("~other/a.sock")), PathBuf::from("~other/a.sock"));
    }
}
