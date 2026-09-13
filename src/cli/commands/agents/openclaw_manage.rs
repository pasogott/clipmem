use std::path::Path;

use anyhow::{anyhow, Result};

use crate::cli::schema::{OpenClawInstallSkillArgs, OpenClawUninstallSkillArgs};

use super::package::{packaged_openclaw_files, resolve_openclaw_skill_dir};
use super::support::{install_packaged_skill, uninstall_packaged_skill, validate_installed_skill};

pub(in crate::cli) fn openclaw_install_skill(args: &OpenClawInstallSkillArgs) -> Result<()> {
    let target_dir = resolve_openclaw_skill_dir(args.dest.as_deref(), args.shared)?;
    if target_dir.exists() {
        if args.force {
            validate_installed_skill(&target_dir, packaged_openclaw_files())?;
        } else {
            return Err(anyhow!(
                "skill directory already exists at {} (pass --force to replace it)",
                target_dir.display()
            ));
        }
    }

    install_openclaw_package(&target_dir)?;

    displayln!("Installed OpenClaw skill into {}", target_dir.display());
    displayln!("Skill file: {}", target_dir.join("SKILL.md").display());
    displayln!(
        "Reload OpenClaw skills or restart OpenClaw if the skill does not appear immediately."
    );
    Ok(())
}

pub(in crate::cli) fn openclaw_uninstall_skill(args: &OpenClawUninstallSkillArgs) -> Result<()> {
    let target_dir = resolve_openclaw_skill_dir(args.dest.as_deref(), args.shared)?;
    if !target_dir.exists() {
        displayln!("No installed skill found at {}", target_dir.display());
        return Ok(());
    }

    uninstall_packaged_skill(&target_dir, packaged_openclaw_files())?;
    displayln!("Removed OpenClaw skill from {}", target_dir.display());
    Ok(())
}

pub(in crate::cli) fn install_openclaw_package(target_dir: &Path) -> Result<()> {
    install_packaged_skill(target_dir, packaged_openclaw_files())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::super::package::{
        packaged_openclaw_files, resolve_openclaw_skill_dir, OPENCLAW_SKILL_NAME,
    };
    use super::super::support::{install_packaged_skill, PackagedSkillFile};

    fn temp_skill_dir(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("clipmem-{name}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn resolve_openclaw_skill_dir_uses_explicit_destination() {
        let explicit = std::path::Path::new("/tmp/custom-openclaw-skill");

        assert_eq!(
            resolve_openclaw_skill_dir(Some(explicit), false)
                .expect("explicit path should resolve"),
            explicit
        );
    }

    #[test]
    fn packaged_openclaw_files_include_skill_and_setup_script() {
        let files = packaged_openclaw_files();

        assert!(files
            .iter()
            .any(|file| file.relative_path == "SKILL.md"
                && file.contents.contains(OPENCLAW_SKILL_NAME)));
        assert!(files
            .iter()
            .any(|file| file.relative_path == "scripts/check-setup.sh"
                && file.contents.starts_with("#!/")));
    }

    #[test]
    fn install_packaged_skill_writes_nested_files_and_marks_scripts_executable() {
        let target_dir = temp_skill_dir("packaged-skill");
        let files = [
            PackagedSkillFile {
                relative_path: "SKILL.md",
                contents: "name: test\n",
            },
            PackagedSkillFile {
                relative_path: "scripts/check-setup.sh",
                contents: "#!/bin/sh\nexit 0\n",
            },
        ];

        install_packaged_skill(&target_dir, &files).expect("package should install");

        let skill_path = target_dir.join("SKILL.md");
        let script_path = target_dir.join("scripts/check-setup.sh");
        assert_eq!(
            std::fs::read_to_string(&skill_path).expect("skill file should be readable"),
            "name: test\n"
        );
        assert!(script_path.is_file());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&script_path)
                .expect("script metadata should be readable")
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0);
        }

        let _ = std::fs::remove_dir_all(target_dir);
    }
}
