//! Memory contract of the COM transport (spec 20 §5.4 Memory): every `[out]` CoTaskMem block
//! the (fake) server hands out — strings, arrays, per-entry strings in `WSLCContainerEntry`,
//! prune results, `GetDisplayName` — is `CoTaskMemFree`d by our RAII wrappers. Checked with a
//! process-wide `IMallocSpy`, hence its own test binary (one test, no parallel neighbours).

#![cfg(windows)]

use std::sync::atomic::Ordering;

use dk_core::{ContainerQuery, Engine, EngineId};
use dk_engine_wslc::com::WslcComEngine;
use dk_engine_wslc::com::fake::{self, FakeManager};
use futures::executor::block_on;

#[test]
fn every_cotaskmem_block_is_freed() {
    // The spy needs COM initialised on the registering thread.
    let mta = std::thread::spawn(|| {
        dk_engine_wslc::init_process_com_security();
        fake::install_malloc_spy()
    })
    .join()
    .expect("thread");
    assert!(mta, "CoRegisterMallocSpy failed");

    let state = fake::sample_state();
    let e = WslcComEngine::from_manager_for_tests(
        EngineId::new("wslc-fake"),
        FakeManager::new_interface(state),
        None,
    );
    let before = fake::ALLOCATIONS.load(Ordering::SeqCst);
    for _ in 0..100 {
        block_on(e.list_containers(ContainerQuery::default())).expect("list");
        block_on(e.list_volumes_json()).expect("volumes");
    }
    // Per iteration: container 1 → 5 strings, container 2 → 4 (null Mounts), entries array,
    // ports array, volumes JSON string = 12.
    assert_eq!(fake::ALLOCATIONS.load(Ordering::SeqCst) - before, 100 * 12);
    block_on(e.self_check(None)).expect("self-check"); // ListSessions array + GetDisplayName
    block_on(e.prune_containers()).expect("prune"); // WSLCPruneContainersResults.Containers
    block_on(e.list_images()).expect("images");
    assert_eq!(fake::live_cotaskmem_blocks(), 0, "leaked CoTaskMem blocks");
}
