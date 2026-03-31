mod support;

use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse};

#[tokio::test]
async fn add_file_overwrites_existing_content() {
    let fixture = support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("demo.txt");
    tokio::fs::write(&path, "old\n").await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: "*** Begin Patch\n*** Add File: demo.txt\n+new\n*** End Patch\n"
                    .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("Success."));
    assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "new\n");
}

#[tokio::test]
async fn patch_failures_do_not_roll_back_earlier_file_changes() {
    let fixture = support::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Delete File: missing.txt\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "after\n",
    );
}
