use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use super::doctor::{AgentDoctorCheck, AgentDoctorStatus};

#[derive(Debug, Clone, Copy)]
pub(in crate::cli) struct PackagedSkillFile {
    pub(in crate::cli) relative_path: &'static str,
    pub(in crate::cli) contents: &'static str,
}

pub(in crate::cli) fn install_packaged_skill(
    target_dir: &Path,
    files: &[PackagedSkillFile],
) -> Result<()> {
    validate_skill_paths(target_dir, files)?;
    std::fs::create_dir_all(target_dir)
        .with_context(|| format!("failed to create {}", target_dir.display()))?;

    for file in files {
        let path = target_dir.join(file.relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(&path, file.contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
        if file.relative_path.ends_with(".sh") {
            set_executable(&path)
                .with_context(|| format!("failed to mark {} executable", path.display()))?;
        }
    }

    Ok(())
}

// Never follow a link while replacing or removing a user-selected installation.
fn validate_skill_paths(target: &Path, files: &[PackagedSkillFile]) -> Result<()> {
    for path in std::iter::once(target.to_path_buf())
        .chain(files.iter().map(|file| target.join(file.relative_path)))
    {
        for ancestor in path.ancestors().take_while(|path| path.starts_with(target)) {
            match std::fs::symlink_metadata(ancestor) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(anyhow!(
                        "refusing skill path containing a symlink: {}",
                        ancestor.display()
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to inspect {}", ancestor.display()))
                }
            }
        }
    }
    Ok(())
}

pub(in crate::cli) fn validate_installed_skill(
    target: &Path,
    files: &[PackagedSkillFile],
) -> Result<()> {
    validate_skill_paths(target, files)?;
    let content = std::fs::read_to_string(target.join("SKILL.md")).with_context(|| {
        format!(
            "refusing to modify unrecognized skill directory {}",
            target.display()
        )
    })?;
    let frontmatter = parse_skill_frontmatter(&content)?;
    if frontmatter_value(&frontmatter, "name") != Some("clipboard-memory") {
        return Err(anyhow!(
            "refusing to modify a directory that is not the clipboard-memory skill: {}",
            target.display()
        ));
    }
    Ok(())
}

pub(in crate::cli) fn uninstall_packaged_skill(
    target: &Path,
    files: &[PackagedSkillFile],
) -> Result<()> {
    validate_installed_skill(target, files)?;
    for file in files {
        let path = target.join(file.relative_path);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("failed to remove {}", path.display()))
            }
        }
    }
    let mut directories: Vec<_> = files
        .iter()
        .flat_map(|file| {
            let path = target.join(file.relative_path);
            path.ancestors()
                .skip(1)
                .take_while(|path| path.starts_with(target))
                .map(Path::to_path_buf)
                .collect::<Vec<_>>()
        })
        .collect();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    directories.dedup();
    for directory in directories {
        match std::fs::remove_dir(&directory) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to remove {}", directory.display()))
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(in crate::cli) fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
pub(in crate::cli) fn set_executable(_path: &Path) -> Result<()> {
    // On non-Unix targets there is no concept of the executable bit to set;
    // scripts are invoked via their interpreter explicitly.
    Ok(())
}

pub(in crate::cli) fn expand_home_path(path: &Path) -> Result<PathBuf> {
    if path == Path::new("~") {
        return home_dir();
    }
    if let Ok(suffix) = path.strip_prefix("~") {
        return Ok(home_dir()?.join(suffix));
    }
    Ok(path.to_path_buf())
}

pub(in crate::cli) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set"))
}

pub(in crate::cli) fn find_executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

pub(in crate::cli) fn binary_check(
    label: &str,
    path: Option<&PathBuf>,
    next_steps: &[&str],
) -> AgentDoctorCheck {
    match path {
        Some(path) => AgentDoctorCheck {
            status: AgentDoctorStatus::Ok,
            label: label.to_string(),
            detail: format!("Found {}", path.display()),
            next_steps: Vec::new(),
        },
        None => AgentDoctorCheck {
            status: AgentDoctorStatus::Fail,
            label: label.to_string(),
            detail: format!("{label} is missing"),
            next_steps: next_steps.iter().map(|s| s.to_string()).collect(),
        },
    }
}

pub(in crate::cli) fn validate_packaged_skill_file(path: &Path, relative_path: &str) -> Result<()> {
    if !path.is_file() {
        return Err(anyhow!("packaged file is missing: {}", path.display()));
    }

    if relative_path.ends_with(".sh") {
        ensure_executable(path)
            .with_context(|| format!("packaged script is not executable: {}", path.display()))?;
    }

    Ok(())
}

#[cfg(unix)]
pub(in crate::cli) fn ensure_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
    if mode & 0o111 == 0 {
        return Err(anyhow!("mode {mode:o} does not include any execute bit"));
    }

    Ok(())
}

#[cfg(not(unix))]
pub(in crate::cli) fn ensure_executable(_path: &Path) -> Result<()> {
    Ok(())
}

pub(in crate::cli) fn referenced_markdown_files(content: &str) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut references = Vec::new();

    for line in content.lines() {
        let mut remainder = line;
        while let Some(start) = remainder.find("(references/") {
            let after = &remainder[start + 1..];
            let Some(end) = after.find(')') else {
                break;
            };
            let candidate = &after[..end];
            if candidate.ends_with(".md") && seen.insert(candidate.to_string()) {
                references.push(PathBuf::from(candidate));
            }
            remainder = &after[end + 1..];
        }
    }

    references
}

pub(in crate::cli) fn parse_skill_frontmatter(content: &str) -> Result<Vec<&str>> {
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return Err(anyhow!("skill frontmatter must start with `---`"));
    }

    let mut frontmatter_lines = Vec::new();
    for line in lines {
        if line == "---" {
            return Ok(frontmatter_lines);
        }
        frontmatter_lines.push(line);
    }

    Err(anyhow!("skill frontmatter is missing the closing `---`"))
}

pub(in crate::cli) fn frontmatter_value<'a>(lines: &'a [&str], key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    lines
        .iter()
        .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
}

pub(in crate::cli) fn required_frontmatter_value<'a>(
    lines: &'a [&str],
    key: &str,
) -> Result<&'a str> {
    frontmatter_value(lines, key)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("skill frontmatter must include `{key}`"))
}

pub(in crate::cli) fn validate_required_skill_references(
    content: &str,
    required_references: &[&str],
) -> Result<()> {
    let references = referenced_markdown_files(content);
    for required in required_references {
        if !references
            .iter()
            .any(|reference| reference == Path::new(required))
        {
            return Err(anyhow!("skill content must reference `{required}`"));
        }
    }
    Ok(())
}
