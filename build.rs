use embuild::cargo::set_rustc_env;
use embuild::kconfig::{try_from_config_file, Value};

fn main() {
    embuild::espidf::sysenv::output();

    for cfg in try_from_config_file("src/kconfig.projbuild").unwrap() {
        let (key, value) = cfg;
        match value {
            Value::String(string) => {
                set_rustc_env(key, string);
            },
            _ => continue
        }
    };
}
