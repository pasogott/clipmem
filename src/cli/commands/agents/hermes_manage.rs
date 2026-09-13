use std::path::Path;

use anyhow::{anyhow, Result};

use crate::cli::schema::{HermesInstallSkillArgs, HermesUninstallSkillArgs};

use super::package::{packaged_hermes_files, resolve_hermes_skill_dir};
use super::support::{install_packaged_skill, uninstall_packaged_skill, validate_installed_skill};

pub(in crate::cli) fn hermes_install_skill(args: &HermesInstallSkillArgs) -> Result<()> {
    let target_dir = resolve_hermes_skill_dir(args.dest.as_deref())?;
    if target_dir.exists() {
        if args.force {
            validate_installed_skill(&target_dir, packaged_hermes_files())?;
        } else {
            return Err(anyhow!(
                "skill directory already exists at {} (pass --force to replace it)",
                target_dir.display()
            ));
        }
    }

    install_hermes_package(&target_dir)?;

    displayln!("Installed Hermes Agent skill into {}", target_dir.display());
    displayln!("Skill file: {}", target_dir.join("SKILL.md").display());
    displayln!("Restart Hermes or open a fresh session if the skill does not appear immediately.");
    Ok(())
}

pub(in crate::cli) fn hermes_uninstall_skill(args: &HermesUninstallSkillArgs) -> Result<()> {
    let target_dir = resolve_hermes_skill_dir(args.dest.as_deref())?;
    if !target_dir.exists() {
        displayln!("No installed skill found at {}", target_dir.display());
        return Ok(());
    }

    uninstall_packaged_skill(&target_dir, packaged_hermes_files())?;
    displayln!("Removed Hermes Agent skill from {}", target_dir.display());
    Ok(())
}

pub(in crate::cli) fn install_hermes_package(target_dir: &Path) -> Result<()> {
    install_packaged_skill(target_dir, packaged_hermes_files())
}

#[cfg(test)]
mod tests {
    use super::super::package::{
        packaged_hermes_files, resolve_hermes_skill_dir, HERMES_SKILL_NAME,
    };

    #[test]
    fn resolve_hermes_skill_dir_uses_explicit_destination() {
        let explicit = std::path::Path::new("/tmp/custom-hermes-skill");

        assert_eq!(
            resolve_hermes_skill_dir(Some(explicit)).expect("explicit path should resolve"),
            explicit
        );
    }

    #[test]
    fn packaged_hermes_files_include_skill_and_setup_script() {
        let files = packaged_hermes_files();

        assert!(files
            .iter()
            .any(|file| file.relative_path == "SKILL.md"
                && file.contents.contains(HERMES_SKILL_NAME)));
        assert!(files
            .iter()
            .any(|file| file.relative_path == "scripts/check-setup.sh"
                && file.contents.starts_with("#!/")));
    }
}
