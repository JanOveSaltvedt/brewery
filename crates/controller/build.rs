fn main() {
    println!("cargo:rerun-if-changed=esp_config.yml");
    println!("cargo:rerun-if-changed=cfg.toml");

    let yaml = std::fs::read_to_string("esp_config.yml")
        .expect("esp_config.yml missing");
    esp_config::generate_config_from_yaml_definition(&yaml, true, false, None)
        .expect("esp-config generation failed");
}
