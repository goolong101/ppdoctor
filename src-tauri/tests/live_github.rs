// Live integration test against the real github.com/goolong101/ppenhancer
// release. Marked `#[ignore]` so `cargo test` doesn't hit the network by
// default. Run explicitly with:
//   cargo test --test live_github --release -- --ignored --nocapture

#[test]
#[ignore]
fn check_ppdoctor_latest_release() {
    let url = "https://api.github.com/repos/goolong101/ppenhancer/releases/latest";
    let resp = ureq::get(url)
        .set("User-Agent", "pp-doctor-updater-test")
        .set("Accept", "application/vnd.github+json")
        .call()
        .expect("github api fetch failed");
    let v: serde_json::Value = resp.into_json().expect("parse json");

    let tag = v.get("tag_name").and_then(|t| t.as_str()).expect("tag_name");
    let n_assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    println!("Latest tag: {}", tag);
    println!("Asset count: {}", n_assets);

    // Verify the v0.1.0 release we just published.
    assert_eq!(tag, "v0.1.0", "expected v0.1.0 tag");
    assert_eq!(n_assets, 6, "expected 6 assets (5 files + SHA256SUMS)");

    // Asset names we expect to find.
    let expected = [
        "pinnerpi_sdl",
        "pinnerpi_power_daemon",
        "commands.json",
        "pinball_tables.json",
        "VERSION",
        "SHA256SUMS",
    ];
    let assets = v.get("assets").and_then(|a| a.as_array()).unwrap();
    for want in &expected {
        let found = assets
            .iter()
            .any(|a| a.get("name").and_then(|n| n.as_str()) == Some(*want));
        assert!(found, "asset {} missing from release", want);
    }
}
