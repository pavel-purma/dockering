//! Pure parsing/validation for the Run, Pull and Tag dialogs (IMG-004/005/011, NFR-022).
//! Every value that reaches the engine is validated here first. Unit-tested.

use std::collections::BTreeMap;

use dk_core::validate::{validate_env_key, validate_image_ref, validate_name};
use dk_core::{MountKind, MountRequest, PortMapping, Proto, RunSpec};

use crate::strings as s;
use crate::ui::kv_rows::KvValue;

/// One row error per input row (`None` = fine).
pub type RowErrors = Vec<Option<&'static str>>;

pub fn check_reference(reference: &str) -> Result<(), &'static str> {
    if reference.trim().is_empty() {
        return Err(s::ERR_REQUIRED);
    }
    validate_image_ref(reference.trim()).map_err(|_| s::ERR_IMAGE_REF)
}

pub fn check_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Ok(());
    }
    validate_name(name).map_err(|_| s::ERR_NAME)
}

fn port(s_: &str) -> Result<u16, &'static str> {
    match s_.parse::<u16>() {
        Ok(p) if p > 0 => Ok(p),
        _ => Err(s::ERR_PORT),
    }
}

/// `host` : `container[/proto]` (IMG-005). The host port is required.
pub fn parse_port(host: &str, container: &str) -> Result<PortMapping, &'static str> {
    if container.is_empty() {
        return Err(s::ERR_CONTAINER_PORT);
    }
    let (cport, proto) = match container.split_once('/') {
        Some((p, proto)) => (
            p,
            match proto.to_ascii_lowercase().as_str() {
                "tcp" => Proto::Tcp,
                "udp" => Proto::Udp,
                "sctp" => Proto::Sctp,
                _ => return Err(s::ERR_PROTO),
            },
        ),
        None => (container, Proto::Tcp),
    };
    Ok(PortMapping {
        ip: None,
        private: port(cport)?,
        public: Some(port(host)?),
        proto,
    })
}

/// Whether a mount source is a host path (bind) rather than a volume name.
pub fn is_host_path(src: &str) -> bool {
    let b = src.as_bytes();
    src.starts_with('/')
        || src.starts_with('.')
        || src.starts_with('~')
        || src.starts_with("\\\\")
        || src.contains('\\')
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// `source` : `target` (+ read-only). An empty source is an anonymous volume.
pub fn parse_mount(
    source: &str,
    target: &str,
    read_only: bool,
) -> Result<MountRequest, &'static str> {
    if !target.starts_with('/') || target.contains('\0') {
        return Err(s::ERR_MOUNT_TARGET);
    }
    let kind = if source.is_empty() {
        MountKind::Volume
    } else if is_host_path(source) {
        if source.contains('\0') || source.starts_with('-') {
            return Err(s::ERR_MOUNT_SOURCE);
        }
        MountKind::Bind
    } else {
        validate_name(source).map_err(|_| s::ERR_MOUNT_SOURCE)?;
        MountKind::Volume
    };
    Ok(MountRequest {
        kind,
        source: source.to_owned(),
        target: target.to_owned(),
        read_only,
    })
}

/// Key/value rows → map, validating keys (labels, env, driver options).
pub fn parse_pairs(rows: &[KvValue]) -> (Vec<(String, String)>, RowErrors) {
    let mut out = Vec::new();
    let mut errors = Vec::with_capacity(rows.len());
    for r in rows {
        if r.is_blank() {
            errors.push(None);
            continue;
        }
        match validate_env_key(&r.key) {
            Ok(()) => {
                out.push((r.key.clone(), r.value.clone()));
                errors.push(None);
            }
            Err(_) => errors.push(Some(s::ERR_ENV_KEY)),
        }
    }
    (out, errors)
}

/// The Run dialog's inputs.
pub struct RunInput<'a> {
    pub image: &'a str,
    pub name: &'a str,
    pub ports: &'a [KvValue],
    pub env: &'a [KvValue],
    pub mounts: &'a [KvValue],
    pub auto_remove: bool,
}

/// Validation result of the Run dialog.
#[derive(Debug, Default, PartialEq)]
pub struct RunErrors {
    pub name: Option<&'static str>,
    pub ports: RowErrors,
    pub env: RowErrors,
    pub mounts: RowErrors,
}

impl RunErrors {
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.ports.iter().all(Option::is_none)
            && self.env.iter().all(Option::is_none)
            && self.mounts.iter().all(Option::is_none)
    }
}

/// Builds the `RunSpec` (IMG-005) or returns per-field errors.
pub fn run_spec(input: RunInput<'_>) -> Result<RunSpec, RunErrors> {
    let mut errors = RunErrors {
        name: check_name(input.name).err(),
        ..Default::default()
    };
    let mut ports = Vec::new();
    for r in input.ports {
        if r.is_blank() {
            errors.ports.push(None);
            continue;
        }
        match parse_port(&r.key, &r.value) {
            Ok(p) => {
                ports.push(p);
                errors.ports.push(None);
            }
            Err(e) => errors.ports.push(Some(e)),
        }
    }
    let (env, env_errors) = parse_pairs(input.env);
    errors.env = env_errors;
    let mut mounts = Vec::new();
    for r in input.mounts {
        if r.is_blank() {
            errors.mounts.push(None);
            continue;
        }
        match parse_mount(&r.key, &r.value, r.flag) {
            Ok(m) => {
                mounts.push(m);
                errors.mounts.push(None);
            }
            Err(e) => errors.mounts.push(Some(e)),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(RunSpec {
        image: input.image.to_owned(),
        name: (!input.name.is_empty()).then(|| input.name.to_owned()),
        ports,
        env,
        mounts,
        auto_remove: input.auto_remove,
        cmd: None,
        labels: BTreeMap::new(),
    })
}

/// IMG-011: `repo` + `tag` → validated pair (`tag` defaults to `latest`).
pub fn tag_target(repo: &str, tag: &str) -> Result<(String, String), &'static str> {
    let repo = repo.trim();
    let tag = if tag.trim().is_empty() {
        "latest"
    } else {
        tag.trim()
    };
    if repo.is_empty() {
        return Err(s::ERR_REQUIRED);
    }
    if repo.contains('@')
        || repo
            .rsplit('/')
            .next()
            .is_some_and(|last| last.contains(':'))
    {
        return Err(s::ERR_IMAGE_REF);
    }
    validate_image_ref(&format!("{repo}:{tag}")).map_err(|_| s::ERR_IMAGE_REF)?;
    Ok((repo.to_owned(), tag.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(k: &str, v: &str, flag: bool) -> KvValue {
        KvValue {
            key: k.into(),
            value: v.into(),
            flag,
        }
    }

    #[test]
    fn img_005_ports() {
        let p = parse_port("8080", "80").unwrap();
        assert_eq!((p.public, p.private, p.proto), (Some(8080), 80, Proto::Tcp));
        let p = parse_port("53", "53/udp").unwrap();
        assert_eq!(p.proto, Proto::Udp);
        assert_eq!(parse_port("", "80"), Err(s::ERR_PORT));
        assert_eq!(parse_port("x", "80"), Err(s::ERR_PORT));
        assert_eq!(parse_port("1", "80/foo"), Err(s::ERR_PROTO));
        assert_eq!(parse_port("1", ""), Err(s::ERR_CONTAINER_PORT));
        assert_eq!(parse_port("0", "80"), Err(s::ERR_PORT));
    }

    #[test]
    fn img_005_mounts() {
        let m = parse_mount("data", "/var/lib/data", true).unwrap();
        assert_eq!((m.kind, m.read_only), (MountKind::Volume, true));
        assert_eq!(
            parse_mount("/srv", "/srv", false).unwrap().kind,
            MountKind::Bind
        );
        assert_eq!(
            parse_mount("C:\\x", "/x", false).unwrap().kind,
            MountKind::Bind
        );
        assert_eq!(
            parse_mount("", "/x", false).unwrap().kind,
            MountKind::Volume
        );
        assert_eq!(parse_mount("a b", "/x", false), Err(s::ERR_MOUNT_SOURCE));
        assert_eq!(parse_mount("v", "rel", false), Err(s::ERR_MOUNT_TARGET));
    }

    #[test]
    fn img_005_run_spec_and_errors() {
        let ports = [kv("8080", "80", false), kv("", "", false)];
        let env = [kv("A", "1", false)];
        let mounts = [kv("data", "/data", true)];
        let spec = run_spec(RunInput {
            image: "nginx:1.27",
            name: "web",
            ports: &ports,
            env: &env,
            mounts: &mounts,
            auto_remove: true,
        })
        .unwrap();
        assert_eq!(spec.name.as_deref(), Some("web"));
        assert_eq!(spec.ports.len(), 1);
        assert_eq!(spec.env, vec![("A".into(), "1".into())]);
        assert!(spec.auto_remove && spec.mounts[0].read_only);
        let bad_env = [kv("A B", "1", false)];
        let err = run_spec(RunInput {
            image: "nginx",
            name: "-x",
            ports: &[],
            env: &bad_env,
            mounts: &[],
            auto_remove: false,
        })
        .unwrap_err();
        assert_eq!(err.name, Some(s::ERR_NAME));
        assert_eq!(err.env, vec![Some(s::ERR_ENV_KEY)]);
    }

    #[test]
    fn img_004_011_reference_and_tag() {
        assert!(check_reference("alpine:3.20").is_ok());
        assert_eq!(check_reference(""), Err(s::ERR_REQUIRED));
        assert_eq!(check_reference("Bad Ref"), Err(s::ERR_IMAGE_REF));
        assert_eq!(
            tag_target("me/app", ""),
            Ok(("me/app".to_owned(), "latest".to_owned()))
        );
        assert_eq!(
            tag_target("localhost:5000/app", "v1"),
            Ok(("localhost:5000/app".to_owned(), "v1".to_owned()))
        );
        assert_eq!(tag_target("app:v1", "v2"), Err(s::ERR_IMAGE_REF));
        assert_eq!(tag_target("", "v2"), Err(s::ERR_REQUIRED));
    }
}
