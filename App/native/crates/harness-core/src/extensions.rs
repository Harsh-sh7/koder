//! What the agent can be extended with: skills on disk and MCP servers.
//!
//! Skills are found where the engine's filesystem skill provider finds them —
//! `<project>/.dsh/skills`, `<project>/.agents/skills`, `<dsh home>/skills`,
//! and `~/.agents/skills`, each holding `<name>/SKILL.md` bundles or flat
//! `<name>.md` files with `name` and `description` frontmatter — so the panel
//! lists exactly what a session can load, plus the harness's own `orchestrate`
//! skill, which ships inside the preset plugin rather than on disk.
//!
//! MCP servers are declared in `<workspace>/.harness/mcp.json`, in the
//! `mcpServers` shape other agent clients use, and travel to the engine in
//! every `session/new` and `session/resume`, which is the one place ACP
//! accepts them.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

/// One skill a session can load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The skill's name, as the model calls it.
    pub name: String,
    /// What it is for, from its frontmatter.
    pub description: String,
    /// Where it came from: `harness`, `project`, or `user`.
    pub source: &'static str,
}

/// One MCP server the workspace declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    /// The server's name.
    pub name: String,
    /// `stdio` or `http`.
    pub transport: &'static str,
    /// The command or URL, for the panel.
    pub target: String,
    /// The server as ACP expects it in `mcpServers`.
    pub wire: Value,
}

/// The harness's own skill, registered by the preset plugin.
const HARNESS_SKILL: (&str, &str) = (
    "orchestrate",
    "Decompose a multi-part job into file-disjoint waves, prove each part, and land the budget.",
);

/// Every skill a session in `workspace` can load, harness first, then by name.
///
/// @param workspace workspace root
/// @param dsh_home the harness home, whose `skills` directory is a user root
/// @returns the skills, with duplicates (same name) kept once, the first root winning
pub fn skills(workspace: &Path, dsh_home: Option<&Path>) -> Vec<Skill> {
    let mut found = vec![Skill {
        name: HARNESS_SKILL.0.to_string(),
        description: HARNESS_SKILL.1.to_string(),
        source: "harness",
    }];
    let project = project_root(workspace);
    let mut roots: Vec<(PathBuf, &'static str)> = vec![
        (project.join(".dsh").join("skills"), "project"),
        (project.join(".agents").join("skills"), "project"),
    ];
    if let Some(home) = dsh_home {
        roots.push((home.join("skills"), "user"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        roots.push((PathBuf::from(home).join(".agents").join("skills"), "user"));
    }
    for (root, source) in roots {
        let mut here: Vec<Skill> = scan(&root)
            .into_iter()
            .map(|(name, description)| Skill { name, description, source })
            .collect();
        here.sort_by(|left, right| left.name.cmp(&right.name));
        for skill in here {
            if !found.iter().any(|known| known.name == skill.name) {
                found.push(skill);
            }
        }
    }
    found
}

/// The nearest ancestor holding `.git`, which is where the engine anchors
/// project skills; the workspace itself when there is none.
fn project_root(workspace: &Path) -> PathBuf {
    workspace
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(workspace)
        .to_path_buf()
}

/// The skills directly inside one root.
fn scan(root: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name() != ".system")
        .filter_map(|entry| {
            let path = entry.path();
            let file = if path.is_dir() {
                path.join("SKILL.md")
            } else if path.extension().is_some_and(|ext| ext == "md") {
                path
            } else {
                return None;
            };
            frontmatter(&std::fs::read_to_string(file).ok()?)
        })
        .collect()
}

/// A skill file's `name` and `description`, from its YAML frontmatter.
///
/// Only the two scalar keys the catalog shows are read, on one line each —
/// the same subset every skill in the wild uses for them.
///
/// @param text the file
/// @returns the pair, or nothing without both keys
pub fn frontmatter(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let (mut name, mut description) = (None, None);
    for line in lines {
        let line = line.trim_end();
        if line.trim() == "---" {
            break;
        }
        let unquote = |value: &str| value.trim().trim_matches(['"', '\'']).to_string();
        if let Some(value) = line.strip_prefix("name:") {
            name = Some(unquote(value));
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(unquote(value));
        }
    }
    Some((name.filter(|name| !name.is_empty())?, description.unwrap_or_default()))
}

/// Where a workspace declares its MCP servers.
///
/// @param workspace workspace root
/// @returns `<workspace>/.harness/mcp.json`
pub fn mcp_path(workspace: &Path) -> PathBuf {
    workspace.join(".harness").join("mcp.json")
}

/// The MCP servers a workspace declares.
///
/// Accepts the common document shape — `{"mcpServers": {"name": {"command",
/// "args", "env"} | {"url", "headers"}}}` — and converts each entry into the
/// list form ACP takes (`env` and `headers` as name/value pairs).
///
/// @param workspace workspace root
/// @returns the servers, or why the document could not be read
pub fn mcp_servers(workspace: &Path) -> Result<Vec<McpServer>, String> {
    let path = mcp_path(workspace);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let document: Value =
        serde_json::from_str(&raw).map_err(|err| format!("{}: {err}", path.display()))?;
    let Some(servers) = document.get("mcpServers").and_then(Value::as_object) else {
        return Err(format!("{}: expected an \"mcpServers\" object", path.display()));
    };
    let pairs = |value: Option<&Value>| -> Vec<Value> {
        value
            .and_then(Value::as_object)
            .map(|map: &Map<String, Value>| {
                map.iter()
                    .map(|(name, value)| json!({ "name": name, "value": value.as_str().unwrap_or_default() }))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut list = Vec::new();
    for (name, server) in servers {
        if let Some(url) = server.get("url").and_then(Value::as_str) {
            list.push(McpServer {
                name: name.clone(),
                transport: "http",
                target: url.to_string(),
                wire: json!({ "type": "http", "name": name, "url": url, "headers": pairs(server.get("headers")) }),
            });
        } else if let Some(command) = server.get("command").and_then(Value::as_str) {
            let args: Vec<Value> = server
                .get("args")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            list.push(McpServer {
                name: name.clone(),
                transport: "stdio",
                target: command.to_string(),
                wire: json!({ "name": name, "command": command, "args": args, "env": pairs(server.get("env")) }),
            });
        } else {
            return Err(format!("{}: server \"{name}\" needs a command or a url", path.display()));
        }
    }
    list.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-ext-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn skills_come_from_the_roots_the_engine_scans_and_the_harness_skill_leads() {
        let root = scratch("skills");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let bundle = root.join(".agents/skills/release-notes");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("SKILL.md"), "---\nname: release-notes\ndescription: \"Draft notes\"\n---\nbody").unwrap();
        std::fs::create_dir_all(root.join(".dsh/skills")).unwrap();
        std::fs::write(root.join(".dsh/skills/triage.md"), "---\nname: triage\ndescription: Sort issues\n---\n").unwrap();
        std::fs::write(root.join(".dsh/skills/notes.txt"), "not a skill").unwrap();
        std::fs::write(root.join(".dsh/skills/broken.md"), "no frontmatter").unwrap();
        let names: Vec<(String, &str)> = skills(&root, None)
            .into_iter()
            .filter(|skill| skill.source != "user")
            .map(|skill| (skill.name, skill.source))
            .collect();
        assert_eq!(
            names,
            [
                ("orchestrate".to_string(), "harness"),
                ("triage".to_string(), "project"),
                ("release-notes".to_string(), "project"),
            ]
        );
    }

    #[test]
    fn mcp_servers_convert_to_the_acp_list_and_a_bad_document_says_where() {
        let root = scratch("mcp");
        assert!(mcp_servers(&root).unwrap().is_empty(), "no document is no servers");
        std::fs::create_dir_all(root.join(".harness")).unwrap();
        std::fs::write(
            mcp_path(&root),
            r#"{"mcpServers":{"files":{"command":"npx","args":["-y","fs"],"env":{"ROOT":"/x"}},"docs":{"url":"https://m.example/mcp"}}}"#,
        )
        .unwrap();
        let servers = mcp_servers(&root).unwrap();
        assert_eq!(servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(), ["docs", "files"]);
        assert_eq!(servers[1].wire["env"][0]["name"], "ROOT");
        assert_eq!(servers[0].wire["type"], "http");
        std::fs::write(mcp_path(&root), r#"{"mcpServers":{"x":{}}}"#).unwrap();
        assert!(mcp_servers(&root).unwrap_err().contains("needs a command or a url"));
    }
}
