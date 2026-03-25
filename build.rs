use std::env;
use std::path::PathBuf;

fn main() {
    // Find Cyclone DDS via pkg-config
    let cyclone = pkg_config::Config::new()
        .atleast_version("11.0.0")
        .probe("CycloneDDS")
        .expect("CycloneDDS not found via pkg-config. Install Cyclone DDS >= 11.0.0");

    // Collect include paths for bindgen
    let include_args: Vec<String> = cyclone
        .include_paths
        .iter()
        .map(|p| format!("-I{}", p.display()))
        .collect();

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(&include_args)
        // Core DDS API
        .allowlist_function("dds_create_participant")
        .allowlist_function("dds_create_topic")
        .allowlist_function("dds_create_writer")
        .allowlist_function("dds_create_reader")
        .allowlist_function("dds_write")
        .allowlist_function("dds_writedispose")
        .allowlist_function("dds_dispose")
        .allowlist_function("dds_read")
        .allowlist_function("dds_take")
        .allowlist_function("dds_delete")
        .allowlist_function("dds_return_loan")
        .allowlist_function("dds_set_status_mask")
        .allowlist_function("dds_get_status_changes")
        .allowlist_function("dds_waitset_create")
        .allowlist_function("dds_waitset_attach")
        .allowlist_function("dds_waitset_wait")
        .allowlist_function("dds_waitset_detach")
        // Listener
        .allowlist_function("dds_create_listener")
        .allowlist_function("dds_delete_listener")
        .allowlist_function("dds_lset_data_available")
        // QoS
        .allowlist_function("dds_create_qos")
        .allowlist_function("dds_delete_qos")
        .allowlist_function("dds_qset_reliability")
        .allowlist_function("dds_qset_durability")
        .allowlist_function("dds_qset_history")
        .allowlist_function("dds_qset_deadline")
        .allowlist_function("dds_qset_latency_budget")
        .allowlist_function("dds_qset_ownership")
        .allowlist_function("dds_qset_ownership_strength")
        .allowlist_function("dds_qset_liveliness")
        .allowlist_function("dds_qset_destination_order")
        .allowlist_function("dds_qset_presentation")
        .allowlist_function("dds_qset_partition")
        .allowlist_function("dds_qset_writer_data_lifecycle")
        .allowlist_function("dds_qset_time_based_filter")
        // Topic with sertype
        .allowlist_function("dds_create_topic_sertype")
        // Sertype internals
        .allowlist_type("ddsi_sertype")
        .allowlist_type("ddsi_sertype_ops")
        .allowlist_type("ddsi_serdata")
        .allowlist_type("ddsi_serdata_ops")
        .allowlist_type("ddsi_serdata_kind")
        .allowlist_function("ddsi_sertype_init")
        .allowlist_function("ddsi_serdata_init")
        // Fragchain helper for from_ser callback (compiled from helper.c)
        // SampleInfo
        .allowlist_type("dds_sample_info_t")
        // Status masks
        .allowlist_var("DDS_DATA_AVAILABLE_STATUS")
        // Free op bits
        .allowlist_var("DDS_FREE_ALL_BIT")
        .allowlist_var("DDS_FREE_CONTENTS_BIT")
        // Derive traits
        .derive_debug(true)
        .derive_default(true)
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings");

    // Compile helper.c (fragchain copy helper)
    let mut cc_build = cc::Build::new();
    cc_build.file("helper.c");
    for path in &cyclone.include_paths {
        cc_build.include(path);
    }
    cc_build.compile("helper");
}
