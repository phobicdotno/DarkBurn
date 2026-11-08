//! The registration text for an MCP client: the exact `claude mcp add` line and the Claude
//! Desktop JSON for an installed `darkburn-mcp` binary. Used by `darkburn-mcp --print-config`
//! and by the app's "AI agents (MCP)" settings page, so both print the same thing.

use std::path::{Path, PathBuf};

use serde_json::json;

/// The file name of the stdio server next to the app.
pub const SERVER_EXE: &str = if cfg!(windows) { "darkburn-mcp.exe" } else { "darkburn-mcp" };

/// Where the stdio server lives when the app is `app_exe`: next to it.
pub fn server_path_next_to(app_exe: &Path) -> PathBuf {
    app_exe.with_file_name(SERVER_EXE)
}

fn quoted(p: &Path) -> String {
    let t = p.display().to_string();
    if t.contains(' ') { format!("\"{t}\"") } else { t }
}

/// `claude mcp add darkburn -- <exe>`.
pub fn claude_add_command(server_exe: &Path) -> String {
    format!("claude mcp add darkburn -- {}", quoted(server_exe))
}

/// The `claude_desktop_config.json` entry.
pub fn desktop_config_json(server_exe: &Path) -> String {
    let v = json!({ "mcpServers": { "darkburn": { "command": server_exe.display().to_string(), "args": [] } } });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// Everything `--print-config` prints.
pub fn print_config(server_exe: &Path) -> String {
    format!(
        "Claude Code (run once):\n\n  {}\n\nClaude Desktop (claude_desktop_config.json):\n\n{}\n\nTurn on \"MCP control\" in DarkBurn: Settings > AI agents (MCP). Without a window: `darkburn --mcp-headless` serves the same tools over stdio.\n",
        claude_add_command(server_exe),
        desktop_config_json(server_exe)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_add_line_quotes_paths_with_spaces() {
        assert_eq!(claude_add_command(Path::new("C:\\a\\darkburn-mcp.exe")), "claude mcp add darkburn -- C:\\a\\darkburn-mcp.exe");
        assert_eq!(
            claude_add_command(Path::new("C:\\Program Files\\D\\darkburn-mcp.exe")),
            "claude mcp add darkburn -- \"C:\\Program Files\\D\\darkburn-mcp.exe\""
        );
        let j: serde_json::Value = serde_json::from_str(&desktop_config_json(Path::new("C:\\a\\x.exe"))).unwrap();
        assert_eq!(j["mcpServers"]["darkburn"]["command"], "C:\\a\\x.exe");
    }
}
