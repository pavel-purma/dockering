//! Pinned root filesystems, one per (image, architecture) (ENG-152, REL-089).
//!
//! `choice_id` is only ever a key into this table; it never reaches a path or an argv (NFR-022).

use dk_core::SetupChoice;

/// Digest of an entry whose asset doesn't exist yet. It can never be marked published.
pub(crate) const PLACEHOLDER_SHA256: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// Offered images, in display order.
pub(crate) const IMAGES: [&str; 2] = ["ubuntu", "alpine"];

/// One downloadable root filesystem.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pin {
    pub(crate) image: &'static str,
    pub(crate) arch: &'static str,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
    pub(crate) url: &'static str,
    pub(crate) size_bytes: u64,
    pub(crate) sha256: &'static str,
    pub(crate) disk_bytes: u64,
    /// False until the build workflow has published the asset and the constants are real.
    pub(crate) published: bool,
}

impl Pin {
    /// `dockering-engine-<image>-<arch>.tar.xz`, built from table constants only.
    pub(crate) fn file_name(&self) -> String {
        format!("dockering-engine-{}-{}.tar.xz", self.image, self.arch)
    }

    /// Published with a real digest: the only kind that is offered or downloaded.
    pub(crate) fn usable(&self) -> bool {
        self.published && is_sha256_hex(self.sha256) && self.sha256 != PLACEHOLDER_SHA256
    }

    pub(crate) fn choice(&self) -> SetupChoice {
        SetupChoice {
            id: self.image.to_owned(),
            label: self.label.to_owned(),
            description: self.description.to_owned(),
            download_bytes: self.size_bytes,
            disk_bytes: self.disk_bytes,
        }
    }
}

macro_rules! pin {
    ($image:literal, $arch:literal, $label:literal, $description:expr, $size:literal, $disk:literal) => {
        Pin {
            image: $image,
            arch: $arch,
            label: $label,
            description: $description,
            url: concat!(
                "https://github.com/pavel-purma/dockering/releases/download/rootfs-1/dockering-engine-",
                $image,
                "-",
                $arch,
                ".tar.xz"
            ),
            size_bytes: $size,
            sha256: PLACEHOLDER_SHA256,
            disk_bytes: $disk,
            published: false,
        }
    };
}

const UBUNTU: &str = "Familiar Ubuntu LTS. Larger download.";
const ALPINE: &str = "Smallest and fastest to set up.";

/// Sizes are the spike's measured x86_64 `.tar.xz` figures (W-3.5); real values, digests and
/// `published = true` arrive with the first `rootfs-<n>` release.
pub(crate) const PINS: [Pin; 4] = [
    pin!(
        "ubuntu",
        "x86_64",
        "Ubuntu",
        UBUNTU,
        355_357_616,
        1_800_000_000
    ),
    pin!(
        "ubuntu",
        "aarch64",
        "Ubuntu",
        UBUNTU,
        355_357_616,
        1_800_000_000
    ),
    pin!(
        "alpine",
        "x86_64",
        "Alpine",
        ALPINE,
        58_477_916,
        370_000_000
    ),
    pin!(
        "alpine",
        "aarch64",
        "Alpine",
        ALPINE,
        58_477_916,
        370_000_000
    ),
];

/// The pin for exactly this image id and architecture.
pub(crate) fn find<'a>(pins: &'a [Pin], image: &str, arch: &str) -> Option<&'a Pin> {
    pins.iter()
        .find(|pin| pin.image == image && pin.arch == arch)
}

/// Offered choices for `arch`: published pins only, Ubuntu first (ENG-162, A13).
pub(crate) fn choices(pins: &[Pin], arch: &str) -> Vec<SetupChoice> {
    IMAGES
        .iter()
        .filter_map(|image| find(pins, image, arch))
        .filter(|pin| pin.usable())
        .map(Pin::choice)
        .collect()
}

/// Whether `digest` is 64 lowercase hex digits.
pub(crate) fn is_sha256_hex(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
