//! Engine contract suite (spec 21 §7) against a real Docker daemon (`--features it`).
//!
//! Target: `DOCKERING_IT_HOST` or the first reachable discovered engine.

#![cfg(feature = "it")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use dk_core::contract::{ContractOptions, run_suite};
use dk_core::{Engine, EngineFactory};
use dk_engine_docker::{DockerEngine, DockerFactory, DockerTarget};

async fn engine() -> Arc<dyn Engine> {
    if let Ok(host) = std::env::var("DOCKERING_IT_HOST")
        && let Some((scheme, rest)) = host.split_once("://")
    {
        let target = match scheme {
            "unix" => DockerTarget::Unix(rest.into()),
            "npipe" => DockerTarget::NamedPipe(rest.replace('/', "\\")),
            _ => {
                let (h, p) = rest.rsplit_once(':').expect("host:port");
                DockerTarget::Tcp {
                    host: h.into(),
                    port: p.parse().expect("port"),
                    tls: None,
                }
            }
        };
        return Arc::new(
            DockerEngine::connect("it-contract".into(), target, Default::default())
                .await
                .expect("connect"),
        );
    }
    let f = DockerFactory::new();
    for d in f.discover().await {
        if d.initial_state.is_some() {
            continue;
        }
        if let Ok(e) = f.connect(&d.config).await {
            return e;
        }
    }
    panic!("no reachable Docker engine");
}

#[tokio::test(flavor = "multi_thread")]
async fn it_contract_suite_docker() {
    let engine = engine().await;
    let report = run_suite(
        engine,
        ContractOptions {
            mutating: true,
            ..ContractOptions::default()
        },
    )
    .await;
    eprintln!(
        "contract: {} passed, {} skipped {:?}",
        report.passed.len(),
        report.skipped.len(),
        report.skipped
    );
    report.assert_ok();
}
