//! WSLC typed adaptation is private; the Inspect tab always receives the original object.
use dk_core::{ContainerDetails, EngineResult, docker_json};
use serde_json::{Value, json};

pub(crate) fn container_details(raw: &Value) -> EngineResult<ContainerDetails> {
    let mut normalized = raw.clone();
    if let Some(ports) = raw.get("Ports") {
        let object = normalized
            .as_object_mut()
            .ok_or_else(|| dk_core::EngineError::protocol("inspect is not an object"))?;
        let ns = object.entry("NetworkSettings").or_insert_with(|| json!({}));
        if ns.is_null() {
            *ns = json!({});
        }
        if let Some(ns) = ns.as_object_mut() {
            ns.entry("Ports").or_insert_with(|| ports.clone());
        }
    }
    let mut details = docker_json::container_details(&normalized)?;
    details
        .summary
        .labels
        .remove("com.microsoft.wsl.container.metadata");
    details.raw = raw.clone();
    Ok(details)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eng_133_ports_and_cdt_040_original_raw() {
        let raw = json!({"Id":"a".repeat(64),"Name":"/web","Config":{},"State":{},
            "Ports":{"80/tcp":[{"HostIp":"127.0.0.1","HostPort":"8080"},{"HostIp":"::1","HostPort":"8081"}],"53/udp":[{"HostIp":"0.0.0.0","HostPort":"5353"}],"90/tcp":null}});
        let d = container_details(&raw).expect("inspect");
        assert_eq!(d.raw, raw);
        assert_eq!(d.summary.ports.len(), 4);
        assert_eq!(d.port_bindings.len(), 4);
        assert!(d.summary.ports.iter().any(|p| p.public.is_none()));
        assert!(
            d.summary
                .ports
                .iter()
                .any(|p| p.ip.is_some_and(|ip| ip.is_ipv6()))
        );
    }
    #[test]
    fn eng_133_recorded_com_cli_inspect_raw_deep_equality() {
        let cli: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/cli/container_inspect/inspect_single_line.json"
        ))
        .expect("fixture");
        let com: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/com-3.0.1/inspect_container.json"
        ))
        .expect("fixture");
        for raw in [&cli[0], &com] {
            let d = container_details(raw).expect("mapped inspect");
            assert_eq!(&d.raw, raw);
            assert!(!d.summary.ports.is_empty());
            assert_eq!(d.summary.ports, d.port_bindings);
        }
    }
}
