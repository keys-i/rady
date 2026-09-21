use std::process::Command;

use serde_json::Value;

#[test]
fn json_contract_reaches_the_process_boundary() {
    for (arguments, exit_code, kind, message) in [
        (
            [
                "--output",
                "json",
                "code",
                "change",
                "--directory",
                "/.rady-json-contract-no-project",
            ]
            .as_slice(),
            1,
            "runtime",
            "could not infer a project test command",
        ),
        (
            ["dependasolve", "--output=json"].as_slice(),
            2,
            "usage",
            "required arguments",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rady"))
            .args(arguments)
            .output()
            .expect("Rady must start");
        assert_eq!(output.status.code(), Some(exit_code));
        assert!(output.stdout.is_empty());
        let document: Value =
            serde_json::from_slice(&output.stderr).expect("stderr must contain one JSON document");
        assert_eq!(document["schema"], 1);
        assert_eq!(document["status"], "error");
        assert_eq!(document["error"]["kind"], kind);
        assert!(
            document["error"]["message"]
                .as_str()
                .is_some_and(|value| value.contains(message))
        );
    }

    let directory = tempfile::tempdir().expect("temporary setup directory");
    let output = Command::new(env!("CARGO_BIN_EXE_rady"))
        .args([
            "--output",
            "json",
            "dependasolve",
            "--repo",
            "owner/repo",
            "--solver-ref",
            &format!("keys-i/rady@{}", "a".repeat(40)),
            "--checks",
            "test",
            "--directory",
            directory.path().to_str().expect("UTF-8 temporary path"),
        ])
        .output()
        .expect("Rady must produce a setup preview");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let document: Value =
        serde_json::from_slice(&output.stdout).expect("stdout must contain one JSON document");
    assert_eq!(document["schema"], 1);
    assert_eq!(document["status"], "ok");
    assert_eq!(document["kind"], "dependasolve");
    assert_eq!(document["result"]["repository"], "owner/repo");
}
