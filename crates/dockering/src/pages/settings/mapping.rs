//! Add-engine form ⇄ [`EngineConfig`] mapping (ENG-105, spec 21 §8).
//!
//! The dialog renders whatever [`EngineConfigSchema`]s the registered factories publish; this
//! module is the only place that knows how a schema's field values become an
//! [`EngineEndpoint`]. It is data mapping keyed by schema kind + label (allowed by ENG-030:
//! the view never branches behaviour on the engine kind). A schema without a mapping is not
//! offered in the dialog.

use std::collections::BTreeMap;

use dk_core::{
    ConfigField, ConfigFieldKind, EngineConfig, EngineConfigSchema, EngineEndpoint, EngineId,
    EngineKind, EngineOrigin, EngineStatus, TlsFiles, WslMode, WslcTransportPref,
};

use crate::strings as s;

/// Field values by `ConfigField::key` (Bool fields hold `"true"` / `"false"`).
pub type Values = BTreeMap<String, String>;
/// Validation messages by field key; [`FORM`] holds form-level messages.
pub type Errors = BTreeMap<String, &'static str>;

/// Key of form-level errors in [`Errors`].
pub const FORM: &str = "";

/// Field keys whose values can be suggested from engines Dockering already knows (ENG-105:
/// "dropdown of detected distros").
const SUGGEST_KEYS: &[&str] = &["distro", "session"];

/// What an endpoint looks like when known: schema `kind` + `label`.
fn endpoint_for(
    schema: &EngineConfigSchema,
    v: &Values,
    errors: &mut Errors,
) -> Option<EngineEndpoint> {
    let get = |k: &str| v.get(k).map(|x| x.trim().to_owned()).unwrap_or_default();
    let port = |k: &str| get(k).parse::<u16>().ok().filter(|p| *p > 0);
    let endpoint = match (schema.kind, schema.label.as_str()) {
        (EngineKind::Docker, "Unix socket") => EngineEndpoint::UnixSocket {
            path: get("path").into(),
        },
        (EngineKind::Docker, "Named pipe") => EngineEndpoint::NamedPipe { path: get("path") },
        (EngineKind::Docker, "TCP") => EngineEndpoint::Tcp {
            host: get("host"),
            port: port("port").unwrap_or(0),
            tls: None,
        },
        (EngineKind::Docker, "TCP + TLS") => EngineEndpoint::Tcp {
            host: get("host"),
            port: port("port").unwrap_or(0),
            tls: Some(TlsFiles {
                ca: get("ca").into(),
                cert: get("cert").into(),
                key: get("key").into(),
                verify: is_true(&get("verify")),
            }),
        },
        (EngineKind::WslDistro, _) => {
            let mode = if get("mode").eq_ignore_ascii_case("tcp") {
                match port("port") {
                    Some(p) => WslMode::Tcp { port: p },
                    None => {
                        errors.entry("port".into()).or_insert(s::INVALID_PORT);
                        WslMode::DialStdio
                    }
                }
            } else {
                WslMode::DialStdio
            };
            EngineEndpoint::WslDistro {
                distro: get("distro"),
                mode,
            }
        }
        (EngineKind::Wslc, _) => EngineEndpoint::Wslc {
            session: Some(get("session")).filter(|x| !x.is_empty()),
            transport: match get("transport").to_ascii_lowercase().as_str() {
                "com" => WslcTransportPref::Com,
                "cli" => WslcTransportPref::Cli,
                _ => WslcTransportPref::Auto,
            },
        },
        _ => return None,
    };
    Some(endpoint)
}

/// Whether the dialog can turn this schema into a config.
pub fn addable(schema: &EngineConfigSchema) -> bool {
    let values: Values = schema
        .fields
        .iter()
        .map(|f| (f.key.clone(), default_value(f)))
        .collect();
    endpoint_for(schema, &values, &mut Errors::new()).is_some()
}

/// Initial value of a field.
pub fn default_value(field: &ConfigField) -> String {
    match &field.kind {
        // TLS verification stays on unless the user turns it off.
        ConfigFieldKind::Bool => (field.key == "verify").to_string(),
        ConfigFieldKind::Choice { options } => field
            .placeholder
            .as_ref()
            .filter(|p| options.contains(p))
            .or(options.first())
            .cloned()
            .unwrap_or_default(),
        _ => String::new(),
    }
}

pub fn is_true(v: &str) -> bool {
    v == "true"
}

/// A host name or IP literal: no whitespace, no scheme, no path.
pub fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && !h.contains("://")
        && !h.chars().any(|c| c.is_whitespace() || c == '/')
}

/// Per-field validation (required, port range, host syntax).
pub fn validate_field(field: &ConfigField, value: &str) -> Option<&'static str> {
    let v = value.trim();
    if v.is_empty() {
        return (field.required && !matches!(field.kind, ConfigFieldKind::Bool))
            .then_some(s::FIELD_REQUIRED);
    }
    match field.kind {
        ConfigFieldKind::Port => match v.parse::<u16>() {
            Ok(p) if p > 0 => None,
            _ => Some(s::INVALID_PORT),
        },
        ConfigFieldKind::Text if field.key == "host" => (!valid_host(v)).then_some(s::INVALID_HOST),
        _ => None,
    }
}

/// Validates the form and builds the manual engine config (origin `Manual`, enabled).
pub fn build(
    schema: &EngineConfigSchema,
    values: &Values,
    name: &str,
) -> Result<EngineConfig, Errors> {
    let mut errors = Errors::new();
    for f in &schema.fields {
        let v = values.get(&f.key).map(String::as_str).unwrap_or("");
        if let Some(e) = validate_field(f, v) {
            errors.insert(f.key.clone(), e);
        }
    }
    let Some(endpoint) = endpoint_for(schema, values, &mut errors) else {
        errors.insert(FORM.into(), s::KIND_NOT_ADDABLE);
        return Err(errors);
    };
    if !errors.is_empty() {
        return Err(errors);
    }
    let name = name.trim();
    let id = slug(if name.is_empty() {
        endpoint.display()
    } else {
        name.to_owned()
    });
    Ok(EngineConfig {
        id,
        name: name.to_owned(),
        endpoint,
        origin: EngineOrigin::Manual,
        enabled: true,
        hidden: false,
    })
}

/// Provisional id (the hub uniquifies it on `add_engine`): lowercase alphanumerics, other
/// characters collapse to `-`.
pub fn slug(name: impl AsRef<str>) -> EngineId {
    let mut out = String::new();
    for c in name.as_ref().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    EngineId::new(if out.is_empty() { "engine" } else { out })
}

/// Field values an existing endpoint carries (inverse of [`build`] for suggestions).
fn endpoint_values(e: &EngineEndpoint) -> Vec<(&'static str, String)> {
    match e {
        EngineEndpoint::WslDistro { distro, .. } => vec![("distro", distro.clone())],
        EngineEndpoint::Wslc {
            session: Some(session),
            ..
        } => vec![("session", session.clone())],
        _ => Vec::new(),
    }
}

/// Values for `key` taken from engines Dockering already knows (e.g. detected WSL distros).
pub fn suggestions(key: &str, engines: &[EngineStatus]) -> Vec<String> {
    if !SUGGEST_KEYS.contains(&key) {
        return Vec::new();
    }
    let mut out: Vec<String> = engines
        .iter()
        .flat_map(|e| endpoint_values(&e.config.endpoint))
        .filter(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schemas() -> Vec<EngineConfigSchema> {
        dk_hub::default_factories()
            .iter()
            .flat_map(|f| f.config_schema())
            .collect()
    }

    fn schema(label: &str) -> EngineConfigSchema {
        schemas()
            .into_iter()
            .find(|s| s.label == label)
            .unwrap_or_else(|| panic!("no schema {label}"))
    }

    fn values(pairs: &[(&str, &str)]) -> Values {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn eng_105_every_default_schema_is_addable() {
        for s in schemas() {
            assert!(addable(&s), "{} has no mapping", s.label);
        }
        let fake = EngineConfigSchema {
            kind: EngineKind::Docker,
            label: "Fake".into(),
            fields: vec![],
        };
        assert!(!addable(&fake));
    }

    #[test]
    fn eng_105_tcp_maps_to_manual_enabled_config() {
        let cfg = build(
            &schema("TCP"),
            &values(&[("host", " 10.0.0.9 "), ("port", "2375")]),
            "Build box",
        )
        .unwrap();
        assert_eq!(cfg.id.as_str(), "build-box");
        assert_eq!(cfg.name, "Build box");
        assert_eq!(cfg.origin, EngineOrigin::Manual);
        assert!(cfg.enabled && !cfg.hidden);
        assert_eq!(
            cfg.endpoint,
            EngineEndpoint::Tcp {
                host: "10.0.0.9".into(),
                port: 2375,
                tls: None
            }
        );
    }

    #[test]
    fn eng_105_tls_files_and_verify_default_on() {
        let s = schema("TCP + TLS");
        let verify = s.fields.iter().find(|f| f.key == "verify").unwrap();
        assert_eq!(default_value(verify), "true");
        let cfg = build(
            &s,
            &values(&[
                ("host", "docker.example.com"),
                ("port", "2376"),
                ("ca", "/c/ca.pem"),
                ("cert", "/c/cert.pem"),
                ("key", "/c/key.pem"),
                ("verify", "true"),
            ]),
            "",
        )
        .unwrap();
        assert_eq!(cfg.id.as_str(), "https-docker-example-com-2376");
        match cfg.endpoint {
            EngineEndpoint::Tcp { tls: Some(t), .. } => {
                assert_eq!(t.key, std::path::PathBuf::from("/c/key.pem"));
                assert!(t.verify);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn eng_105_validation_messages() {
        let e = build(
            &schema("TCP"),
            &values(&[("host", "tcp://x"), ("port", "0")]),
            "",
        )
        .unwrap_err();
        assert_eq!(e.get("host"), Some(&s::INVALID_HOST));
        assert_eq!(e.get("port"), Some(&s::INVALID_PORT));
        let e = build(&schema("TCP + TLS"), &values(&[]), "").unwrap_err();
        for k in ["host", "port", "ca", "cert", "key"] {
            assert_eq!(e.get(k), Some(&s::FIELD_REQUIRED), "{k}");
        }
        assert!(!e.contains_key("verify"));
    }

    #[test]
    fn eng_105_wsl_and_wslc_mapping() {
        let wsl = schema("WSL distro");
        let cfg = build(
            &wsl,
            &values(&[("distro", "Ubuntu"), ("mode", "bridge")]),
            "",
        )
        .unwrap();
        assert_eq!(
            cfg.endpoint,
            EngineEndpoint::WslDistro {
                distro: "Ubuntu".into(),
                mode: WslMode::DialStdio
            }
        );
        let e = build(&wsl, &values(&[("distro", "Ubuntu"), ("mode", "tcp")]), "").unwrap_err();
        assert_eq!(e.get("port"), Some(&s::INVALID_PORT));
        let cfg = build(
            &wsl,
            &values(&[("distro", "Ubuntu"), ("mode", "tcp"), ("port", "2375")]),
            "",
        )
        .unwrap();
        assert_eq!(
            cfg.endpoint,
            EngineEndpoint::WslDistro {
                distro: "Ubuntu".into(),
                mode: WslMode::Tcp { port: 2375 }
            }
        );
        let wslc = schema("WSL containers");
        let transport = wslc.fields.iter().find(|f| f.key == "transport").unwrap();
        assert_eq!(default_value(transport), "Auto");
        let cfg = build(&wslc, &values(&[("session", ""), ("transport", "CLI")]), "").unwrap();
        assert_eq!(
            cfg.endpoint,
            EngineEndpoint::Wslc {
                session: None,
                transport: WslcTransportPref::Cli
            }
        );
    }

    #[test]
    fn slug_matches_hub_rules() {
        assert_eq!(slug("My Remote!").as_str(), "my-remote");
        assert_eq!(slug("***").as_str(), "engine");
    }
}
