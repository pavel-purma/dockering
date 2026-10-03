//! M1 exit check: discover local engines, connect each, and print containers, images, volumes.
//!
//! `cargo run -p dk-hub --example dump`

use std::time::{Duration, Instant};

use dk_core::{ContainerQuery, EngineState};
use dk_hub::{Config, EngineHub, HubOptions, Paths, UiState};

fn main() {
    let tmp = std::env::temp_dir().join(format!("dockering-dump-{}", std::process::id()));
    dk_hub::init_platform();
    let hub = EngineHub::start(HubOptions {
        paths: Paths::in_dir(&tmp),
        config: Config::default(),
        ui_state: UiState::default(),
        factories: None,
        discover_on_start: true,
        worker_threads: 2,
        demo: false,
    })
    .expect("hub start");

    // Let discovery finish (3 s per probe, parallel).
    std::thread::sleep(Duration::from_secs(4));
    let engines = futures::executor::block_on(hub.engines()).expect("engines");
    println!("{} engine(s) discovered", engines.len());
    for e in &engines {
        println!(
            "- {:<28} {:<40} {:?}",
            e.config.id,
            e.config.endpoint.display(),
            e.state
        );
    }

    for e in engines {
        if matches!(
            e.state,
            EngineState::Unsupported { .. } | EngineState::Stopped
        ) {
            continue;
        }
        let id = e.config.id.clone();
        let _ = futures::executor::block_on(hub.set_active(&id));
        let deadline = Instant::now() + Duration::from_secs(20);
        let connected = loop {
            let all = futures::executor::block_on(hub.engines()).unwrap_or_default();
            match all
                .iter()
                .find(|s| s.config.id == id)
                .map(|s| s.state.clone())
            {
                Some(EngineState::Connected) => break true,
                Some(EngineState::Failed { error, .. }) => {
                    println!("\n[{id}] failed: {error}");
                    break false;
                }
                _ if Instant::now() > deadline => {
                    println!("\n[{id}] timed out connecting");
                    break false;
                }
                _ => std::thread::sleep(Duration::from_millis(200)),
            }
        };
        if !connected {
            continue;
        }
        let info = futures::executor::block_on(hub.call(&id, |e| async move { e.info().await }));
        let containers = futures::executor::block_on(hub.call(&id, |e| async move {
            e.list_containers(ContainerQuery::default()).await
        }));
        let images =
            futures::executor::block_on(hub.call(&id, |e| async move { e.list_images().await }));
        let volumes =
            futures::executor::block_on(hub.call(&id, |e| async move { e.list_volumes().await }));
        match info {
            Ok(i) => println!(
                "\n[{id}] {} {} · API {} · {}/{} · transport {}",
                i.name,
                i.server_version,
                i.api_version.unwrap_or_default(),
                i.os,
                i.arch,
                i.transport.unwrap_or_default()
            ),
            Err(err) => println!("\n[{id}] info failed: {err}"),
        }
        match containers {
            Ok(cs) => {
                println!("  containers: {}", cs.len());
                for c in cs.iter().take(10) {
                    println!(
                        "    {:<32} {:<10} {}",
                        c.name,
                        c.state.label(),
                        c.compose.as_ref().map(|x| x.project.as_str()).unwrap_or("")
                    );
                }
            }
            Err(err) => println!("  containers: error {err}"),
        }
        println!(
            "  images: {}   volumes: {}",
            images
                .map(|v| v.len().to_string())
                .unwrap_or_else(|e| e.to_string()),
            volumes
                .map(|v| v.len().to_string())
                .unwrap_or_else(|e| e.to_string())
        );
    }
    let after = futures::executor::block_on(hub.engines()).unwrap_or_default();
    println!(
        "
after connecting (ENG-009 de-duplication by daemon id):"
    );
    for e in &after {
        println!(
            "- {:<28} also reachable via: {:?}",
            e.config.id, e.also_reachable_via
        );
    }
    hub.shutdown();
    let _ = std::fs::remove_dir_all(&tmp);
}
