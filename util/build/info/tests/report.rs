// Copyright (c) 2018-2022 The Botho Foundation

/// Test that bth_util_build_info::write_report produces valid json
#[test]
fn build_info_report_json() {
    let mut buf = String::new();
    bth_util_build_info::write_report(&mut buf).unwrap();

    json::parse(&buf).unwrap();
}
