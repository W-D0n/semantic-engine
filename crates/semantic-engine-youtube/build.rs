fn main() {
    let proto_root = std::path::PathBuf::from("proto");
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    let include = protoc_bin_vendored::include_path().expect("vendored protobuf includes");
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    tonic_prost_build::configure()
        .build_server(true)
        .compile_with_config(
            config,
            &[proto_root.join("stream_list.proto")],
            &[proto_root, include],
        )
        .expect("compile official YouTube streamList subset");
    println!("cargo:rerun-if-changed=proto/stream_list.proto");
}
