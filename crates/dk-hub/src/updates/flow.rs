//! The updater's work (UPD-001…012, UPD-017): start (restore before any network, the AppImage
//! staging sweep), the schedule, one check with its download, the retained offer's life cycle,
//! and cleanup.
//!
//! Every change to the retained offer goes through [`commit`]: it persists `offer.json` first,
//! then swaps the in-memory offer, so a crash leaves the old or the new offer on disk (UPD-012).
//! While an apply runs a commit is **held** and reconciled when the apply ends (UPD-011).

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;

use dk_core::{EngineError, EngineResult};
use dk_update::offer::{self, CheckOutcome, NewerManifest, OfferChange, OfferStage as Stored};
use dk_update::source::release_asset_name;
use dk_update::{InstallKind, PlatformAsset, UpdateError};
use futures::FutureExt;
use tokio::sync::OwnedMutexGuard;
use tokio_util::sync::CancellationToken;

use super::model::{Change, Offer, State, apply_change, next_delay, rfc3339};
use super::{DisabledReason, OfferMode, UpdateCheck, UpdateStatus, disabled_reason};
use crate::hub::{HubInner, lock};

/// How often the expiry timer looks at the wall clock again at the latest (see `expiry`).
const EXPIRY_RECHECK: Duration = Duration::from_secs(3600);

/// Why updates are off right now, if they are: [`disabled_reason`] plus the trust keys. A build
/// with no keys fails closed before any network access (UPD-005) and, like a policy, hides the
/// retained offer without touching its file (UPD-017 (8)).
pub(crate) fn current_disabled(inner: &HubInner, u: &State) -> Option<DisabledReason> {
    let setting = lock(&inner.config).updates.check;
    match disabled_reason(true, inner.demo, setting) {
        None | Some(DisabledReason::Setting) if u.keys.is_empty() => {
            Some(DisabledReason::Unavailable)
        }
        other => other,
    }
}

fn idle_status(inner: &HubInner) -> UpdateStatus {
    UpdateStatus::Idle {
        last_check: inner.ui_state().updates.last_check,
    }
}

fn engine_err(e: UpdateError) -> EngineError {
    EngineError::protocol(e.to_string())
}

/// Runs blocking work on a hub blocking worker.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, UpdateError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| UpdateError::Io(e.to_string()))
}

// ── start ──────────────────────────────────────────────────────────────────────────────────

/// Called from `HubInner::start_with`. A hard disable (policy, env, demo, no keys) stops here:
/// no restore, no cleanup, no network, and the offer file stays as it is (UPD-017 (8)).
/// Otherwise the retained offer is restored *before* any check can run, then the schedule starts
/// (unless only the setting is off).
pub(crate) fn start(inner: &Arc<HubInner>) {
    let Some(u) = inner.updates.as_ref() else {
        return;
    };
    let reason = current_disabled(inner, u);
    u.set_disabled(reason, idle_status(inner));
    if reason.is_some_and(DisabledReason::is_hard) {
        return;
    }
    // Nobody has the hub handle yet, so this can't fail: the check lock is held from before the
    // first check could ask for it until the restore is done (UPD-017 (1)).
    let busy = u.busy.clone().try_lock_owned().ok();
    inner.handle.spawn(init(inner.clone(), busy));
    inner.handle.spawn(expiry(inner.clone()));
}

async fn init(inner: Arc<HubInner>, busy: Option<OwnedMutexGuard<()>>) {
    let Some(u) = inner.updates.as_ref() else {
        return;
    };
    let busy = match busy {
        Some(guard) => guard,
        None => u.busy.clone().lock_owned().await,
    };
    restore(&inner, u).await;
    cleanup(&inner, u).await;
    sweep_staging(u).await;
    drop(busy);
    if current_disabled(&inner, u).is_none() {
        start_schedule(&inner);
    }
}

/// An AppImage staging file older than this is a crash's leftover (UPD-012, UPD-014).
const STAGING_MAX_AGE: Duration = Duration::from_secs(3600);

/// UPD-012: an AppImage install removes its own stale staging files, `.dockering-update-*` older
/// than one hour next to the AppImage. Only an AppImage install does anything: the kind and the
/// file are probed here on a blocking worker, like at every check. Best effort: a failure is
/// logged at debug level and never delays or fails the start.
async fn sweep_staging(u: &State) {
    let host = u.host.clone();
    let swept = blocking(move || {
        if host.install_kind() != InstallKind::AppImage {
            return None;
        }
        let dir = host.appimage_target()?.parent()?.to_path_buf();
        let removed = dk_update::apply::sweep_stale_staging(&dir, STAGING_MAX_AGE, host.now());
        Some((dir, removed))
    })
    .await;
    match swept {
        Ok(Some((dir, removed))) => {
            tracing::debug!(dir = %dir.display(), removed, "updater: swept stale AppImage staging files");
        }
        Ok(None) => {}
        Err(error) => tracing::debug!(%error, "updater: the staging sweep didn't run"),
    }
}

/// UPD-017 (1): the retained offer comes back from `offer.json` with no network. The signature,
/// version, platform entry and freshness are checked by [`offer::restore`]; a file it rejects is
/// useless and deleted. An install offer is *Ready* only when its payload still verifies, else it
/// stays *Downloading* (no button) until a permitted check or *Check now* fetches it.
async fn restore(inner: &Arc<HubInner>, u: &State) {
    let dir = inner.paths.updates_dir().to_path_buf();
    let host = u.host.clone();
    let keys = u.keys.clone();
    let running = u.current.clone();
    let probed = blocking({
        let dir = dir.clone();
        move || {
            let platform = host.platform();
            let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
            let restored = offer::restore(&dir, &key_refs, &running, &platform, host.now());
            if restored.is_none()
                && !keys.is_empty()
                && offer::read(&dir).is_ok_and(|o| o.is_some())
            {
                // Bad signature, the running version caught up, no entry for this platform,
                // expired: nothing in the file is worth keeping (UPD-017 (2), (5)).
                offer::discard(&dir);
            }
            (restored, platform, host.install_kind())
        }
    })
    .await;
    let Ok((Some(stored), platform, kind)) = probed else {
        return;
    };
    let Some(mut offer) = Offer::from_stored(stored, &platform, kind) else {
        return;
    };
    if offer.mode == OfferMode::Install && offer.stored.stage == Stored::Downloading {
        let version = offer.version.to_string();
        if let Ok(path) = payload_file(&dir, &version, &offer.asset)
            && dk_update::verify::verify_file(&path, &offer.asset.sha256, offer.asset.size)
                .await
                .is_ok()
        {
            offer.stored.stage = Stored::Ready;
            offer.payload = Some(path);
        }
    }
    let mut core = lock(&u.core);
    core.offer = Some(offer);
    u.publish_locked(&core);
}

/// Re-evaluates the gates after a Settings change (`[updates] check`). Only the schedule and the
/// activity change: the retained offer stays, and an apply is never interrupted.
pub(crate) fn settings_changed(inner: &Arc<HubInner>) {
    let Some(u) = inner.updates.as_ref() else {
        return;
    };
    let reason = current_disabled(inner, u);
    if reason.is_some()
        && let Some(token) = lock(&u.schedule).take()
    {
        token.cancel();
    }
    u.set_disabled(reason, idle_status(inner));
    if reason.is_none() {
        start_schedule(inner);
    }
}

/// Starts the automatic schedule unless it already runs.
fn start_schedule(inner: &Arc<HubInner>) {
    let Some(u) = inner.updates.as_ref() else {
        return;
    };
    let token = {
        let mut slot = lock(&u.schedule);
        if slot.as_ref().is_some_and(|t| !t.is_cancelled()) {
            return;
        }
        let token = inner.shutdown.child_token();
        *slot = Some(token.clone());
        token
    };
    inner.handle.spawn(schedule(inner.clone(), token));
}

async fn schedule(inner: Arc<HubInner>, token: CancellationToken) {
    let Some((mut delay, interval, jitter_secs)) = inner
        .updates
        .as_ref()
        .map(|u| (u.first_check, u.interval, u.jitter_secs))
    else {
        return;
    };
    loop {
        tokio::select! {
            _ = token.cancelled() => return,
            _ = tokio::time::sleep(delay) => {}
        }
        if let Err(e) = check(&inner, false).await {
            tracing::info!(error = %e, "automatic update check failed");
        }
        delay = next_delay(interval, jitter_secs, rand::random());
    }
}

/// Keeps the snapshot honest about time: the offer disappears when it expires (UPD-017 (2)),
/// also with checks off and in a long offline session, without waiting for another event.
async fn expiry(inner: Arc<HubInner>) {
    let Some(u) = inner.updates.as_ref() else {
        return;
    };
    loop {
        // A second of slack, so the wake-up is after the expiry, never on its edge. The wait is
        // capped: tokio timers are monotonic and don't count a system suspend, so after a long
        // sleep the wall clock is looked at again within the hour.
        let wait = u
            .next_expiry()
            .map(|d| d.min(EXPIRY_RECHECK).saturating_add(Duration::from_secs(1)));
        tokio::select! {
            biased;
            () = inner.shutdown.cancelled() => return,
            () = u.wake.notified() => {}
            () = async {
                match wait {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending().await,
                }
            } => {}
        }
        u.publish();
    }
}

// ── check ──────────────────────────────────────────────────────────────────────────────────

/// One check (+ the payload download when the install can update itself). `manual` surfaces
/// errors. A failed check never changes the offer (UPD-017 (3)).
pub(crate) async fn check(inner: &Arc<HubInner>, manual: bool) -> EngineResult<UpdateCheck> {
    let Some(u) = inner.updates.as_ref() else {
        return Ok(UpdateCheck::Disabled);
    };
    if let Some(reason) = current_disabled(inner, u) {
        // A manual check while only the setting is off still runs (Settings → *Check now*),
        // but never against policy, env, demo or missing keys.
        if reason.is_hard() || !manual {
            return Ok(UpdateCheck::Disabled);
        }
    }
    let request = manual.then(|| u.request.fetch_add(1, SeqCst) + 1);
    let superseded = || request.is_some_and(|r| u.request.load(SeqCst) != r);
    let _busy = u.busy.lock().await;
    if superseded() {
        // A newer manual check superseded this one (UPD-011).
        return Err(EngineError::Cancelled);
    }
    if !manual && lock(&u.schedule).as_ref().is_none_or(|t| t.is_cancelled()) {
        // Turned off while this automatic check waited.
        return Ok(UpdateCheck::Disabled);
    }
    u.set_activity(UpdateStatus::Checking);
    let result = match AssertUnwindSafe(run_check(inner, u)).catch_unwind().await {
        Ok(result) => result,
        Err(panic) => Err(UpdateError::Io(format!(
            "the update check crashed: {}",
            crate::hub::panic_message(&*panic)
        ))),
    };
    let stale = superseded();
    let summary = match &result {
        Ok(UpdateCheck::UpToDate) => "up to date".to_owned(),
        Ok(UpdateCheck::Available { version }) => format!("{version} available"),
        Ok(UpdateCheck::Disabled) => "disabled".to_owned(),
        Err(e) => e.to_string(),
    };
    let checked_at = rfc3339(u.host.now());
    inner.update_ui_state(|s| {
        s.updates.last_check = Some(checked_at.clone());
        s.updates.last_result = Some(summary);
    });
    cleanup(inner, u).await;
    // `Checking` and `Downloading` never outlive the check; the offer is unaffected by either.
    let activity = match (&result, manual && !stale) {
        (Err(e), true) => UpdateStatus::Error {
            message: e.to_string(),
        },
        _ => match current_disabled(inner, u) {
            Some(reason) => UpdateStatus::Disabled { reason },
            None => UpdateStatus::Idle {
                last_check: Some(checked_at),
            },
        },
    };
    u.set_activity(activity);
    if stale {
        return Err(EngineError::Cancelled);
    }
    result.map_err(engine_err)
}

async fn run_check(inner: &Arc<HubInner>, u: &State) -> Result<UpdateCheck, UpdateError> {
    // UPD-006: eligibility is probed again at every check, on a blocking worker.
    let kind = blocking({
        let host = u.host.clone();
        move || host.install_kind()
    })
    .await?;
    let (json, sig) = u.source.fetch_manifest().await?;
    let keys: Vec<&str> = u.keys.iter().map(String::as_str).collect();
    dk_update::verify::verify_manifest(&json, &sig, &keys)?;
    let platform = u.host.platform();
    let checked_at = rfc3339(u.host.now());
    let newer = NewerManifest::from_authenticated(&json, &sig, &u.current, &platform, &checked_at)?;
    let result = match &newer {
        Some(newer) => UpdateCheck::Available {
            version: newer.version.to_string(),
        },
        None => UpdateCheck::UpToDate,
    };
    let outcome = newer.map_or(CheckOutcome::NoNewer, CheckOutcome::Newer);
    let previous = lock(&u.core).offer.as_ref().map(|o| o.stored.clone());
    match offer::after_check(previous.as_ref(), &outcome, &u.current) {
        // Nothing learned, or nothing to forget.
        OfferChange::Keep => {}
        // The release is gone (or never newer): drop the offer; cleanup removes its payload.
        OfferChange::Withdraw => {
            commit(inner, Change::Withdraw).await;
        }
        // The same entry: only `confirmed_at` moves; a payload that is missing is fetched.
        OfferChange::Refresh { confirmed_at } => {
            commit(inner, Change::Refresh { confirmed_at, kind }).await;
            finish_payload(inner, u).await?;
        }
        OfferChange::Replace {
            new,
            keep_old_until_verified,
        } => {
            let Some(offer) = Offer::from_stored(new, &platform, kind) else {
                return Err(UpdateError::Manifest(
                    "the release has no entry for this platform".to_owned(),
                ));
            };
            replace(inner, u, offer, keep_old_until_verified).await?;
        }
    }
    Ok(result)
}

/// A different entry becomes the offer (UPD-017 (4), UPD-012).
///
/// - *Download* mode (this install can't install it): nothing is fetched; it replaces the offer.
/// - A higher release than the offer: its payload is downloaded and verified **first**, then
///   `offer.json` is written, then the swap; the old offer stays actionable until then.
/// - The release the offer named was pulled (a lower one is served): the old offer is
///   non-actionable at once, so the new one replaces it as *Downloading* and the payload follows.
async fn replace(
    inner: &Arc<HubInner>,
    u: &State,
    mut offer: Offer,
    keep_old_until_verified: bool,
) -> Result<(), UpdateError> {
    if offer.mode == OfferMode::Download {
        commit(inner, Change::Replace(Box::new(offer))).await;
        return Ok(());
    }
    // While an apply runs the swap is held anyway; fetching first keeps its result whole.
    let applying = lock(&u.core).applying.is_some();
    if !keep_old_until_verified && !applying {
        match commit(inner, Change::Replace(Box::new(offer.clone()))).await {
            Committed::Applied => return finish_payload(inner, u).await,
            // An apply started in between: the old offer is no longer ours to drop yet.
            Committed::Held => {}
        }
    }
    let version = offer.version.to_string();
    // The payload being applied lives at this very path (a republished entry of the same
    // version): nothing may be written there until the apply ends. The replacement is held and
    // its payload fetched when the apply is reconciled.
    if lock(&u.core).applying.as_deref() == Some(version.as_str()) {
        commit(inner, Change::Replace(Box::new(offer))).await;
        return Ok(());
    }
    match obtain(inner, u, &version, &offer.asset).await {
        Ok(path) => {
            offer.stored.stage = Stored::Ready;
            offer.payload = Some(path);
        }
        // A signed running exe rejects this installer: the offer is a *Download* one.
        Err(UpdateError::UntrustedInstaller) => offer.degrade(),
        // The old offer (if any) is untouched: a failed check changes nothing.
        Err(e) => return Err(e),
    }
    commit(inner, Change::Replace(Box::new(offer))).await;
    Ok(())
}

/// Makes the offered install payload exist and verify, then marks the offer *Ready*; a payload
/// that went missing or fails its check hides the button until it is fetched again.
async fn finish_payload(inner: &Arc<HubInner>, u: &State) -> Result<(), UpdateError> {
    let (version, asset, stage, payload) = {
        let core = lock(&u.core);
        match core.offer.as_ref() {
            Some(o)
                if o.mode == OfferMode::Install
                    && !o.applying
                    && matches!(o.stored.stage, Stored::Downloading | Stored::Ready) =>
            {
                (
                    o.version.clone(),
                    o.asset.clone(),
                    o.stored.stage,
                    o.payload.clone(),
                )
            }
            _ => return Ok(()),
        }
    };
    let name = version.to_string();
    let path = match existing_payload(inner, &name, &asset).await? {
        Some(path) => path,
        None => {
            if stage == Stored::Ready {
                commit(
                    inner,
                    Change::Downloading {
                        version: version.clone(),
                    },
                )
                .await;
            }
            download(inner, u, &name, &asset).await?
        }
    };
    match trust(u, &path).await {
        Ok(()) => {}
        Err(UpdateError::UntrustedInstaller) => {
            commit(inner, Change::Degrade { version }).await;
            return Ok(());
        }
        Err(e) => return Err(e),
    }
    if !(stage == Stored::Ready && payload.as_ref() == Some(&path)) {
        commit(
            inner,
            Change::Ready {
                version,
                payload: path,
            },
        )
        .await;
    }
    Ok(())
}

/// `<updates>/<version>/<asset file name>`.
fn payload_file(dir: &Path, version: &str, asset: &PlatformAsset) -> Result<PathBuf, UpdateError> {
    Ok(dir
        .join(version)
        .join(release_asset_name(&asset.url, version)?))
}

/// The payload when it is already on disk and verifies (no network).
async fn existing_payload(
    inner: &Arc<HubInner>,
    version: &str,
    asset: &PlatformAsset,
) -> Result<Option<PathBuf>, UpdateError> {
    let path = payload_file(inner.paths.updates_dir(), version, asset)?;
    Ok(
        dk_update::verify::verify_file(&path, &asset.sha256, asset.size)
            .await
            .is_ok()
            .then_some(path),
    )
}

/// The verified payload of `version`: the one on disk, else a download; then the trust check.
async fn obtain(
    inner: &Arc<HubInner>,
    u: &State,
    version: &str,
    asset: &PlatformAsset,
) -> Result<PathBuf, UpdateError> {
    let path = match existing_payload(inner, version, asset).await? {
        Some(path) => path,
        None => download(inner, u, version, asset).await?,
    };
    trust(u, &path).await?;
    Ok(path)
}

/// Downloads and verifies the payload of `version`; progress is the activity (UPD-008). Never
/// cleans up first: the offered payload must survive (UPD-012).
async fn download(
    inner: &Arc<HubInner>,
    u: &State,
    version: &str,
    asset: &PlatformAsset,
) -> Result<PathBuf, UpdateError> {
    let dir = inner.paths.updates_dir().join(version);
    let name = release_asset_name(&asset.url, version)?;
    u.set_activity(UpdateStatus::Downloading {
        version: version.to_owned(),
        done: 0,
        total: asset.size,
    });
    let v = version.to_owned();
    u.source
        .download(
            asset,
            &dir,
            &name,
            Box::new(move |done, total| {
                u.set_activity(UpdateStatus::Downloading {
                    version: v.clone(),
                    done,
                    total,
                });
            }),
        )
        .await
}

/// The Authenticode pin (UPD-003): a signed running exe never accepts an unsigned installer. A
/// rejected file is deleted. WinVerifyTrust blocks (and may build a certificate chain), so it
/// runs on a blocking worker.
async fn trust(u: &State, payload: &Path) -> Result<(), UpdateError> {
    let Some(exe) = u.running_exe.clone() else {
        return Ok(());
    };
    let host = u.host.clone();
    let installer = payload.to_path_buf();
    let checked = blocking(move || host.check_installer(&installer, &exe)).await?;
    if checked.is_err() {
        let _ = tokio::fs::remove_file(payload).await;
    }
    checked
}

// ── commit, hold, cleanup ──────────────────────────────────────────────────────────────────

/// Whether a [`commit`] took effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Committed {
    Applied,
    /// An apply is running: the change waits for it to end (UPD-011).
    Held,
}

/// Persists `offer.json`, then swaps the in-memory offer (UPD-012). While an apply runs a result
/// of a check is held instead, so nothing replaces or deletes the payload being applied.
pub(crate) async fn commit(inner: &Arc<HubInner>, change: Change) -> Committed {
    let inner = inner.clone();
    tokio::task::spawn_blocking(move || commit_blocking(&inner, change))
        .await
        .unwrap_or_else(|error| {
            tracing::error!(%error, "updater: committing the offer failed");
            Committed::Held
        })
}

fn commit_blocking(inner: &HubInner, change: Change) -> Committed {
    let Some(u) = inner.updates.as_ref() else {
        return Committed::Held;
    };
    // Exclusive with the start and the end of an apply, so no apply can begin between the
    // decision below and the swap.
    let _gate = lock(&u.gate);
    let next = {
        let mut core = lock(&u.core);
        let from_check = matches!(
            change,
            Change::Replace(_) | Change::Refresh { .. } | Change::Withdraw
        );
        if core.applying.is_some() {
            // The result of a check waits for the apply; a payload step aimed at the offer that
            // is being applied (or a stale one) has nothing left to do.
            if from_check {
                core.held = Some(change);
            }
            return Committed::Held;
        }
        if from_check {
            // A direct result is newer than anything held.
            core.held = None;
        }
        apply_change(core.offer.clone(), &change)
    };
    persist(inner.paths.updates_dir(), next.as_ref());
    let mut core = lock(&u.core);
    core.offer = next;
    u.publish_locked(&core);
    Committed::Applied
}

/// Writes (or removes) `offer.json`; a failure is logged, the in-memory offer still follows.
fn persist(dir: &Path, offer: Option<&Offer>) {
    let result = match offer {
        Some(offer) => offer::write(dir, &offer.stored),
        None => offer::remove(dir),
    };
    if let Err(error) = result {
        tracing::warn!(%error, "updater: couldn't save offer.json");
    }
}

/// Applies the check result that was held while an apply ran (UPD-011), once it has ended. A
/// newer direct result already cleared it.
pub(crate) async fn reconcile(inner: Arc<HubInner>) {
    let Some(u) = inner.updates.as_ref() else {
        return;
    };
    let _busy = u.busy.lock().await;
    let held = lock(&u.core).held.take();
    if let Some(change) = held {
        let fetch = matches!(change, Change::Replace(_) | Change::Refresh { .. });
        if commit(&inner, change).await == Committed::Applied
            && fetch
            && let Err(error) = finish_payload(&inner, u).await
        {
            tracing::info!(%error, "updater: fetching the payload after an apply failed");
        }
    }
    cleanup(&inner, u).await;
}

/// UPD-012: removes `*.part` files, a stale `offer.json.tmp` and the version directories outside
/// the keep set; never `offer.json`. The caller holds the check lock.
async fn cleanup(inner: &Arc<HubInner>, u: &State) {
    let dir = inner.paths.updates_dir().to_path_buf();
    let keep = u.keep_set();
    let _ = blocking(move || dk_update::cleanup::cleanup(&dir, &keep)).await;
}
