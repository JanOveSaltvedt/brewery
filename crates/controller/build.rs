fn main() {
    println!("cargo:rerun-if-changed=esp_config.yml");
    println!("cargo:rerun-if-changed=cfg.toml");

    // esp-config 0.7+ reads values from environment variables, not cfg.toml.
    // Parse cfg.toml here and inject each entry so esp-config picks them up.
    if let Ok(contents) = std::fs::read_to_string("cfg.toml") {
        let table: toml::Table = contents.parse().expect("cfg.toml: invalid TOML");
        if let Some(section) = table.get("brewtech-controller").and_then(|v| v.as_table()) {
            for (key, value) in section {
                let env_key = format!(
                    "BREWTECH_CONTROLLER_CONFIG_{}",
                    key.to_uppercase().replace('-', "_")
                );
                let env_val = match value {
                    toml::Value::String(s) => s.clone(),
                    toml::Value::Integer(i) => i.to_string(),
                    toml::Value::Boolean(b) => b.to_string(),
                    toml::Value::Float(f) => f.to_string(),
                    _ => continue,
                };
                // SAFETY: build scripts are single-threaded.
                unsafe { std::env::set_var(&env_key, env_val) };
            }
        }
    }

    let yaml = std::fs::read_to_string("esp_config.yml").expect("esp_config.yml missing");
    esp_config::generate_config_from_yaml_definition(&yaml, true, false, None)
        .expect("esp-config generation failed");
}
