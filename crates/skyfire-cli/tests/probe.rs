use std::process::Command;

fn probe_json(fixture: &str) -> serde_json::Value {
    let bin = env!("CARGO_BIN_EXE_skyfire");
    let path = format!("{}/../../fixtures/{}", env!("CARGO_MANIFEST_DIR"), fixture);
    let out = Command::new(bin)
        .arg(&path)
        .arg("--probe")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "probe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("valid json")
}

#[test]
fn france2_probe_reports_three_audio_and_two_subtitles() {
    let v = probe_json("france2-8s.ts");
    let audio = v["audio"].as_array().unwrap();
    assert_eq!(audio.len(), 3, "france2-8s has 3 audio tracks");
    let subs = v["subtitle"].as_array().unwrap();
    assert_eq!(subs.len(), 2, "france2-8s has 2 DVB-subtitle tracks");
    // Codec strings are canonical uppercase.
    assert!(
        audio
            .iter()
            .all(|a| ["AC3", "EAC3", "MP2"].contains(&a["codec"].as_str().unwrap()))
    );
    // A language tag is present on at least the primary audio.
    assert!(audio.iter().any(|a| a["lang"].as_str() == Some("fre")));
    assert!(v["default_audio_pid"].is_number());
}
