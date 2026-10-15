//! Variables XDG définies et valeurs effectives.

use std::env;

use crate::{Section, Value};

pub fn probe() -> Section {
    let mut s = Section::new("xdg");
    let home = env::var("HOME").ok();
    let under_home = |sub: &str| home.as_ref().map(|h| format!("{h}/{sub}"));
    let vars: [(&str, Option<String>); 9] = [
        ("XDG_CONFIG_HOME", under_home(".config")),
        ("XDG_DATA_HOME", under_home(".local/share")),
        ("XDG_CACHE_HOME", under_home(".cache")),
        ("XDG_STATE_HOME", under_home(".local/state")),
        ("XDG_RUNTIME_DIR", None),
        (
            "XDG_DATA_DIRS",
            Some("/usr/local/share:/usr/share".to_string()),
        ),
        ("XDG_CONFIG_DIRS", Some("/etc/xdg".to_string())),
        ("XDG_SESSION_TYPE", None),
        ("XDG_CURRENT_DESKTOP", None),
    ];
    for (name, default) in vars {
        let value = match (env::var(name), default) {
            (Ok(v), _) if !v.is_empty() => Value::ok(format!("{v} (défini)")),
            (_, Some(d)) => Value::ok(format!("{d} (défaut)")),
            (_, None) => Value::ok("non défini"),
        };
        s.push(name, value);
    }
    s
}
