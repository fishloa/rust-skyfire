use assert_cmd::Command;

fn fixture_path(name: &str) -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
        .to_string_lossy()
        .to_string()
}

#[test]
fn gulli_15s_reports_h264_video_and_eac3_audio() {
    let output = Command::cargo_bin("skyfire")
        .unwrap()
        .arg(fixture_path("gulli-15s.ts"))
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("Channel map: video PID 0x0100 (H264)"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("audio PID 0x0101 (EAc3)"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn h264_25fps_reports_h264_video() {
    let output = Command::cargo_bin("skyfire")
        .unwrap()
        .arg(fixture_path("h264-25fps.ts"))
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("Channel map: video PID 0x0100 (H264)"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn garbage_input_exits_nonzero_no_panic() {
    let output = Command::cargo_bin("skyfire")
        .unwrap()
        .arg("/dev/null")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("no PAT/PMT channel map found"),
        "stderr:\n{stderr}"
    );
}
