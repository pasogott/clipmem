use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{anyhow, Context, Result};

use super::doctor::render_agent_doctor_report;
use super::doctor::{AgentDoctorCheck, AgentDoctorStatus, HermesDoctorReport};
use super::package::{packaged_hermes_files, resolve_hermes_skill_dir, HERMES_SKILL_NAME};
use super::support::{
    binary_check, find_executable, frontmatter_value, parse_skill_frontmatter,
    required_frontmatter_value, validate_packaged_skill_file, validate_required_skill_references,
};
use crate::cli::schema::HermesDoctorArgs;

#[cfg(test)]
use super::package::packaged_hermes_skill;

pub(in crate::cli) fn hermes_doctor(args: &HermesDoctorArgs) -> Result<()> {
    let report = build_hermes_doctor_report(args)?;
    display!("{}", render_hermes_doctor_report(&report));

    if report
        .checks
        .iter()
        .any(|check| matches!(check.status, AgentDoctorStatus::Fail))
    {
        Err(anyhow!("Hermes Agent integration checks failed"))
    } else {
        Ok(())
    }
}

pub(in crate::cli) fn build_hermes_doctor_report(
    args: &HermesDoctorArgs,
) -> Result<HermesDoctorReport> {
    let target_dir = resolve_hermes_skill_dir(args.dest.as_deref())?;
    let mut checks = Vec::new();

    let clipmem_path = find_executable("clipmem");
    checks.push(binary_check(
        "Host clipmem on PATH",
        clipmem_path.as_ref(),
        &[
            "Install clipmem with `brew install tristanmanchester/tap/clipmem`.",
            "Or install it with `cargo install clipmem`.",
            "Then run `clipmem setup` to initialize the database and start background capture.",
        ],
    ));

    let hermes_path = find_executable("hermes");
    checks.push(hermes_binary_check(hermes_path.as_ref()));

    if target_dir.exists() {
        checks.push(AgentDoctorCheck {
            status: AgentDoctorStatus::Ok,
            label: "Installed skill directory".to_string(),
            detail: format!("Found {}", target_dir.display()),
            next_steps: Vec::new(),
        });
    } else {
        checks.push(AgentDoctorCheck {
            status: AgentDoctorStatus::Fail,
            label: "Installed skill directory".to_string(),
            detail: format!("Missing {}", target_dir.display()),
            next_steps: vec![
                "Install the skill with `clipmem agents hermes install-skill`.".to_string(),
            ],
        });
    }

    let skill_path = target_dir.join("SKILL.md");
    if skill_path.is_file() {
        match validate_hermes_skill_dir(&target_dir) {
            Ok(()) => checks.push(AgentDoctorCheck {
                status: AgentDoctorStatus::Ok,
                label: "SKILL.md metadata".to_string(),
                detail: format!(
                    "Validated {} and referenced package files under {}",
                    skill_path.display(),
                    target_dir.display()
                ),
                next_steps: Vec::new(),
            }),
            Err(error) => checks.push(AgentDoctorCheck {
                status: AgentDoctorStatus::Fail,
                label: "SKILL.md metadata".to_string(),
                detail: error.to_string(),
                next_steps: vec![
                    "Reinstall the packaged skill with `clipmem agents hermes install-skill --force`.".to_string(),
                    "Use `clipmem agents hermes print-skill` to inspect the packaged content.".to_string(),
                ],
            }),
        }
    } else {
        checks.push(AgentDoctorCheck {
            status: AgentDoctorStatus::Fail,
            label: "SKILL.md file".to_string(),
            detail: format!("Missing {}", skill_path.display()),
            next_steps: vec![
                "Install the packaged skill with `clipmem agents hermes install-skill`."
                    .to_string(),
            ],
        });
    }

    checks.push(hermes_skill_discovery_check(hermes_path.as_ref()));

    Ok(HermesDoctorReport { target_dir, checks })
}

pub(in crate::cli) fn hermes_binary_check(path: Option<&PathBuf>) -> AgentDoctorCheck {
    match path {
        Some(path) => AgentDoctorCheck {
            status: AgentDoctorStatus::Ok,
            label: "Host hermes on PATH".to_string(),
            detail: format!("Found {}", path.display()),
            next_steps: Vec::new(),
        },
        None => AgentDoctorCheck {
            status: AgentDoctorStatus::Warn,
            label: "Host hermes on PATH".to_string(),
            detail: "hermes is not available; skipping live Hermes skill discovery checks."
                .to_string(),
            next_steps: vec![
                "Install Hermes Agent if you want to verify live skill discovery.".to_string(),
            ],
        },
    }
}

pub(in crate::cli) fn hermes_skill_discovery_check(
    hermes_path: Option<&PathBuf>,
) -> AgentDoctorCheck {
    let Some(hermes_path) = hermes_path else {
        return AgentDoctorCheck {
            status: AgentDoctorStatus::Warn,
            label: "Hermes skill discovery".to_string(),
            detail: "Skipped live discovery because `hermes` is not available.".to_string(),
            next_steps: vec![
                "Install Hermes Agent, then run `clipmem agents hermes doctor` again.".to_string(),
            ],
        };
    };

    let output = ProcessCommand::new(hermes_path)
        .args(["skills", "list"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if stdout.contains(HERMES_SKILL_NAME) {
                AgentDoctorCheck {
                    status: AgentDoctorStatus::Ok,
                    label: "Hermes skill discovery".to_string(),
                    detail: format!("`hermes skills list` includes {HERMES_SKILL_NAME}."),
                    next_steps: Vec::new(),
                }
            } else {
                AgentDoctorCheck {
                    status: AgentDoctorStatus::Warn,
                    label: "Hermes skill discovery".to_string(),
                    detail: format!(
                        "`hermes skills list` did not show {HERMES_SKILL_NAME}."
                    ),
                    next_steps: vec![
                        "Restart Hermes or open a fresh session so it rescans skills.".to_string(),
                        "If you installed to a custom path, add that directory under `skills.external_dirs` in ~/.hermes/config.yaml.".to_string(),
                    ],
                }
            }
        }
        Ok(output) => AgentDoctorCheck {
            status: AgentDoctorStatus::Warn,
            label: "Hermes skill discovery".to_string(),
            detail: format!(
                "Could not list Hermes skills (`hermes skills list` exited with {}).",
                output.status
            ),
            next_steps: vec![
                "Run `hermes skills list` manually and verify whether the skill appears."
                    .to_string(),
            ],
        },
        Err(error) => AgentDoctorCheck {
            status: AgentDoctorStatus::Warn,
            label: "Hermes skill discovery".to_string(),
            detail: format!("Could not list Hermes skills: {error}"),
            next_steps: vec![
                "Run `hermes skills list` manually and verify whether the skill appears."
                    .to_string(),
            ],
        },
    }
}

pub(in crate::cli) fn validate_hermes_skill_file(path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    validate_hermes_skill_content(&content)
}

pub(in crate::cli) fn validate_hermes_skill_dir(path: &Path) -> Result<()> {
    let skill_path = path.join("SKILL.md");
    validate_hermes_skill_file(&skill_path)?;

    for file in packaged_hermes_files() {
        let installed_path = path.join(file.relative_path);
        validate_packaged_skill_file(&installed_path, file.relative_path)?;
    }

    Ok(())
}

pub(in crate::cli) fn validate_hermes_skill_content(content: &str) -> Result<()> {
    let frontmatter_lines = parse_skill_frontmatter(content)?;

    let name = frontmatter_value(&frontmatter_lines, "name").filter(|value| !value.is_empty());
    if name != Some(HERMES_SKILL_NAME) {
        return Err(anyhow!(
            "skill frontmatter must include `name: {HERMES_SKILL_NAME}`"
        ));
    }

    required_frontmatter_value(&frontmatter_lines, "description")?;
    required_frontmatter_value(&frontmatter_lines, "version")?;

    let platforms = frontmatter_value(&frontmatter_lines, "platforms")
        .ok_or_else(|| anyhow!("skill frontmatter must include `platforms`"))?;
    if !inline_yaml_list_contains(platforms, "macos") {
        return Err(anyhow!(
            "skill frontmatter `platforms` must include `macos`"
        ));
    }

    if !frontmatter_lines
        .iter()
        .any(|line| line.trim() == "metadata:")
    {
        return Err(anyhow!("skill frontmatter must include `metadata`"));
    }
    if !frontmatter_lines
        .iter()
        .any(|line| line.trim() == "hermes:")
    {
        return Err(anyhow!("metadata must include `hermes`"));
    }
    let category = frontmatter_lines
        .iter()
        .find_map(|line| line.trim().strip_prefix("category:").map(str::trim));
    if category != Some("productivity") {
        return Err(anyhow!("metadata.hermes.category must be `productivity`"));
    }

    let tags = frontmatter_lines
        .iter()
        .find_map(|line| line.trim().strip_prefix("tags:").map(str::trim))
        .ok_or_else(|| anyhow!("metadata.hermes.tags must be present"))?;
    for tag in [
        "clipboard",
        "memory",
        "macos",
        "local-first",
        "cli",
        "retrieval",
    ] {
        if !inline_yaml_list_contains(tags, tag) {
            return Err(anyhow!("metadata.hermes.tags must include `{tag}`"));
        }
    }

    validate_required_skill_references(
        content,
        &["references/commands.md", "references/troubleshooting.md"],
    )?;

    Ok(())
}

fn inline_yaml_list_contains(value: &str, expected: &str) -> bool {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|item| item.trim().trim_matches(['"', '\'']))
        .any(|item| item == expected)
}

pub(in crate::cli) fn render_hermes_doctor_report(report: &HermesDoctorReport) -> String {
    render_agent_doctor_report(
        "Hermes Agent integration target",
        &report.target_dir,
        &report.checks,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::support::referenced_markdown_files;
    use super::{
        inline_yaml_list_contains, packaged_hermes_files, packaged_hermes_skill,
        validate_hermes_skill_content,
    };

    #[test]
    fn packaged_hermes_skill_includes_required_metadata() {
        let content = packaged_hermes_skill();

        validate_hermes_skill_content(&content).expect("packaged Hermes skill should validate");
        assert!(content.contains("platforms: [macos]"));
        assert!(content.contains("metadata:"));
        assert!(content.contains("hermes:"));
        assert!(content.contains("category: productivity"));
        assert!(content.contains("license:"));
        assert!(content.contains("clipmem recall"));
        assert!(content.contains("clipmem timeline"));
        assert!(content.contains("clipmem export"));
        assert!(content.contains("references/commands.md"));
        assert!(content.contains("references/json-schema.md"));
        assert!(content.contains("references/examples.md"));
        assert!(content.contains("references/setup-check.md"));
        assert!(content.contains("references/troubleshooting.md"));
        assert!(content.contains("scripts/check-setup.sh"));
        assert!(content.contains("what was that command I copied?"));
        assert!(content.contains("toon"));
        assert!(content.contains("--cursor"));
        assert!(content.contains("schema_version"));
    }

    #[test]
    fn packaged_hermes_package_embeds_reference_files() {
        let files = packaged_hermes_files();
        assert_eq!(files.len(), 7);
        assert!(files.iter().any(|file| file.relative_path == "SKILL.md"));
        assert!(files
            .iter()
            .any(|file| file.relative_path == "references/commands.md"));
        assert!(files
            .iter()
            .any(|file| file.relative_path == "references/troubleshooting.md"));
        assert!(files
            .iter()
            .any(|file| file.relative_path == "references/json-schema.md"));
        assert!(files
            .iter()
            .any(|file| file.relative_path == "references/examples.md"));
        assert!(files
            .iter()
            .any(|file| file.relative_path == "references/setup-check.md"));
        assert!(files
            .iter()
            .any(|file| file.relative_path == "scripts/check-setup.sh"));
    }

    #[test]
    fn packaged_hermes_skill_references_relative_markdown_files() {
        let references = referenced_markdown_files(&packaged_hermes_skill());
        assert!(references
            .iter()
            .any(|path| path == &PathBuf::from("references/commands.md")));
        assert!(references
            .iter()
            .any(|path| path == &PathBuf::from("references/troubleshooting.md")));
        assert!(references
            .iter()
            .any(|path| path == &PathBuf::from("references/json-schema.md")));
        assert!(references
            .iter()
            .any(|path| path == &PathBuf::from("references/examples.md")));
        assert!(references
            .iter()
            .any(|path| path == &PathBuf::from("references/setup-check.md")));
    }

    #[test]
    fn hermes_skill_validation_rejects_missing_required_frontmatter() {
        let valid = packaged_hermes_skill();

        let missing_name = valid.replacen("name: clipboard-memory\n", "", 1);
        assert!(validate_hermes_skill_content(&missing_name)
            .unwrap_err()
            .to_string()
            .contains("name: clipboard-memory"));

        let wrong_name = valid.replacen("name: clipboard-memory", "name: other", 1);
        assert!(validate_hermes_skill_content(&wrong_name)
            .unwrap_err()
            .to_string()
            .contains("name: clipboard-memory"));

        let missing_description = valid.replacen("description:", "summary:", 1);
        assert!(validate_hermes_skill_content(&missing_description)
            .unwrap_err()
            .to_string()
            .contains("description"));

        let missing_platforms = valid.replacen("platforms: [macos]\n", "", 1);
        assert!(validate_hermes_skill_content(&missing_platforms)
            .unwrap_err()
            .to_string()
            .contains("platforms"));

        let wrong_platforms = valid.replacen("platforms: [macos]", "platforms: [macos-intel]", 1);
        assert!(validate_hermes_skill_content(&wrong_platforms)
            .unwrap_err()
            .to_string()
            .contains("platforms"));

        let missing_metadata = valid.replacen("  hermes:", "  runtime:", 1);
        assert!(validate_hermes_skill_content(&missing_metadata)
            .unwrap_err()
            .to_string()
            .contains("metadata must include `hermes`"));

        let missing_references = valid.replace("references/commands.md", "references/missing.md");
        assert!(validate_hermes_skill_content(&missing_references)
            .unwrap_err()
            .to_string()
            .contains("references/commands.md"));
    }

    #[test]
    fn hermes_skill_validation_requires_exact_inline_list_items() {
        assert!(inline_yaml_list_contains(
            "[clipboard, 'memory', \"macos\"]",
            "memory"
        ));
        assert!(!inline_yaml_list_contains(
            "[clipboard-memory]",
            "clipboard"
        ));

        let valid = packaged_hermes_skill();
        let wrong_tags = valid.replacen(
            "tags: [clipboard, memory, macos, local-first, cli, retrieval]",
            "tags: [clipboard-memory, macos, local-first, cli, retrieval]",
            1,
        );
        assert!(validate_hermes_skill_content(&wrong_tags)
            .unwrap_err()
            .to_string()
            .contains("metadata.hermes.tags must include `clipboard`"));
    }
}
