use std::{env, fs, path::PathBuf};

use protox::prost::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file_descriptors = protox::compile(["proto/speedupdate.proto"], ["."]).unwrap();

    let file_descriptor_path = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"))
        .join("file_descriptor_set.bin");
    fs::write(&file_descriptor_path, file_descriptors.encode_to_vec()).unwrap();

    let mut config = prost_build::Config::new();
    config.protoc_arg("--experimental_allow_proto3_optional");
    tonic_build::configure()
        .skip_protoc_run()
        .file_descriptor_set_path(&file_descriptor_path)
        .compile_protos_with_config(config, &["proto/speedupdate.proto"], &["proto"])?;
    Ok(())
}
