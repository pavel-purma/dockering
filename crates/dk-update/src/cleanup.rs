//! Startup cleanup of the downloads directory (UPD-012).

use std::path::Path;

/// Removes `*.part` files and every version directory except `keep_version` under `dir`
/// (`<data-local>/updates`). Errors are ignored: cleanup must never block startup.
pub async fn cleanup(dir: &Path, keep_version: Option<&str>) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().await.is_ok_and(|t| t.is_dir());
        if is_dir && Some(name.as_str()) == keep_version {
            remove_partials(&path).await;
        } else if is_dir {
            let _ = tokio::fs::remove_dir_all(&path).await;
        } else {
            let _ = tokio::fs::remove_file(&path).await;
        }
    }
}

async fn remove_partials(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_name().to_string_lossy().ends_with(".part") {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upd_012_cleanup_keeps_only_pending() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        for (path, body) in [
            ("0.2.0/Dockering-Setup-x64.exe", "ok"),
            ("0.2.0/Dockering-Setup-x64.exe.part", "partial"),
            ("0.1.9/Dockering-Setup-x64.exe", "old"),
            ("stray.part", "x"),
        ] {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            std::fs::write(p, body).expect("write");
        }
        cleanup(root, Some("0.2.0")).await;
        assert!(root.join("0.2.0/Dockering-Setup-x64.exe").exists());
        assert!(!root.join("0.2.0/Dockering-Setup-x64.exe.part").exists());
        assert!(!root.join("0.1.9").exists());
        assert!(!root.join("stray.part").exists());

        cleanup(root, None).await;
        assert!(!root.join("0.2.0").exists());
        cleanup(&root.join("missing"), None).await;
    }
}
