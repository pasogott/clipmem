use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{anyhow, Context, Result};

use super::doctor::render_agent_doctor_report;
use super::doctor::{AgentDoctorCheck, AgentDoctorStatus, OpenClawDoctorReport};
#[cfg(test)]
use super::package::packaged_openclaw_skill;
use super::package::{
    packaged_openclaw_files, resolve_openclaw_skill_dir, resolve_openclaw_workspace_root,
    OPENCLAW_SKILL_NAME,
};
use super::support::{
    binary_check, find_executable, frontmatter_value, parse_skill_frontmatter,
    required_frontmatter_value, validate_packaged_skill_file, validate_required_skill_references,
};
use crate::cli::schema::OpenClawDoctorArgs;

pub(in crate::cli) fn openclaw_doctor(args: &OpenClawDoctorArgs) -> Result<()> {
    let report = build_openclaw_doctor_report(args)?;
    display!("{}", render_openclaw_doctor_report(&report));

    if report
        .checks
        .iter()
        .any(|check| matches!(check.status, AgentDoctorStatus::Fail))
    {
        Err(anyhow!("OpenClaw integration checks failed"))
    } else {
        Ok(())
    }
}

pub(in crate::cli) fn build_openclaw_doctor_report(
    args: &OpenClawDoctorArgs,
) -> Result<OpenClawDoctorReport> {
    let target_dir = resolve_openclaw_skill_dir(args.dest.as_deref(), args.shared)?;
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

    let openclaw_path = find_executable("openclaw");
    checks.push(binary_check(
        "Host openclaw on PATH",
        openclaw_path.as_ref(),
        &["Install OpenClaw and ensure `openclaw` is available on the host PATH."],
    ));

    let workspace_root = resolve_openclaw_workspace_root()?;
    checks.push(AgentDoctorCheck {
        status: AgentDoctorStatus::Ok,
        label: "OpenClaw workspace root".to_string(),
        detail: format!("Resolved workspace root: {}", workspace_root.display()),
        next_steps: Vec::new(),
    });

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
                "Install the skill with `clipmem agents openclaw install-skill`.".to_string(),
                "Use `--shared` if you intended a shared install under ~/.openclaw/skills."
                    .to_string(),
            ],
        });
    }

    let skill_path = target_dir.join("SKILL.md");
    if skill_path.is_file() {
        match validate_openclaw_skill_dir(&target_dir) {
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
                    "Reinstall the packaged skill with `clipmem agents openclaw install-skill --force`.".to_string(),
                    "Use `clipmem agents openclaw print-skill` to inspect the packaged content.".to_string(),
                ],
            }),
        }
    } else {
        checks.push(AgentDoctorCheck {
            status: AgentDoctorStatus::Fail,
            label: "SKILL.md file".to_string(),
            detail: format!("Missing {}", skill_path.display()),
            next_steps: vec![
                "Install the packaged skill with `clipmem agents openclaw install-skill`."
                    .to_string(),
            ],
        });
    }

    checks.push(openclaw_sandbox_check(openclaw_path.as_ref()));

    Ok(OpenClawDoctorReport { target_dir, checks })
}

pub(in crate::cli) fn openclaw_sandbox_check(openclaw_path: Option<&PathBuf>) -> AgentDoctorCheck {
    let Some(openclaw_path) = openclaw_path else {
        return AgentDoctorCheck {
            status: AgentDoctorStatus::Warn,
            label: "Sandbox visibility".to_string(),
            detail: "Skipped sandbox checks because `openclaw` is not available.".to_string(),
            next_steps: vec![
                "Install OpenClaw first if you want sandbox-specific guidance.".to_string(),
            ],
        };
    };

    let output = ProcessCommand::new(openclaw_path)
        .args(["sandbox", "explain", "--json"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let state = serde_json::from_slice::<serde_json::Value>(&output.stdout).ok();
            let mode = state.as_ref().and_then(|value| value.pointer("/sandbox/mode")).and_then(serde_json::Value::as_str);
            if mode == Some("off") {
                AgentDoctorCheck {
                    status: AgentDoctorStatus::Ok,
                    label: "Sandbox visibility".to_string(),
                    detail: "OpenClaw reports sandbox.mode=off; host PATH should be sufficient.".to_string(),
                    next_steps: Vec::new(),
                }
            } else {
                AgentDoctorCheck {
                    status: AgentDoctorStatus::Warn,
                    label: "Sandbox visibility".to_string(),
                    detail: "OpenClaw sandboxing is active or its state could not be verified; `clipmem` may need to be available inside sandbox containers as well as on the host.".to_string(),
                    next_steps: vec![
                        "Ensure `clipmem` is installed in a path visible inside the sandbox image, not only your host shell.".to_string(),
                        "If you installed clipmem after sandbox creation, recreate containers with `openclaw sandbox recreate --all`.".to_string(),
                        "If commands still fail in the sandbox, use `openclaw sandbox explain` and verify the container PATH.".to_string(),
                    ],
                }
            }
        }
        Ok(output) => AgentDoctorCheck {
            status: AgentDoctorStatus::Warn,
            label: "Sandbox visibility".to_string(),
            detail: format!(
                "Could not inspect sandbox state (`openclaw sandbox explain` exited with {}).",
                output.status
            ),
            next_steps: vec![
                "Run `openclaw sandbox explain` manually and verify whether `clipmem` is present in the sandbox environment.".to_string(),
            ],
        },
        Err(error) => AgentDoctorCheck {
            status: AgentDoctorStatus::Warn,
            label: "Sandbox visibility".to_string(),
            detail: format!("Could not inspect sandbox state: {error}"),
            next_steps: vec![
                "Run `openclaw sandbox explain` manually and verify whether `clipmem` is present in the sandbox environment.".to_string(),
            ],
        },
    }
}

pub(in crate::cli) fn validate_openclaw_skill_file(path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    validate_openclaw_skill_content(&content)
}

pub(in crate::cli) fn validate_openclaw_skill_dir(path: &Path) -> Result<()> {
    let skill_path = path.join("SKILL.md");
    validate_openclaw_skill_file(&skill_path)?;

    for file in packaged_openclaw_files() {
        let installed_path = path.join(file.relative_path);
        validate_packaged_skill_file(&installed_path, file.relative_path)?;
    }

    Ok(())
}

pub(in crate::cli) fn validate_openclaw_skill_content(content: &str) -> Result<()> {
    let frontmatter_lines = parse_skill_frontmatter(content)?;

    let name = frontmatter_value(&frontmatter_lines, "name").filter(|value| !value.is_empty());
    if name != Some(OPENCLAW_SKILL_NAME) {
        return Err(anyhow!(
            "skill frontmatter must include `name: {OPENCLAW_SKILL_NAME}`"
        ));
    }

    required_frontmatter_value(&frontmatter_lines, "description")?;

    let metadata_line = required_frontmatter_value(&frontmatter_lines, "metadata")?;
    let metadata: serde_json::Value = serde_json::from_str(metadata_line)
        .map_err(|error| anyhow!("invalid metadata JSON: {error}"))?;
    let openclaw = metadata
        .get("openclaw")
        .ok_or_else(|| anyhow!("metadata must include `openclaw`"))?;
    let bins = openclaw
        .pointer("/requires/bins")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("metadata.openclaw.requires.bins must be an array"))?;
    if !bins.iter().any(|value| value.as_str() == Some("clipmem")) {
        return Err(anyhow!(
            "metadata.openclaw.requires.bins must contain `clipmem`"
        ));
    }

    let install = openclaw
        .get("install")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("metadata.openclaw.install must be an array"))?;
    validate_openclaw_install_entries(install)?;

    validate_required_skill_references(
        content,
        &["references/commands.md", "references/troubleshooting.md"],
    )?;

    Ok(())
}

pub(in crate::cli) fn validate_openclaw_install_entries(
    entries: &[serde_json::Value],
) -> Result<()> {
    if entries.is_empty() {
        return Err(anyhow!(
            "metadata.openclaw.install must contain at least one entry"
        ));
    }

    for entry in entries {
        let object = entry
            .as_object()
            .ok_or_else(|| anyhow!("metadata.openclaw.install entries must be objects"))?;
        for key in ["id", "kind", "label", "bins"] {
            if !object.contains_key(key) {
                return Err(anyhow!(
                    "metadata.openclaw.install entry is missing `{key}`"
                ));
            }
        }
        let bins = object
            .get("bins")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow!("metadata.openclaw.install entry `bins` must be an array"))?;
        if bins.is_empty() || !bins.iter().any(|value| value.as_str() == Some("clipmem")) {
            return Err(anyhow!(
                "metadata.openclaw.install entry `bins` must contain `clipmem`"
            ));
        }
    }

    Ok(())
}

pub(in crate::cli) fn render_openclaw_doctor_report(report: &OpenClawDoctorReport) -> String {
    render_agent_doctor_report(
        "OpenClaw integration target",
        &report.target_dir,
        &report.checks,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::support::referenced_markdown_files;
    use super::{
        packaged_openclaw_files, packaged_openclaw_skill, validate_openclaw_skill_content,
    };

    #[test]
    fn packaged_openclaw_skill_includes_required_metadata() {
        let content = packaged_openclaw_skill();

        validate_openclaw_skill_content(&content).expect("packaged skill should validate");
        assert!(content.contains("\"openclaw\""));
        assert!(content.contains("\"requires\":{\"bins\":[\"clipmem\"]}"));
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
    fn packaged_openclaw_package_embeds_reference_files() {
        let files = packaged_openclaw_files();
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
    fn packaged_openclaw_skill_references_relative_markdown_files() {
        let references = referenced_markdown_files(&packaged_openclaw_skill());
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
}

#[cfg(test)]
mod profile_tests {
    use super::super::support::referenced_markdown_files;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "profiling harness for agent skill reference extraction"]
    fn profile_referenced_markdown_file_extraction() {
        let content = large_skill_markdown_with_repeated_references(25_000);

        let before = median_duration(11, 5, || {
            referenced_markdown_files_linear_dedupe_for_profile(&content).len()
        });
        let after = median_duration(11, 5, || referenced_markdown_files(&content).len());

        eprintln!(
            "referenced_markdown_files_linear_before={before:?} referenced_markdown_files_hash_after={after:?}"
        );
    }

    fn median_duration(
        runs: usize,
        expected_count: usize,
        mut f: impl FnMut() -> usize,
    ) -> Duration {
        let mut samples = Vec::with_capacity(runs);
        for _ in 0..runs {
            let started = Instant::now();
            let count = f();
            assert_eq!(count, expected_count);
            samples.push(started.elapsed());
        }
        samples.sort();
        samples[samples.len() / 2]
    }

    fn large_skill_markdown_with_repeated_references(reference_count: usize) -> String {
        let references = [
            "references/commands.md",
            "references/troubleshooting.md",
            "references/json-schema.md",
            "references/examples.md",
            "references/setup-check.md",
        ];
        let mut out = String::with_capacity(reference_count * 72);
        out.push_str("---\nname: clipboard-memory\n---\n");
        for index in 0..reference_count {
            let reference = references[index % references.len()];
            out.push_str("- See [reference](");
            out.push_str(reference);
            out.push_str(") for command details.\n");
        }
        out
    }

    fn referenced_markdown_files_linear_dedupe_for_profile(content: &str) -> Vec<PathBuf> {
        let mut references = Vec::new();

        for line in content.lines() {
            let mut remainder = line;
            while let Some(start) = remainder.find("(references/") {
                let after_start = &remainder[start + 1..];
                let Some(end) = after_start.find(')') else {
                    break;
                };
                let candidate = &after_start[..end];
                if candidate.ends_with(".md") {
                    let path = PathBuf::from(candidate);
                    if !references.iter().any(|existing| existing == &path) {
                        references.push(path);
                    }
                }
                remainder = &after_start[end + 1..];
            }
        }

        references
    }
}
