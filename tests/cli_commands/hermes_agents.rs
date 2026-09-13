use super::*;

#[test]
fn agents_hermes_install_print_and_uninstall_skill_work() -> Result<()> {
    let test_dir = temp_test_dir("hermes-install");
    let bin_dir = test_dir.join("bin");
    let install_dir = test_dir
        .join(".hermes")
        .join("skills")
        .join("productivity")
        .join("clipboard-memory");
    fs::create_dir_all(&bin_dir)?;

    let clipmem_link = bin_dir.join("clipmem");
    #[cfg(unix)]
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_clipmem"), &clipmem_link)?;

    let hermes_path = bin_dir.join("hermes");
    write_executable(
        &hermes_path,
        "#!/bin/sh\nif [ \"$1\" = \"skills\" ] && [ \"$2\" = \"list\" ]; then\n  printf 'clipboard-memory\\n'\n  exit 0\nfi\nexit 1\n",
    )?;

    let path_value = bin_dir.display().to_string();

    let install = run_cli_with_env(
        &["agents", "hermes", "install-skill"],
        &[("PATH", &path_value), ("HOME", test_dir.to_str().unwrap())],
    );
    assert!(install.status.success());
    assert!(install_dir.join("SKILL.md").is_file());
    assert!(install_dir.join("references/commands.md").is_file());
    assert!(install_dir.join("references/troubleshooting.md").is_file());
    assert!(install_dir.join("references/json-schema.md").is_file());
    assert!(install_dir.join("references/examples.md").is_file());
    assert!(install_dir.join("references/setup-check.md").is_file());
    assert!(install_dir.join("scripts/check-setup.sh").is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(install_dir.join("scripts/check-setup.sh"))?
            .permissions()
            .mode()
            & 0o777;
        assert!(mode & 0o111 != 0);
    }

    let duplicate = run_cli_with_env(
        &["agents", "hermes", "install-skill"],
        &[("PATH", &path_value), ("HOME", test_dir.to_str().unwrap())],
    );
    assert!(!duplicate.status.success());
    assert!(stderr_text(&duplicate).contains("pass --force to replace it"));

    let printed = run_cli_with_env(
        &["agents", "hermes", "print-skill"],
        &[("PATH", &path_value), ("HOME", test_dir.to_str().unwrap())],
    );
    assert!(printed.status.success());
    assert_eq!(
        stdout_text(&printed),
        fs::read_to_string(install_dir.join("SKILL.md"))?
    );

    let doctor = run_cli_with_env(
        &["agents", "hermes", "doctor"],
        &[("PATH", &path_value), ("HOME", test_dir.to_str().unwrap())],
    );
    assert!(doctor.status.success());
    let stdout = stdout_text(&doctor);
    assert!(stdout.contains("[OK] Host hermes on PATH"));
    assert!(stdout.contains("[OK] Hermes skill discovery"));

    let uninstall = run_cli_with_env(
        &["agents", "hermes", "uninstall-skill"],
        &[("PATH", &path_value), ("HOME", test_dir.to_str().unwrap())],
    );
    assert!(uninstall.status.success());
    assert!(!install_dir.exists());

    let _ = fs::remove_dir_all(&test_dir);
    Ok(())
}

#[test]
fn agents_hermes_install_force_preserves_unmanaged_files() -> Result<()> {
    let test_dir = temp_test_dir("hermes-install-force");
    let bin_dir = test_dir.join("bin");
    let install_dir = test_dir
        .join(".hermes")
        .join("skills")
        .join("productivity")
        .join("clipboard-memory");
    fs::create_dir_all(&bin_dir)?;
    fs::create_dir_all(&install_dir)?;
    fs::write(
        install_dir.join("SKILL.md"),
        "---\nname: clipboard-memory\n---\nstale skill",
    )?;
    fs::write(install_dir.join("old-file.txt"), "remove me")?;

    let path_value = bin_dir.display().to_string();
    let install = run_cli_with_env(
        &["agents", "hermes", "install-skill", "--force"],
        &[("PATH", &path_value), ("HOME", test_dir.to_str().unwrap())],
    );

    assert!(install.status.success(), "{}", stderr_text(&install));
    assert_eq!(
        fs::read_to_string(install_dir.join("old-file.txt"))?,
        "remove me"
    );
    assert!(fs::read_to_string(install_dir.join("SKILL.md"))?.contains("clipboard-memory"));
    assert!(install_dir.join("references/commands.md").is_file());
    assert!(install_dir.join("scripts/check-setup.sh").is_file());

    let _ = fs::remove_dir_all(&test_dir);
    Ok(())
}

#[test]
fn agents_hermes_doctor_reports_missing_clipmem_with_next_steps() -> Result<()> {
    let test_dir = temp_test_dir("hermes-doctor-missing-clipmem");
    let bin_dir = test_dir.join("bin");
    let skill_dir = test_dir
        .join(".hermes")
        .join("skills")
        .join("productivity")
        .join("clipboard-memory");
    fs::create_dir_all(&bin_dir)?;
    write_hermes_skill_package(&skill_dir)?;

    let hermes_path = bin_dir.join("hermes");
    write_executable(
        &hermes_path,
        "#!/bin/sh\nif [ \"$1\" = \"skills\" ] && [ \"$2\" = \"list\" ]; then\n  printf 'clipboard-memory\\n'\n  exit 0\nfi\nexit 1\n",
    )?;

    let output = run_cli_with_env(
        &["agents", "hermes", "doctor"],
        &[
            ("PATH", bin_dir.to_str().unwrap()),
            ("HOME", test_dir.to_str().unwrap()),
        ],
    );
    assert!(!output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("[FAIL] Host clipmem on PATH"));
    assert!(stdout.contains("brew install tristanmanchester/tap/clipmem"));

    let _ = fs::remove_dir_all(&test_dir);
    Ok(())
}

#[test]
fn agents_hermes_doctor_warns_but_passes_when_hermes_is_missing() -> Result<()> {
    let test_dir = temp_test_dir("hermes-doctor-missing-hermes");
    let bin_dir = test_dir.join("bin");
    let skill_dir = test_dir
        .join(".hermes")
        .join("skills")
        .join("productivity")
        .join("clipboard-memory");
    fs::create_dir_all(&bin_dir)?;
    write_hermes_skill_package(&skill_dir)?;

    let clipmem_link = bin_dir.join("clipmem");
    #[cfg(unix)]
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_clipmem"), &clipmem_link)?;

    let output = run_cli_with_env(
        &["agents", "hermes", "doctor"],
        &[
            ("PATH", bin_dir.to_str().unwrap()),
            ("HOME", test_dir.to_str().unwrap()),
        ],
    );
    assert!(output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("[WARN] Host hermes on PATH"));
    assert!(stdout.contains("[WARN] Hermes skill discovery"));
    assert!(stdout.contains("Skipped live discovery"));

    let _ = fs::remove_dir_all(&test_dir);
    Ok(())
}

#[test]
fn agents_hermes_doctor_fails_when_reference_file_is_missing() -> Result<()> {
    let test_dir = temp_test_dir("hermes-doctor-missing-reference");
    let bin_dir = test_dir.join("bin");
    let skill_dir = test_dir
        .join(".hermes")
        .join("skills")
        .join("productivity")
        .join("clipboard-memory");
    fs::create_dir_all(&bin_dir)?;
    write_hermes_skill_package(&skill_dir)?;
    fs::remove_file(skill_dir.join("references/troubleshooting.md"))?;

    let clipmem_link = bin_dir.join("clipmem");
    #[cfg(unix)]
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_clipmem"), &clipmem_link)?;

    let output = run_cli_with_env(
        &["agents", "hermes", "doctor"],
        &[
            ("PATH", bin_dir.to_str().unwrap()),
            ("HOME", test_dir.to_str().unwrap()),
        ],
    );
    assert!(!output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("[FAIL] SKILL.md metadata"));
    assert!(stdout.contains("packaged file is missing"));

    let _ = fs::remove_dir_all(&test_dir);
    Ok(())
}

#[cfg(unix)]
#[test]
fn agents_hermes_doctor_fails_when_setup_script_is_not_executable() -> Result<()> {
    let test_dir = temp_test_dir("hermes-doctor-nonexec-setup-script");
    let bin_dir = test_dir.join("bin");
    let skill_dir = test_dir
        .join(".hermes")
        .join("skills")
        .join("productivity")
        .join("clipboard-memory");
    fs::create_dir_all(&bin_dir)?;
    write_hermes_skill_package(&skill_dir)?;
    set_mode(&skill_dir.join("scripts/check-setup.sh"), 0o644)?;

    let clipmem_link = bin_dir.join("clipmem");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_clipmem"), &clipmem_link)?;

    let output = run_cli_with_env(
        &["agents", "hermes", "doctor"],
        &[
            ("PATH", bin_dir.to_str().unwrap()),
            ("HOME", test_dir.to_str().unwrap()),
        ],
    );
    assert!(!output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("[FAIL] SKILL.md metadata"));
    assert!(stdout.contains("packaged script is not executable"));
    assert!(stdout.contains("scripts/check-setup.sh"));

    let _ = fs::remove_dir_all(&test_dir);
    Ok(())
}

#[test]
fn skill_management_rejects_unrelated_destinations_and_preserves_extras() -> Result<()> {
    for runtime in ["hermes", "openclaw"] {
        let root = temp_test_dir(&format!("{runtime}-safe-delete"));
        fs::create_dir_all(&root)?;
        fs::write(root.join("important.txt"), "keep")?;
        let destination = root.to_str().unwrap();
        for action in ["install-skill", "uninstall-skill"] {
            let mut args = vec!["agents", runtime, action, "--dest", destination];
            if action == "install-skill" {
                args.push("--force");
            }
            let output = run_cli_with_env(&args, &[]);
            assert!(!output.status.success());
            assert_eq!(fs::read_to_string(root.join("important.txt"))?, "keep");
        }
        let skill = root.join("clipboard-memory");
        let output = run_cli_with_env(
            &[
                "agents",
                runtime,
                "install-skill",
                "--dest",
                skill.to_str().unwrap(),
            ],
            &[],
        );
        assert!(output.status.success(), "{}", stderr_text(&output));
        fs::write(skill.join("personal.txt"), "keep too")?;
        let output = run_cli_with_env(
            &[
                "agents",
                runtime,
                "uninstall-skill",
                "--dest",
                skill.to_str().unwrap(),
            ],
            &[],
        );
        assert!(output.status.success(), "{}", stderr_text(&output));
        assert!(!skill.join("SKILL.md").exists());
        assert_eq!(fs::read_to_string(skill.join("personal.txt"))?, "keep too");
        fs::remove_dir_all(root)?;
    }
    Ok(())
}
