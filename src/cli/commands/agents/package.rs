use super::support::expand_home_path;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::Result;

use super::support::{find_executable, home_dir, PackagedSkillFile};

pub(in crate::cli) const OPENCLAW_SKILL_NAME: &str = "clipboard-memory";
pub(in crate::cli) const OPENCLAW_SHARED_ROOT: &str = ".openclaw/skills";
pub(in crate::cli) const OPENCLAW_WORKSPACE_ROOT: &str = ".openclaw/workspace";
pub(in crate::cli) const HERMES_SKILL_NAME: &str = "clipboard-memory";
pub(in crate::cli) const HERMES_SKILL_ROOT: &str = ".hermes/skills/productivity";

pub(in crate::cli) const OPENCLAW_SKILL_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/openclaw/clipboard-memory/SKILL.md"
));
pub(in crate::cli) const OPENCLAW_COMMANDS_REF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/openclaw/clipboard-memory/references/commands.md"
));
pub(in crate::cli) const OPENCLAW_TROUBLESHOOTING_REF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/openclaw/clipboard-memory/references/troubleshooting.md"
));
pub(in crate::cli) const OPENCLAW_JSON_SCHEMA_REF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/openclaw/clipboard-memory/references/json-schema.md"
));
pub(in crate::cli) const OPENCLAW_EXAMPLES_REF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/openclaw/clipboard-memory/references/examples.md"
));
pub(in crate::cli) const OPENCLAW_SETUP_CHECK_REF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/openclaw/clipboard-memory/references/setup-check.md"
));
pub(in crate::cli) const OPENCLAW_CHECK_SETUP_SH: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/openclaw/clipboard-memory/scripts/check-setup.sh"
));

pub(in crate::cli) const HERMES_SKILL_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/hermes/clipboard-memory/SKILL.md"
));
pub(in crate::cli) const HERMES_COMMANDS_REF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/hermes/clipboard-memory/references/commands.md"
));
pub(in crate::cli) const HERMES_TROUBLESHOOTING_REF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/hermes/clipboard-memory/references/troubleshooting.md"
));
pub(in crate::cli) const HERMES_JSON_SCHEMA_REF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/hermes/clipboard-memory/references/json-schema.md"
));
pub(in crate::cli) const HERMES_EXAMPLES_REF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/hermes/clipboard-memory/references/examples.md"
));
pub(in crate::cli) const HERMES_SETUP_CHECK_REF: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/hermes/clipboard-memory/references/setup-check.md"
));
pub(in crate::cli) const HERMES_CHECK_SETUP_SH: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/extras/hermes/clipboard-memory/scripts/check-setup.sh"
));

pub(in crate::cli) fn packaged_openclaw_skill() -> String {
    OPENCLAW_SKILL_MD.to_string()
}

pub(in crate::cli) fn packaged_openclaw_files() -> &'static [PackagedSkillFile] {
    &[
        PackagedSkillFile {
            relative_path: "SKILL.md",
            contents: OPENCLAW_SKILL_MD,
        },
        PackagedSkillFile {
            relative_path: "references/commands.md",
            contents: OPENCLAW_COMMANDS_REF,
        },
        PackagedSkillFile {
            relative_path: "references/troubleshooting.md",
            contents: OPENCLAW_TROUBLESHOOTING_REF,
        },
        PackagedSkillFile {
            relative_path: "references/json-schema.md",
            contents: OPENCLAW_JSON_SCHEMA_REF,
        },
        PackagedSkillFile {
            relative_path: "references/examples.md",
            contents: OPENCLAW_EXAMPLES_REF,
        },
        PackagedSkillFile {
            relative_path: "references/setup-check.md",
            contents: OPENCLAW_SETUP_CHECK_REF,
        },
        PackagedSkillFile {
            relative_path: "scripts/check-setup.sh",
            contents: OPENCLAW_CHECK_SETUP_SH,
        },
    ]
}

pub(in crate::cli) fn resolve_openclaw_skill_dir(
    dest: Option<&Path>,
    shared: bool,
) -> Result<PathBuf> {
    if let Some(dest) = dest {
        return expand_home_path(dest);
    }

    let base = if shared {
        match std::env::var_os("OPENCLAW_STATE_DIR") {
            Some(path) => expand_home_path(Path::new(&path))?.join("skills"),
            None => home_dir()?.join(OPENCLAW_SHARED_ROOT),
        }
    } else {
        resolve_openclaw_workspace_root()?.join("skills")
    };
    Ok(base.join(OPENCLAW_SKILL_NAME))
}

pub(in crate::cli) fn resolve_openclaw_workspace_root() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CLIPMEM_OPENCLAW_WORKSPACE") {
        return expand_home_path(Path::new(&path));
    }

    if let Some(openclaw_bin) = find_executable("openclaw") {
        let output = ProcessCommand::new(openclaw_bin)
            .args(["config", "get", "agents.defaults.workspace"])
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !value.is_empty() {
                    let value = serde_json::from_str::<String>(&value).unwrap_or(value);
                    return expand_home_path(Path::new(&value));
                }
            }
        }
    }

    Ok(home_dir()?.join(OPENCLAW_WORKSPACE_ROOT))
}

pub(in crate::cli) fn packaged_hermes_skill() -> String {
    HERMES_SKILL_MD.to_string()
}

pub(in crate::cli) fn packaged_hermes_files() -> &'static [PackagedSkillFile] {
    &[
        PackagedSkillFile {
            relative_path: "SKILL.md",
            contents: HERMES_SKILL_MD,
        },
        PackagedSkillFile {
            relative_path: "references/commands.md",
            contents: HERMES_COMMANDS_REF,
        },
        PackagedSkillFile {
            relative_path: "references/troubleshooting.md",
            contents: HERMES_TROUBLESHOOTING_REF,
        },
        PackagedSkillFile {
            relative_path: "references/json-schema.md",
            contents: HERMES_JSON_SCHEMA_REF,
        },
        PackagedSkillFile {
            relative_path: "references/examples.md",
            contents: HERMES_EXAMPLES_REF,
        },
        PackagedSkillFile {
            relative_path: "references/setup-check.md",
            contents: HERMES_SETUP_CHECK_REF,
        },
        PackagedSkillFile {
            relative_path: "scripts/check-setup.sh",
            contents: HERMES_CHECK_SETUP_SH,
        },
    ]
}

pub(in crate::cli) fn resolve_hermes_skill_dir(dest: Option<&Path>) -> Result<PathBuf> {
    if let Some(dest) = dest {
        return expand_home_path(dest);
    }

    let root = match std::env::var_os("HERMES_HOME") {
        Some(path) => expand_home_path(Path::new(&path))?.join("skills/productivity"),
        None => home_dir()?.join(HERMES_SKILL_ROOT),
    };
    Ok(root.join(HERMES_SKILL_NAME))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        packaged_hermes_files, packaged_hermes_skill, packaged_openclaw_files,
        packaged_openclaw_skill, resolve_hermes_skill_dir, resolve_openclaw_skill_dir,
        HERMES_CHECK_SETUP_SH, HERMES_COMMANDS_REF, HERMES_EXAMPLES_REF, HERMES_JSON_SCHEMA_REF,
        HERMES_SETUP_CHECK_REF, HERMES_SKILL_MD, HERMES_SKILL_NAME, HERMES_TROUBLESHOOTING_REF,
        OPENCLAW_CHECK_SETUP_SH, OPENCLAW_COMMANDS_REF, OPENCLAW_EXAMPLES_REF,
        OPENCLAW_JSON_SCHEMA_REF, OPENCLAW_SETUP_CHECK_REF, OPENCLAW_SKILL_MD, OPENCLAW_SKILL_NAME,
        OPENCLAW_TROUBLESHOOTING_REF,
    };

    #[test]
    fn packaged_skill_strings_return_embedded_skill_markdown() {
        assert_eq!(packaged_openclaw_skill(), OPENCLAW_SKILL_MD);
        assert_eq!(packaged_hermes_skill(), HERMES_SKILL_MD);
        assert!(packaged_openclaw_skill().contains(OPENCLAW_SKILL_NAME));
        assert!(packaged_hermes_skill().contains(HERMES_SKILL_NAME));
    }

    #[test]
    fn packaged_openclaw_manifest_lists_all_embedded_files() {
        let files = packaged_openclaw_files();
        let manifest = files
            .iter()
            .map(|file| (file.relative_path, file.contents))
            .collect::<Vec<_>>();

        assert_eq!(
            manifest,
            vec![
                ("SKILL.md", OPENCLAW_SKILL_MD),
                ("references/commands.md", OPENCLAW_COMMANDS_REF),
                (
                    "references/troubleshooting.md",
                    OPENCLAW_TROUBLESHOOTING_REF
                ),
                ("references/json-schema.md", OPENCLAW_JSON_SCHEMA_REF),
                ("references/examples.md", OPENCLAW_EXAMPLES_REF),
                ("references/setup-check.md", OPENCLAW_SETUP_CHECK_REF),
                ("scripts/check-setup.sh", OPENCLAW_CHECK_SETUP_SH),
            ]
        );
    }

    #[test]
    fn packaged_hermes_manifest_lists_all_embedded_files() {
        let files = packaged_hermes_files();
        let manifest = files
            .iter()
            .map(|file| (file.relative_path, file.contents))
            .collect::<Vec<_>>();

        assert_eq!(
            manifest,
            vec![
                ("SKILL.md", HERMES_SKILL_MD),
                ("references/commands.md", HERMES_COMMANDS_REF),
                ("references/troubleshooting.md", HERMES_TROUBLESHOOTING_REF),
                ("references/json-schema.md", HERMES_JSON_SCHEMA_REF),
                ("references/examples.md", HERMES_EXAMPLES_REF),
                ("references/setup-check.md", HERMES_SETUP_CHECK_REF),
                ("scripts/check-setup.sh", HERMES_CHECK_SETUP_SH),
            ]
        );
    }

    #[test]
    fn explicit_skill_destinations_bypass_default_resolution() {
        let openclaw_dest = Path::new("/tmp/custom-openclaw-skill");
        let hermes_dest = Path::new("/tmp/custom-hermes-skill");

        assert_eq!(
            resolve_openclaw_skill_dir(Some(openclaw_dest), false)
                .expect("explicit OpenClaw destination should resolve"),
            openclaw_dest
        );
        assert_eq!(
            resolve_openclaw_skill_dir(Some(openclaw_dest), true)
                .expect("explicit shared OpenClaw destination should resolve"),
            openclaw_dest
        );
        assert_eq!(
            resolve_hermes_skill_dir(Some(hermes_dest))
                .expect("explicit Hermes destination should resolve"),
            hermes_dest
        );
    }
}
