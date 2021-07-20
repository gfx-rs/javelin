// Test that tries to parse a set of Rust GPU generated SPIRV shaders from Embark Studios
#[cfg(feature = "spv-in")]
#[test]
fn parse_embark_spv() {
    let path = std::path::PathBuf::from("test-data")
        .join("spv")
        .join("embark.spv");
    let spv_bytes = std::fs::read(path).expect("Couldn't read embark.spv");

    let module =
        naga::front::spv::parse_u8_slice(&spv_bytes, &naga::front::spv::Options::default())
            .expect("Module parsing failed");
    naga::proc::Validator::new()
        .validate(&module)
        .expect("Module validation failed");
}
