//! Image, volume, and network ops; `run_image`; `disk_usage` (spec 21 §6, IMG-004/007).

use std::collections::HashMap;

use bollard::auth::DockerCredentials;
use bollard::models::{
    ContainerCreateBody, CreateImageInfo, HostConfig, Mount, MountType, PortBinding,
    VolumeCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListImagesOptions,
    ListNetworksOptions, ListVolumesOptions, PruneImagesOptionsBuilder, PruneNetworksOptions,
    PruneVolumesOptionsBuilder, RemoveImageOptionsBuilder, RemoveVolumeOptionsBuilder,
    StartContainerOptions, TagImageOptionsBuilder,
};
use dk_core::docker_json;
use dk_core::validate::{validate_env_key, validate_id_or_name, validate_image_ref, validate_name};
use dk_core::{
    ContainerQuery, DiskUsage, EngineError, EngineResult, EngineStream, ImageDeleteItem,
    ImageDetails, ImageLayer, ImageSummary, MountKind, NetworkDetails, NetworkSummary, PruneReport,
    PullProgress, RegistryAuth, ResourceKind, RunSpec, VolumeDetails, VolumeSpec, VolumeSummary,
    error_stream,
};
use futures::{Stream, StreamExt, stream};
use serde_json::Value;

use crate::DockerEngine;
use crate::containers::report;
use crate::errors::{ErrCtx, map_err};
use crate::registry_auth;

fn bad_request(message: impl Into<String>) -> EngineError {
    EngineError::Api {
        status: 400,
        message: message.into(),
    }
}

/// `alpine` → `("alpine", "latest")`, `localhost:5000/a:1` → `("localhost:5000/a", "1")`,
/// `a@sha256:…` → `("a", "sha256:…")`. Without a tag Docker would pull *all* tags.
pub(crate) fn split_pull_ref(reference: &str) -> (String, String) {
    if let Some((name, digest)) = reference.split_once('@') {
        let name = strip_tag(name);
        return (name.to_owned(), digest.to_owned());
    }
    let last_slash = reference.rfind('/').map_or(0, |i| i + 1);
    match reference[last_slash..].rfind(':') {
        Some(i) => (
            reference[..last_slash + i].to_owned(),
            reference[last_slash + i + 1..].to_owned(),
        ),
        None => (reference.to_owned(), "latest".to_owned()),
    }
}

fn strip_tag(name: &str) -> &str {
    let last_slash = name.rfind('/').map_or(0, |i| i + 1);
    match name[last_slash..].rfind(':') {
        Some(i) => &name[..last_slash + i],
        None => name,
    }
}

/// IMG-007: the 401 message.
pub(crate) fn auth_required(reference: &str) -> EngineError {
    EngineError::Api {
        status: 401,
        message: format!(
            "Authentication required: run `docker login {}`",
            registry_auth::registry_of(reference)
        ),
    }
}

/// Registry auth failures surface as 401, as in-stream `unauthorized`, or (ghcr, Hub) as
/// 500 `error from registry: denied` / `pull access denied … may require 'docker login'`.
fn looks_unauthorized(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("unauthorized")
        || m.contains("authentication required")
        || m.contains("pull access denied")
        || m.contains("docker login")
        || m.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w == "denied")
}

/// Map a pull error (HTTP status or in-stream `errorDetail`).
fn map_pull_err(e: bollard::errors::Error, ctx: &ErrCtx, reference: &str) -> EngineError {
    match &e {
        bollard::errors::Error::DockerResponseServerError {
            status_code: 401, ..
        } => return auth_required(reference),
        bollard::errors::Error::DockerResponseServerError { message, .. }
        | bollard::errors::Error::DockerStreamError { error: message }
            if looks_unauthorized(message) =>
        {
            return auth_required(reference);
        }
        _ => {}
    }
    map_err(e, ctx, None)
}

/// One progress message → DTO; `digest` collects `Digest: sha256:…`.
pub(crate) fn pull_progress(
    info: &CreateImageInfo,
    digest: &mut Option<String>,
) -> Option<PullProgress> {
    let status = info.status.as_deref().unwrap_or("").trim();
    if let Some(d) = status.strip_prefix("Digest:") {
        *digest = Some(d.trim().to_owned());
        return Some(PullProgress::Status(status.to_owned()));
    }
    let id = info.id.as_deref().unwrap_or("").trim();
    if status.is_empty() {
        return None;
    }
    // `Pulling from library/alpine` carries the tag as `id`: that's a status line.
    if id.is_empty() || status.starts_with("Pulling from") {
        let text = if id.is_empty() || !status.starts_with("Pulling from") {
            status.to_owned()
        } else {
            format!("{id}: {status}")
        };
        return Some(PullProgress::Status(text));
    }
    let detail = info.progress_detail.as_ref();
    let pos = |n: Option<i64>| n.and_then(|n| u64::try_from(n).ok()).filter(|n| *n > 0);
    Some(PullProgress::Layer {
        id: id.to_owned(),
        status: status.to_owned(),
        current: pos(detail.and_then(|d| d.current)),
        total: pos(detail.and_then(|d| d.total)),
    })
}

fn to_credentials(auth: RegistryAuth) -> DockerCredentials {
    // NFR-020: never logged; DockerCredentials goes straight into the request header.
    DockerCredentials {
        username: auth.username,
        password: auth.password.map(|p| p.expose().to_owned()),
        serveraddress: (!auth.server.is_empty()).then_some(auth.server),
        identitytoken: auth.identity_token.map(|t| t.expose().to_owned()),
        ..Default::default()
    }
}

fn port_key(private: u16, proto: dk_core::Proto) -> String {
    format!("{private}/{proto}")
}

/// `RunSpec` → create body (pure; tested).
pub(crate) fn create_body(spec: &RunSpec) -> EngineResult<ContainerCreateBody> {
    validate_image_ref(&spec.image)?;
    let mut env = Vec::with_capacity(spec.env.len());
    for (k, v) in &spec.env {
        validate_env_key(k)?;
        env.push(format!("{k}={v}"));
    }
    for k in spec.labels.keys() {
        validate_env_key(k)?;
    }
    let mut exposed = Vec::new();
    let mut bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
    for p in &spec.ports {
        if p.private == 0 {
            return Err(bad_request("invalid container port 0"));
        }
        let key = port_key(p.private, p.proto);
        if !exposed.contains(&key) {
            exposed.push(key.clone());
        }
        if p.public.is_some() || p.ip.is_some() {
            bindings
                .entry(key)
                .or_insert_with(|| Some(Vec::new()))
                .get_or_insert_with(Vec::new)
                .push(PortBinding {
                    host_ip: p.ip.map(|ip| ip.to_string()),
                    host_port: Some(p.public.map(|n| n.to_string()).unwrap_or_default()),
                });
        }
    }
    let mut mounts = Vec::with_capacity(spec.mounts.len());
    for m in &spec.mounts {
        if m.target.is_empty() || m.target.contains('\0') {
            return Err(bad_request("invalid mount target"));
        }
        let typ = match m.kind {
            MountKind::Volume => {
                if !m.source.is_empty() {
                    validate_name(&m.source)?;
                }
                MountType::VOLUME
            }
            MountKind::Bind => {
                if m.source.is_empty() || m.source.contains('\0') {
                    return Err(bad_request("invalid bind mount source"));
                }
                MountType::BIND
            }
            MountKind::Tmpfs => MountType::TMPFS,
            MountKind::Npipe => MountType::NPIPE,
            MountKind::Unknown => return Err(bad_request("unsupported mount type")),
        };
        mounts.push(Mount {
            target: Some(m.target.clone()),
            source: (!m.source.is_empty()).then(|| m.source.clone()),
            typ: Some(typ),
            read_only: Some(m.read_only),
            ..Default::default()
        });
    }
    Ok(ContainerCreateBody {
        image: Some(spec.image.clone()),
        env: (!env.is_empty()).then_some(env),
        cmd: spec.cmd.clone().filter(|c| !c.is_empty()),
        labels: (!spec.labels.is_empty()).then(|| {
            spec.labels
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        }),
        exposed_ports: (!exposed.is_empty()).then_some(exposed),
        host_config: Some(HostConfig {
            port_bindings: (!bindings.is_empty()).then_some(bindings),
            mounts: (!mounts.is_empty()).then_some(mounts),
            auto_remove: Some(spec.auto_remove),
            ..Default::default()
        }),
        ..Default::default()
    })
}

impl DockerEngine {
    pub(crate) async fn list_images_impl(&self) -> EngineResult<Vec<ImageSummary>> {
        let list = self
            .docker
            .list_images(None::<ListImagesOptions>)
            .await
            .map_err(|e| self.err(e))?;
        list.iter()
            .map(|i| Self::to_value(i).map(|v| docker_json::image_summary(&v)))
            .collect()
    }

    pub(crate) async fn inspect_image_impl(&self, id: &str) -> EngineResult<ImageDetails> {
        validate_image_ref(id)?;
        let i = self
            .docker
            .inspect_image(id)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Image, id))?;
        docker_json::image_details(&Self::to_value(&i)?)
    }

    pub(crate) async fn image_history_impl(&self, id: &str) -> EngineResult<Vec<ImageLayer>> {
        validate_image_ref(id)?;
        let h = self
            .docker
            .image_history(id)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Image, id))?;
        h.iter()
            .map(|l| Self::to_value(l).map(|v| docker_json::image_layer(&v)))
            .collect()
    }

    pub(crate) fn pull_image_stream(
        &self,
        reference: &str,
        auth: Option<RegistryAuth>,
    ) -> EngineStream<PullProgress> {
        if let Err(e) = validate_image_ref(reference) {
            return error_stream(e);
        }
        let docker = self.docker.clone();
        let ctx = self.ctx.clone();
        let reference = reference.to_owned();
        let fut = async move {
            // IMG-007: fall back to the user's Docker config when the caller has no auth.
            let auth = match auth {
                Some(a) => Some(a),
                None => registry_auth::resolve_auth(&reference).await,
            };
            let (from_image, tag) = split_pull_ref(&reference);
            let opts = CreateImageOptionsBuilder::new()
                .from_image(&from_image)
                .tag(&tag)
                .build();
            let inner = docker.create_image(Some(opts), None, auth.map(to_credentials));
            pull_stream(inner, ctx, reference)
        };
        Box::pin(stream::once(fut).flatten())
    }

    pub(crate) async fn remove_image_impl(
        &self,
        id: &str,
        force: bool,
    ) -> EngineResult<Vec<ImageDeleteItem>> {
        validate_image_ref(id)?;
        let opts = RemoveImageOptionsBuilder::new().force(force).build();
        let items = self
            .docker
            .remove_image(id, Some(opts), None)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Image, id))?;
        Ok(items
            .into_iter()
            .flat_map(|i| {
                let mut out = Vec::new();
                if let Some(u) = i.untagged {
                    out.push(ImageDeleteItem::Untagged(u));
                }
                if let Some(d) = i.deleted {
                    out.push(ImageDeleteItem::Deleted(d));
                }
                out
            })
            .collect())
    }

    pub(crate) async fn prune_images_impl(&self, dangling_only: bool) -> EngineResult<PruneReport> {
        let mut filters = HashMap::new();
        filters.insert(
            "dangling".to_owned(),
            vec![if dangling_only { "true" } else { "false" }.to_owned()],
        );
        let opts = PruneImagesOptionsBuilder::new().filters(&filters).build();
        let r = self
            .docker
            .prune_images(Some(opts))
            .await
            .map_err(|e| self.err(e))?;
        let deleted = r.images_deleted.map(|items| {
            items
                .into_iter()
                .filter_map(|i| i.deleted.or(i.untagged))
                .collect()
        });
        Ok(report(deleted, r.space_reclaimed))
    }

    pub(crate) async fn tag_image_impl(&self, id: &str, repo: &str, tag: &str) -> EngineResult<()> {
        validate_image_ref(id)?;
        validate_image_ref(repo)?;
        let tag = if tag.is_empty() { "latest" } else { tag };
        validate_image_ref(&format!("{repo}:{tag}"))?;
        let opts = TagImageOptionsBuilder::new().repo(repo).tag(tag).build();
        self.docker
            .tag_image(id, Some(opts))
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Image, id))
    }

    pub(crate) async fn run_image_impl(&self, spec: RunSpec) -> EngineResult<String> {
        let body = create_body(&spec)?;
        let mut opts = CreateContainerOptionsBuilder::new();
        if let Some(name) = spec.name.as_deref().filter(|n| !n.is_empty()) {
            validate_name(name)?;
            opts = opts.name(name);
        }
        let created = self
            .docker
            .create_container(Some(opts.build()), body)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Image, &spec.image))?;
        self.docker
            .start_container(&created.id, None::<StartContainerOptions>)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Container, &created.id))?;
        Ok(created.id)
    }

    // ── volumes ────────────────────────────────────────────────────────

    pub(crate) async fn list_volumes_impl(&self) -> EngineResult<Vec<VolumeSummary>> {
        let r = self
            .docker
            .list_volumes(None::<ListVolumesOptions>)
            .await
            .map_err(|e| self.err(e))?;
        r.volumes
            .unwrap_or_default()
            .iter()
            .map(|v| Self::to_value(v).map(|v| docker_json::volume_summary(&v)))
            .collect()
    }

    pub(crate) async fn inspect_volume_impl(&self, name: &str) -> EngineResult<VolumeDetails> {
        validate_name(name)?;
        let v = self
            .docker
            .inspect_volume(name)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Volume, name))?;
        let raw = Self::to_value(&v)?;
        let containers = self
            .list_containers_impl(ContainerQuery {
                all: true,
                size: false,
                label_filter: Vec::new(),
            })
            .await?;
        let summary = docker_json::volume_summary(&raw);
        let used_by = docker_json::volume_used_by(&summary.name, &containers);
        Ok(VolumeDetails {
            options: raw
                .get("Options")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
                        .collect()
                })
                .unwrap_or_default(),
            status: raw.get("Status").filter(|s| !s.is_null()).cloned(),
            summary,
            used_by,
            raw,
        })
    }

    pub(crate) async fn create_volume_impl(&self, spec: VolumeSpec) -> EngineResult<VolumeSummary> {
        if let Some(name) = spec.name.as_deref().filter(|n| !n.is_empty()) {
            validate_name(name)?;
        }
        if let Some(driver) = spec.driver.as_deref()
            && (driver.trim().is_empty() || driver.starts_with('-'))
        {
            return Err(bad_request("invalid volume driver"));
        }
        for k in spec.labels.keys().chain(spec.driver_opts.keys()) {
            validate_env_key(k)?;
        }
        let req = VolumeCreateRequest {
            name: spec.name.filter(|n| !n.is_empty()),
            driver: spec.driver,
            driver_opts: (!spec.driver_opts.is_empty())
                .then(|| spec.driver_opts.into_iter().collect()),
            labels: (!spec.labels.is_empty()).then(|| spec.labels.into_iter().collect()),
            ..Default::default()
        };
        let v = self
            .docker
            .create_volume(req)
            .await
            .map_err(|e| self.err(e))?;
        Ok(docker_json::volume_summary(&Self::to_value(&v)?))
    }

    pub(crate) async fn remove_volume_impl(&self, name: &str, force: bool) -> EngineResult<()> {
        validate_name(name)?;
        let opts = RemoveVolumeOptionsBuilder::new().force(force).build();
        self.docker
            .remove_volume(name, Some(opts))
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Volume, name))
    }

    pub(crate) async fn prune_volumes_impl(&self) -> EngineResult<PruneReport> {
        // API ≥ 1.42 prunes only anonymous volumes unless `all=true` (old behaviour).
        let opts = if self.api_at_least(1, 42) {
            let mut filters = HashMap::new();
            filters.insert("all".to_owned(), vec!["true".to_owned()]);
            Some(PruneVolumesOptionsBuilder::new().filters(&filters).build())
        } else {
            None
        };
        let r = self
            .docker
            .prune_volumes(opts)
            .await
            .map_err(|e| self.err(e))?;
        Ok(report(r.volumes_deleted, r.space_reclaimed))
    }

    pub(crate) async fn disk_usage_impl(&self) -> EngineResult<DiskUsage> {
        let df = self.docker.df(None).await.map_err(|e| self.err(e))?;
        Ok(docker_json::disk_usage(&Self::to_value(&df)?))
    }

    // ── networks ───────────────────────────────────────────────────────

    pub(crate) async fn list_networks_impl(&self) -> EngineResult<Vec<NetworkSummary>> {
        let list = self
            .docker
            .list_networks(None::<ListNetworksOptions>)
            .await
            .map_err(|e| self.err(e))?;
        list.iter()
            .map(|n| Self::to_value(n).map(|v| docker_json::network_summary(&v)))
            .collect()
    }

    pub(crate) async fn inspect_network_impl(&self, id: &str) -> EngineResult<NetworkDetails> {
        validate_id_or_name(id)?;
        let n = self
            .docker
            .inspect_network(id, None)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Network, id))?;
        docker_json::network_details(&Self::to_value(&n)?)
    }

    pub(crate) async fn remove_network_impl(&self, id: &str) -> EngineResult<()> {
        validate_id_or_name(id)?;
        self.docker
            .remove_network(id)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Network, id))
    }

    pub(crate) async fn prune_networks_impl(&self) -> EngineResult<PruneReport> {
        let r = self
            .docker
            .prune_networks(None::<PruneNetworksOptions>)
            .await
            .map_err(|e| self.err(e))?;
        Ok(report(r.networks_deleted, None))
    }
}

/// Progress stream → DTOs, then a final `Done { digest }`. Stops after the first error.
fn pull_stream<S>(inner: S, ctx: ErrCtx, reference: String) -> EngineStream<PullProgress>
where
    S: Stream<Item = Result<CreateImageInfo, bollard::errors::Error>> + Send + 'static,
{
    struct St<S> {
        inner: std::pin::Pin<Box<S>>,
        digest: Option<String>,
        finished: bool,
    }
    let st = St {
        inner: Box::pin(inner),
        digest: None,
        finished: false,
    };
    Box::pin(stream::unfold(
        (st, ctx, reference),
        |(mut st, ctx, reference)| async move {
            if st.finished {
                return None;
            }
            loop {
                match st.inner.next().await {
                    Some(Ok(info)) => {
                        if let Some(p) = pull_progress(&info, &mut st.digest) {
                            return Some((Ok(p), (st, ctx, reference)));
                        }
                    }
                    Some(Err(e)) => {
                        st.finished = true;
                        let err = map_pull_err(e, &ctx, &reference);
                        return Some((Err(err), (st, ctx, reference)));
                    }
                    None => {
                        st.finished = true;
                        let done = PullProgress::Done {
                            digest: st.digest.take(),
                        };
                        return Some((Ok(done), (st, ctx, reference)));
                    }
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::ProgressDetail;
    use dk_core::{MountRequest, PortMapping, Proto};

    fn info(
        id: Option<&str>,
        status: &str,
        cur: Option<i64>,
        total: Option<i64>,
    ) -> CreateImageInfo {
        CreateImageInfo {
            id: id.map(Into::into),
            status: Some(status.into()),
            progress_detail: Some(ProgressDetail {
                current: cur,
                total,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn img_004_pull_progress_parsing() {
        let mut digest = None;
        assert_eq!(
            pull_progress(
                &info(Some("latest"), "Pulling from library/alpine", None, None),
                &mut digest
            ),
            Some(PullProgress::Status(
                "latest: Pulling from library/alpine".into()
            ))
        );
        assert_eq!(
            pull_progress(
                &info(Some("abc"), "Downloading", Some(10), Some(100)),
                &mut digest
            ),
            Some(PullProgress::Layer {
                id: "abc".into(),
                status: "Downloading".into(),
                current: Some(10),
                total: Some(100)
            })
        );
        assert_eq!(
            pull_progress(
                &info(Some("abc"), "Pull complete", Some(0), Some(0)),
                &mut digest
            ),
            Some(PullProgress::Layer {
                id: "abc".into(),
                status: "Pull complete".into(),
                current: None,
                total: None
            })
        );
        pull_progress(&info(None, "Digest: sha256:0123", None, None), &mut digest);
        assert_eq!(digest.as_deref(), Some("sha256:0123"));
        assert_eq!(
            pull_progress(
                &info(
                    None,
                    "Status: Downloaded newer image for alpine:latest",
                    None,
                    None
                ),
                &mut digest
            ),
            Some(PullProgress::Status(
                "Status: Downloaded newer image for alpine:latest".into()
            ))
        );
        assert_eq!(
            pull_progress(&CreateImageInfo::default(), &mut digest),
            None
        );
    }

    #[test]
    fn img_004_pull_stream_ends_with_done() {
        let items = vec![
            Ok(info(Some("l1"), "Downloading", Some(1), Some(2))),
            Ok(info(None, "Digest: sha256:feed", None, None)),
        ];
        let ctx = ErrCtx {
            hint: crate::errors::HintCtx::Pipe,
            timeout: std::time::Duration::from_secs(30),
        };
        let out: Vec<_> = futures::executor::block_on(
            pull_stream(stream::iter(items), ctx.clone(), "alpine".into()).collect(),
        );
        assert_eq!(out.len(), 3);
        assert_eq!(
            out[2],
            Ok(PullProgress::Done {
                digest: Some("sha256:feed".into())
            })
        );

        let items = vec![Err(bollard::errors::Error::DockerStreamError {
            error: "unauthorized: authentication required".into(),
        })];
        let out: Vec<_> = futures::executor::block_on(
            pull_stream(stream::iter(items), ctx, "ghcr.io/me/app:1".into()).collect(),
        );
        assert_eq!(
            out,
            vec![Err(EngineError::Api {
                status: 401,
                message: "Authentication required: run `docker login ghcr.io`".into()
            })]
        );
    }

    #[test]
    fn img_007_denied_maps_to_auth_required() {
        assert!(looks_unauthorized(
            "error from registry: denied
denied"
        ));
        assert!(looks_unauthorized(
            "pull access denied for nope, repository does not exist or may require 'docker login'"
        ));
        assert!(!looks_unauthorized("manifest for alpine:nope not found"));
        assert!(!looks_unauthorized("no space left on device"));
    }

    #[test]
    fn img_split_pull_ref() {
        assert_eq!(split_pull_ref("alpine"), ("alpine".into(), "latest".into()));
        assert_eq!(
            split_pull_ref("alpine:3.20"),
            ("alpine".into(), "3.20".into())
        );
        assert_eq!(
            split_pull_ref("localhost:5000/app"),
            ("localhost:5000/app".into(), "latest".into())
        );
        assert_eq!(
            split_pull_ref("localhost:5000/app:dev"),
            ("localhost:5000/app".into(), "dev".into())
        );
        assert_eq!(
            split_pull_ref("alpine:3@sha256:abcd"),
            ("alpine".into(), "sha256:abcd".into())
        );
    }

    #[test]
    fn img_005_create_body() {
        let spec = RunSpec {
            image: "nginx:1.27".into(),
            name: Some("web".into()),
            ports: vec![
                PortMapping {
                    ip: None,
                    private: 80,
                    public: Some(8080),
                    proto: Proto::Tcp,
                },
                PortMapping {
                    ip: Some("127.0.0.1".parse().unwrap()),
                    private: 80,
                    public: Some(8081),
                    proto: Proto::Tcp,
                },
                PortMapping {
                    ip: None,
                    private: 53,
                    public: None,
                    proto: Proto::Udp,
                },
            ],
            env: vec![("A".into(), "1=2".into())],
            mounts: vec![
                MountRequest {
                    kind: MountKind::Volume,
                    source: "data".into(),
                    target: "/data".into(),
                    read_only: false,
                },
                MountRequest {
                    kind: MountKind::Bind,
                    source: "C:\\src".into(),
                    target: "/src".into(),
                    read_only: true,
                },
            ],
            auto_remove: true,
            cmd: Some(vec!["nginx".into()]),
            labels: [("app".to_string(), "web".to_string())].into(),
        };
        let b = create_body(&spec).unwrap();
        assert_eq!(b.env, Some(vec!["A=1=2".to_string()]));
        assert_eq!(
            b.exposed_ports,
            Some(vec!["80/tcp".to_string(), "53/udp".to_string()])
        );
        let hc = b.host_config.unwrap();
        let pb = hc.port_bindings.unwrap();
        assert_eq!(pb["80/tcp"].as_ref().unwrap().len(), 2);
        assert!(!pb.contains_key("53/udp"));
        let mounts = hc.mounts.unwrap();
        assert_eq!(mounts[0].typ, Some(MountType::VOLUME));
        assert_eq!(mounts[1].read_only, Some(true));
        assert_eq!(hc.auto_remove, Some(true));

        let mut bad = spec.clone();
        bad.env = vec![("-x".into(), "1".into())];
        assert!(create_body(&bad).is_err());
        let mut bad = spec.clone();
        bad.image = "--privileged".into();
        assert!(create_body(&bad).is_err());
        let mut bad = spec;
        bad.mounts[0].source = "../etc".into();
        assert!(create_body(&bad).is_err());
    }

    #[test]
    fn nfr_020_credentials_conversion() {
        let c = to_credentials(RegistryAuth {
            server: "ghcr.io".into(),
            username: Some("me".into()),
            password: Some(dk_core::SecretString::new("pw")),
            identity_token: None,
        });
        assert_eq!(c.serveraddress.as_deref(), Some("ghcr.io"));
        assert_eq!(c.password.as_deref(), Some("pw"));
    }
}
