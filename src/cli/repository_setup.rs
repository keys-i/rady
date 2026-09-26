use std::io::{self, IsTerminal, Write};

use anyhow::bail;
use serde_json::Value;

use crate::Result;
use crate::setup::{self, SourceRef};
use crate::ui::{OutputMode, Theme, Ui, json_success_document, print_markdown};

use super::{DependSolveArgs, SetupArgs};

pub(super) fn setup(mut arguments: SetupArgs, theme: Theme, output: OutputMode) -> Result<()> {
    let mut ui = Ui::new(theme, output, 7);
    ui.title(
        "Koelu setup",
        "Connect this repository without copying secrets into it",
    );
    ui.stage("Finding the repository");
    let repository = setup::resolve_repository_in(arguments.repo.as_deref(), &arguments.directory)?;
    ui.stage("Finding a check to run");
    let checks = setup::resolve_checks(&repository, &arguments.checks)?;
    ui.stage("Checking the service agreement");
    let agreement_exists = setup::has_verified_agreement(&repository, &arguments.directory)?;
    if arguments.accept_terms || !agreement_exists {
        ui.finish_progress();
        arguments.accept_terms =
            accept_terms(&repository, &checks, theme, output, arguments.accept_terms)?;
    } else {
        arguments.accept_terms = true;
    }
    ui.stage(if agreement_exists {
        "Agreement already on file"
    } else {
        "Agreement accepted"
    });
    ui.stage("Pinning the trusted Koelu version");
    let source = SourceRef::resolve(arguments.solver_ref.as_deref())?;
    ui.stage("Connecting Koelu and saving the setup");
    ui.finish_progress();
    let preview = setup::run(
        &repository,
        &source,
        &checks,
        &arguments.directory,
        !arguments.no_overwrite,
        true,
        arguments.accept_terms,
    )?;
    ui.stage("Ready to review");
    if output == OutputMode::Json {
        println!("{}", json_success_document("setup", &preview)?);
        return Ok(());
    }
    let files = preview["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|path| format!("- `{path}`"))
        .collect::<Vec<_>>()
        .join("\n");
    print_markdown(
        &format!(
            "## Repository is ready\n\n**Repository:** `{repository}`\n\n**Checks:** {}\n\n**Credentials stay in:** `keys-i/koelu`\n\nNo secrets were added here. If GitHub opened the Koelu installation page, finish it, then commit the generated files below. The service will handle mentions and dependency pull requests on its next pass.\n\n### Files\n\n{files}",
            checks.join(", ")
        ),
        theme,
    )
}

pub(super) fn accept_terms(
    repository: &str,
    checks: &[String],
    theme: Theme,
    output: OutputMode,
    accepted: bool,
) -> Result<bool> {
    if output == OutputMode::Json
        || !io::stdin().is_terminal()
        || !io::stdout().is_terminal()
        || !io::stderr().is_terminal()
    {
        if accepted {
            return Ok(true);
        }
        bail!(
            "read {} and {}, then rerun with --accept-terms if you agree",
            setup::TERMS_URL,
            setup::PRIVACY_URL
        );
    }
    print_markdown(&setup_consent_preview(repository, checks), theme)?;
    if accepted {
        eprintln!("Accepted with --accept-terms. Continuing with {repository}.\n");
        return Ok(true);
    }
    eprint!("Accept and continue? [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        return Ok(true);
    }
    bail!("setup was not changed; rerun with --accept-terms after you agree")
}

pub(super) fn setup_consent_preview(repository: &str, checks: &[String]) -> String {
    format!(
        "## Before Koelu connects\n\nFor `{repository}`, Koelu will:\n\n- verify your admin access and open the Koelu installation page if needed\n- use `{}` as CI evidence\n- read relevant issues, pull requests, diffs and check results\n- record your agreement in a closed issue and non-secret `.github/koelu.json` file\n- add Dependabot configuration only when it is missing\n- send bounded evidence to the model providers described in the privacy policy\n\nYour App and model credentials stay in `keys-i/koelu`. Koelu won't copy them here or change branch protection. You still decide what gets merged.\n\n**Terms:** {}\n\n**Privacy:** {}\n",
        checks.join("`, `"),
        setup::TERMS_URL,
        setup::PRIVACY_URL,
    )
}

pub(super) fn dependasolve(
    arguments: DependSolveArgs,
    theme: Theme,
    output: OutputMode,
) -> Result<()> {
    let mut ui = Ui::new(theme, output, 3);
    ui.title(
        "Koelu dependasolve",
        "Review dependency updates with the checks you trust",
    );
    ui.stage("Finding Koelu's source");
    let source = SourceRef::resolve(arguments.solver_ref.as_deref())?;
    ui.stage("Checking repository setup");
    ui.finish_progress();
    let preview = setup::run(
        &arguments.repo,
        &source,
        &arguments.checks,
        &arguments.directory,
        !arguments.no_overwrite,
        arguments.apply,
        arguments.accept_terms,
    )?;
    ui.stage(if arguments.apply {
        "Repository ready"
    } else {
        "Preview ready"
    });
    if output == OutputMode::Json {
        println!("{}", json_success_document("dependasolve", &preview)?);
    } else {
        let files = preview["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|path| format!("- `{path}`"))
            .collect::<Vec<_>>()
            .join("\n");
        let markdown = format!(
            "## {}\n\n**Repository:** `{}`\n\n**Source:** `{}`\n\n**CI evidence:** {}\n\n**Branch protection:** unchanged\n\n### Files\n\n{}\n\n{}",
            if arguments.apply {
                "Repository is ready"
            } else {
                "Setup preview"
            },
            arguments.repo,
            source.joined(),
            arguments.checks.join(", "),
            files,
            if arguments.apply {
                "Your agreement is saved. Koelu will check its access, then handle mentions and pending dependency pull requests."
            } else {
                "Read `docs/TERMS.md` and `docs/PRIVACY.md`, then run again with `--apply --accept-terms` if you agree."
            }
        );
        print_markdown(&markdown, theme)?;
    }
    Ok(())
}
