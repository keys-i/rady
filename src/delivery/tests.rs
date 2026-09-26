use super::*;

#[test]
fn repository_remote_parser_covers_supported_and_rejected_forms() {
    for (remote, expected) in [
        ("https://github.com/owner/repo.git", Some("owner/repo")),
        ("git@github.com:owner/repo.git", Some("owner/repo")),
        ("ssh://git@github.com/owner/repo", Some("owner/repo")),
        ("https://example.com/owner/repo", None),
    ] {
        assert_eq!(repository_from_remote(remote).as_deref(), expected);
    }
}

#[test]
fn hosted_auth_is_bound_and_never_part_of_config() -> Result<()> {
    let auth = DeliveryAuth::installation("owner/repo", "installation-secret")?;
    assert_eq!(auth.repository, "owner/repo");
    assert_eq!(auth.token, "installation-secret");
    assert!(auth.revalidate_publication().is_err());
    for (repository, token) in [
        ("owner", "token"),
        ("owner/repo", ""),
        ("owner/repo", &"x".repeat(git::MAX_PUSH_TOKEN_BYTES + 1)),
    ] {
        assert!(DeliveryAuth::installation(repository, token).is_err());
    }
    Ok(())
}

#[test]
fn hosted_delivery_refuses_protected_paths() {
    for (path, allowed) in [
        ("src/lib.rs", true),
        (".github/workflows/checks.yml", false),
        (".github/actions/setup/action.yml", false),
        (".github/koelu.json", false),
        (".env", false),
        ("AGENTS.md", false),
        ("DESIGN.md", false),
        ("GEMINI.md", false),
        (".koelu/context.json", false),
        (".gemini/settings.json", false),
        (".github/workflows-old/notes.md", true),
    ] {
        assert_eq!(
            require_safe_hosted_publication_paths(&[path.to_owned()]).is_ok(),
            allowed,
            "{path}"
        );
    }
}

#[test]
fn command_parser_and_tail_cover_quotes_unicode_and_limits() -> Result<()> {
    let commands = parse_commands(&["cargo test -- 'two words'".to_owned()])?;
    assert_eq!(commands[0], ["cargo", "test", "--", "two words"]);
    assert_eq!(tail("hello", 3), "llo");
    assert_eq!(tail("🦔rust", 4), "rust");
    Ok(())
}

#[test]
fn pull_request_body_keeps_hostile_evidence_inert_and_bounded() -> Result<()> {
    let hostile = "@team &#64;team &commat;team <!-- nope -->\u{1b}[31m\u{202e} # heading [link](https://example.test)";
    let config = Config {
        task: hostile.to_owned(),
        directory: PathBuf::from("."),
        repo: Some("owner/repo".to_owned()),
        checks: vec![hostile.to_owned()],
        harness: Harness::Command,
        agents: 1,
        model: None,
        model_choices: Vec::new(),
        review_model: None,
        plan: None,
        acceptance_checks: Vec::new(),
        max_tokens: None,
        orchestrator_model: None,
        orchestrator_harness: None,
        base: None,
        attempts: 1,
        timeout: Duration::from_secs(1),
        benchmarks: Vec::new(),
        benchmark_runs: 3,
        benchmark_warmups: 0,
        benchmark_metric: None,
        max_benchmark_noise: 1.0,
        max_regression: 1.0,
        max_files: 1,
        max_lines: 1,
        theme: Theme::Plain,
        output: OutputMode::Human,
        seed_patch: None,
        resumed_from: None,
        expected_start: None,
        mcp_servers: Vec::new(),
        ghost: false,
    };
    let plan = Plan {
        acceptance: vec![hostile.to_owned()],
        scope: vec!["src".to_owned()],
        limitations: vec![hostile.to_owned()],
        performance_required: false,
        model_index: None,
        tasks: Vec::new(),
    };
    let mut gates = BTreeMap::new();
    gates.insert(
        hostile.to_owned(),
        quality::Gate {
            status: quality::GateStatus::Pass,
            evidence: hostile.to_owned(),
        },
    );
    let report = ReviewReport {
        summary: hostile.to_owned(),
        gates,
        blockers: Vec::new(),
        limitations: vec![hostile.to_owned()],
        acceptance: vec![quality::AcceptanceResult {
            criterion: 0,
            status: quality::GateStatus::Pass,
            evidence: hostile.to_owned(),
            checks: Vec::new(),
        }],
    };
    let body = pull_request_body(&config, &plan, &report, &[], &[])?;
    assert!(body.contains("## Requested change"));
    assert!(body.contains("@\u{200b}team"));
    assert!(body.contains(r"&amp;\#64;team"));
    assert!(body.contains("&amp;commat;team"));
    assert!(body.contains(r"\<\!\-\- nope \-\-\>"));
    assert!(body.contains(r"\# heading \[link\]\(https://example\.test\)"));
    assert!(!body.contains('\u{1b}'));
    assert!(!body.contains('\u{202e}'));
    let title = github_plain_text(hostile);
    assert!(title.contains("@\u{200b}team"));
    assert!(!title.contains('\u{1b}'));
    assert!(!title.contains('\u{202e}'));
    assert_eq!(
        pull_request_title("\n\u{202e}\n# Ship the fix"),
        "Ship the fix"
    );
    assert_eq!(
        pull_request_title("#  \n\u{202e}"),
        "Implement the requested specification"
    );

    let oversized_field = Config {
        task: "x".repeat(MAX_PULL_REQUEST_FIELD_BYTES + 1),
        ..config
    };
    assert!(
        pull_request_body(&oversized_field, &plan, &report, &[], &[])
            .unwrap_err()
            .to_string()
            .contains("task is too large")
    );

    let oversized_body = Config {
        task: "x".repeat(30_000),
        ..oversized_field
    };
    let oversized_report = ReviewReport {
        summary: "y".repeat(30_000),
        gates: BTreeMap::new(),
        blockers: Vec::new(),
        limitations: Vec::new(),
        acceptance: Vec::new(),
    };
    assert!(
        pull_request_body(&oversized_body, &plan, &oversized_report, &[], &[])
            .unwrap_err()
            .to_string()
            .contains("body is too large")
    );
    Ok(())
}
