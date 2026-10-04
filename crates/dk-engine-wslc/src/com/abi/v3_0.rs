//! ABI module for the internal WSLC COM interfaces at WSL **3.0.x** (`idl/3.0.1/wslc.idl`,
//! byte-identical to tags 2.9.13, 3.0.0 and 3.0.1, checked 2026-10-02).
//!
//! Every interface declares **every** method of the IDL in IDL order so vtable offsets are
//! exact. Methods we never call keep their real signature where it is cheap to spell, otherwise
//! opaque pointer parameters; either way they occupy their slot. `#[repr(C)]` structs mirror the
//! IDL field-for-field (MIDL uses natural C alignment on x64; `LPSTR` = `*mut u8`, `BOOL` = `i32`,
//! enums = `i32`).
//!
//! **Never** call through these declarations unless `abi::select()` returned this module for
//! the WSL version read from `wslservice.exe` (ADR-0003, spec 20 §5.3).

#![allow(
    non_snake_case,
    non_camel_case_types,
    clippy::upper_case_acronyms,
    // IDL signatures are fixed by the ABI (e.g. CreateRootNamespaceProcess has 7 params).
    clippy::too_many_arguments
)]

use std::ffi::c_void;

use windows::Win32::Foundation::HANDLE;
use windows::core::{BOOL, GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCSTR, PCWSTR, PSTR, PWSTR};

use crate::version::WslVersion;

/// Lowest WSL version this module was verified against (IDL identical since 2.9.13; live-tested
/// on 3.0.1 only, so the range starts at 3.0.0).
pub const VERIFIED_MIN: WslVersion = WslVersion::new(3, 0, 1, 0);
/// Highest WSL version (inclusive): any 3.0.x patch release.
pub const VERIFIED_MAX: WslVersion = WslVersion::new(3, 0, 1, 0);
/// Vendored IDL directory this module was written from.
pub const IDL_TAG: &str = "3.0.1";

// ───────────────────────────── constants ─────────────────────────────

pub const CLSID_WSLC_SESSION_MANAGER: GUID =
    GUID::from_u128(0xa9b7a1b9_0671_405c_95f1_e0612cb4ce8f);

pub const WSLC_MAX_CONTAINER_NAME_LENGTH: usize = 255;
pub const WSLC_MAX_IMAGE_NAME_LENGTH: usize = 255;
pub const WSLC_MAX_VOLUME_NAME_LENGTH: usize = 255;
pub const WSLC_MAX_VOLUME_DRIVER_LENGTH: usize = 255;
pub const WSLC_MAX_NETWORK_NAME_LENGTH: usize = 255;
pub const WSLC_MAX_IMAGE_ID_LENGTH: usize = 255;
pub const WSLC_CONTAINER_ID_LENGTH: usize = 64;
pub const WSLC_MAX_BINDING_ADDRESS_LENGTH: usize = 45;
/// `LONG_MIN`: use the container's default stop timeout.
pub const WSLC_STOP_TIMEOUT_DEFAULT: i32 = i32::MIN;
/// Wait forever for the container to stop.
pub const WSLC_STOP_TIMEOUT_NONE: i32 = -1;

// WSLCShared.idl enums/flags (values verbatim).
pub const WSLC_SIGNAL_NONE: i32 = 0;
pub const WSLC_SIGNAL_SIGKILL: i32 = 9;
pub const WSLC_FD_STDIN: i32 = 0;
pub const WSLC_FD_STDOUT: i32 = 1;
pub const WSLC_FD_STDERR: i32 = 2;
pub const WSLC_FD_TTY: i32 = 3;
pub const WSLC_LIST_IMAGES_ALL: u32 = 1;
pub const WSLC_LIST_IMAGES_DIGESTS: u32 = 2;
pub const WSLC_LIST_IMAGES_CONTAINER_COUNTS: u32 = 4;
pub const WSLC_PROCESS_FLAGS_STDIN: i32 = 1;
pub const WSLC_PROCESS_FLAGS_TTY: i32 = 2;
pub const WSLC_CONTAINER_FLAGS_RM: i32 = 1;
pub const WSLC_CONTAINER_START_FLAGS_NONE: i32 = 0;
pub const WSLC_CONTAINER_STATE_INVALID: i32 = 0;
pub const WSLC_CONTAINER_STATE_CREATED: i32 = 1;
pub const WSLC_CONTAINER_STATE_RUNNING: i32 = 2;
pub const WSLC_CONTAINER_STATE_EXITED: i32 = 3;
pub const WSLC_CONTAINER_STATE_DELETED: i32 = 4;
pub const WSLC_HANDLE_TYPE_UNKNOWN: i32 = 0;
pub const WSLC_HANDLE_TYPE_FILE: i32 = 1;
pub const WSLC_HANDLE_TYPE_PIPE: i32 = 2;
pub const WSLC_HANDLE_TYPE_SOCKET: i32 = 3;
pub const WSLC_PROCESS_STATE_UNKNOWN: i32 = 0;
pub const WSLC_PROCESS_STATE_RUNNING: i32 = 1;
pub const WSLC_PROCESS_STATE_EXITED: i32 = 2;
pub const WSLC_PROCESS_STATE_SIGNALLED: i32 = 3;
pub const WSLC_LOGS_FLAGS_FOLLOW: i32 = 1;
pub const WSLC_LOGS_FLAGS_TIMESTAMPS: i32 = 2;
pub const WSLC_DELETE_FLAGS_FORCE: i32 = 1;
pub const WSLC_DELETE_FLAGS_DELETE_VOLUMES: i32 = 2;
pub const WSLC_DELETED_IMAGE_TYPE_DELETED: i32 = 0;
pub const WSLC_DELETED_IMAGE_TYPE_UNTAGGED: i32 = 1;
pub const WSLC_DELETE_IMAGE_FLAGS_FORCE: u32 = 1;
pub const WSLC_LIST_CONTAINERS_FLAGS_ALL: u32 = 1;
pub const WSLC_LIST_CONTAINERS_FLAGS_SIZE: u32 = 2;
pub const WSLC_SESSION_STATE_RUNNING: i32 = 0;
pub const WSLC_SESSION_STATE_TERMINATED: i32 = 1;

// ───────────────────────────── structs ─────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WSLCVersion {
    pub Major: u32,
    pub Minor: u32,
    pub Revision: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WSLCImageInformation {
    pub Image: [u8; WSLC_MAX_IMAGE_NAME_LENGTH + 1],
    pub Hash: [u8; WSLC_MAX_IMAGE_ID_LENGTH + 1],
    pub Digest: [u8; 256],
    pub Size: i64,
    pub Created: i64,
    pub ParentId: [u8; 256],
    pub Containers: i64,
}

/// `[out]` key/value pair: both strings callee-allocated (`CoTaskMemFree`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KeyValuePairInformation {
    pub Key: PSTR,
    pub Value: PSTR,
}

/// `[in]` key/value pair (labels, driver options, filters).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KeyValuePair {
    pub Key: PCSTR,
    pub Value: PCSTR,
}
pub type WSLCLabel = KeyValuePair;
pub type WSLCDriverOption = KeyValuePair;
pub type WSLCFilter = KeyValuePair;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCListImagesOptions {
    pub Flags: u32,
    pub Filters: *const WSLCFilter,
    pub FiltersCount: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCStringArray {
    pub Values: *const PCSTR,
    pub Count: u32,
}

impl WSLCStringArray {
    pub const EMPTY: Self = Self {
        Values: std::ptr::null(),
        Count: 0,
    };
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCProcessOptions {
    pub CurrentDirectory: PCSTR,
    pub User: PCSTR,
    pub CommandLine: WSLCStringArray,
    pub Environment: WSLCStringArray,
    pub Flags: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCProcessStartOptions {
    pub TtyRows: u32,
    pub TtyColumns: u32,
    pub DetachKeys: PCSTR,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCNamedVolume {
    pub Name: PCSTR,
    pub ContainerPath: PCSTR,
    pub ReadOnly: BOOL,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCVolume {
    pub HostPath: PCWSTR,
    pub ContainerPath: PCSTR,
    pub ReadOnly: BOOL,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCPortMapping {
    pub HostPort: u16,
    pub ContainerPort: u16,
    /// `AF_INET` (2) / `AF_INET6` (23).
    pub Family: i32,
    /// `IPPROTO_TCP` (6) / `IPPROTO_UDP` (17).
    pub Protocol: i32,
    pub BindingAddress: [u8; WSLC_MAX_BINDING_ADDRESS_LENGTH + 1],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCTmpfsMount {
    pub Destination: PCSTR,
    pub Options: PCSTR,
}

pub const WSLC_MOUNT_TYPE_BIND: i32 = 0;
pub const WSLC_MOUNT_TYPE_VOLUME: i32 = 1;
pub const WSLC_MOUNT_TYPE_TMPFS: i32 = 2;
pub const WSLC_MOUNT_SPEC_FLAGS_CREATE_SOURCE_IF_MISSING: i32 = 4;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCMountSpec {
    pub Type: i32,
    pub Source: PCWSTR,
    pub Target: PCSTR,
    pub ReadOnly: BOOL,
    pub Flags: i32,
    pub TmpfsSizeBytes: i64,
    pub TmpfsMode: u32,
    pub TmpfsOptions: PCSTR,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCUlimit {
    pub Name: PCSTR,
    pub Soft: i64,
    pub Hard: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCNetworkConnection {
    pub NetworkName: PCSTR,
    pub Settings: *const KeyValuePair,
    pub SettingsCount: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCNetworkConnectionOptions {
    pub NetworkName: PCSTR,
    pub Settings: *const KeyValuePair,
    pub SettingsCount: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCContainerNetwork {
    pub NetworkMode: PCSTR,
    pub Networks: *const WSLCNetworkConnection,
    pub NetworksCount: u32,
    pub Settings: *const KeyValuePair,
    pub SettingsCount: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCContainerOptions {
    pub Image: PCSTR,
    pub Name: PCSTR,
    pub Entrypoint: WSLCStringArray,
    pub InitProcessOptions: WSLCProcessOptions,
    pub Volumes: *mut WSLCVolume,
    pub VolumesCount: u32,
    pub Ports: *mut WSLCPortMapping,
    pub PortsCount: u32,
    pub Labels: *const WSLCLabel,
    pub LabelsCount: u32,
    pub Flags: i32,
    pub StopSignal: i32,
    pub HostName: PCSTR,
    pub DomainName: PCSTR,
    pub DnsServers: WSLCStringArray,
    pub DnsSearchDomains: WSLCStringArray,
    pub DnsOptions: WSLCStringArray,
    pub ShmSize: i64,
    pub ContainerNetwork: WSLCContainerNetwork,
    pub Tmpfs: *const WSLCTmpfsMount,
    pub TmpfsCount: u32,
    pub NamedVolumes: *mut WSLCNamedVolume,
    pub NamedVolumesCount: u32,
    pub MemoryBytes: i64,
    pub NanoCpus: i64,
    pub Ulimits: *const WSLCUlimit,
    pub UlimitsCount: u32,
    pub StopTimeout: i32,
    pub HealthCmd: PCSTR,
    pub HealthIntervalNs: i64,
    pub HealthTimeoutNs: i64,
    pub HealthStartPeriodNs: i64,
    pub HealthRetries: i32,
    pub Mounts: *const WSLCMountSpec,
    pub MountsCount: u32,
}

/// `char[WSLC_CONTAINER_ID_LENGTH + 1]`
pub type WSLCContainerId = [u8; WSLC_CONTAINER_ID_LENGTH + 1];

/// One `ListContainers` entry. `Command`/`Status`/`Labels`/`Networks`/`Mounts` are callee
/// allocated (`CoTaskMemFree` each, then the array) and may be null.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WSLCContainerEntry {
    pub Name: [u8; WSLC_MAX_CONTAINER_NAME_LENGTH + 1],
    pub Image: [u8; WSLC_MAX_IMAGE_NAME_LENGTH + 1],
    pub Command: PSTR,
    pub Status: PSTR,
    pub Labels: PSTR,
    pub Networks: PSTR,
    pub Mounts: PSTR,
    pub Id: WSLCContainerId,
    pub StateChangedAt: i64,
    pub CreatedAt: i64,
    pub SizeRw: i64,
    pub SizeRootFs: i64,
    pub LocalVolumes: u32,
    pub State: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCContainerPortMapping {
    pub Id: WSLCContainerId,
    pub PortMapping: WSLCPortMapping,
}

/// `WSLCHandle`: a `switch_type` union of three `system_handle` kinds, all `HANDLE`-sized, so
/// the C layout is `{ i32 Type; HANDLE Handle; }`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCHandle {
    pub Type: i32,
    pub Handle: HANDLE,
}

impl Default for WSLCHandle {
    fn default() -> Self {
        Self {
            Type: WSLC_HANDLE_TYPE_UNKNOWN,
            Handle: HANDLE(std::ptr::null_mut()),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WSLCDeletedImageInformation {
    pub Image: [u8; WSLC_MAX_IMAGE_NAME_LENGTH + 1],
    pub Type: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCDeleteImageOptions {
    pub Image: PCSTR,
    pub Flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCTagImageOptions {
    pub Image: PCSTR,
    pub Repo: PCSTR,
    pub Tag: PCSTR,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCVolumeOptions {
    pub Name: PCSTR,
    pub Driver: PCSTR,
    pub DriverOpts: *const WSLCDriverOption,
    pub DriverOptsCount: u32,
    pub Labels: *const WSLCLabel,
    pub LabelsCount: u32,
}

pub type WSLCVolumeName = [u8; WSLC_MAX_VOLUME_NAME_LENGTH + 1];

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WSLCVolumeInformation {
    pub Name: WSLCVolumeName,
    pub Driver: [u8; WSLC_MAX_VOLUME_DRIVER_LENGTH + 1],
}

pub type WSLCNetworkName = [u8; WSLC_MAX_NETWORK_NAME_LENGTH + 1];

/// `[out]` struct: `Containers` is callee-allocated (`CoTaskMemFree`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCPruneContainersResults {
    pub Containers: *mut WSLCContainerId,
    pub ContainersCount: u32,
    pub SpaceReclaimed: u64,
}

impl Default for WSLCPruneContainersResults {
    fn default() -> Self {
        Self {
            Containers: std::ptr::null_mut(),
            ContainersCount: 0,
            SpaceReclaimed: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WSLCListContainersOptions {
    pub Flags: u32,
    pub Limit: i32,
    pub Filters: *const WSLCFilter,
    pub FiltersCount: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WSLCSessionListEntry {
    pub SessionId: u32,
    pub CreatorPid: u32,
    pub DisplayName: [u16; 256],
    pub Sid: [u16; 257],
}

// ───────────────────────────── callbacks ─────────────────────────────

#[windows::core::interface("5038842F-53DB-4F30-A6D0-A41B02C94AC1")]
pub unsafe trait IProgressCallback: IUnknown {
    pub fn OnProgress(&self, Status: PCSTR, Id: PCSTR, Current: u64, Total: u64) -> HRESULT;
}

#[windows::core::interface("8153ED5D-8ABB-408B-ADBE-C0F3B13E07C3")]
pub unsafe trait IWarningCallback: IUnknown {
    pub fn OnWarning(&self, Message: PCWSTR) -> HRESULT;
}

// ───────────────────────────── IWSLCProcess ─────────────────────────────

#[windows::core::interface("1AD163CD-393D-4B33-83A2-8A3F3F23E608")]
pub unsafe trait IWSLCProcess: IUnknown {
    pub fn Signal(&self, Signal: i32) -> HRESULT;
    pub fn GetExitEvent(&self, EventHandle: *mut HANDLE) -> HRESULT;
    pub fn GetStdHandle(&self, Fd: i32, Handle: *mut WSLCHandle) -> HRESULT;
    pub fn GetFlags(&self, Flags: *mut i32) -> HRESULT;
    pub fn GetPid(&self, Pid: *mut i32) -> HRESULT;
    pub fn GetState(&self, State: *mut i32, Code: *mut i32) -> HRESULT;
    pub fn ResizeTty(&self, Rows: u32, Columns: u32) -> HRESULT;
}

// ───────────────────────────── IWSLCContainer ─────────────────────────────

#[windows::core::interface("7577FE8D-DE85-471E-B870-11669986F332")]
pub unsafe trait IWSLCContainer: IUnknown {
    /* 0*/
    pub fn Attach(
        &self,
        DetachKeys: PCSTR,
        StdIn: *mut WSLCHandle,
        StdOut: *mut WSLCHandle,
        StdErr: *mut WSLCHandle,
    ) -> HRESULT;
    /* 1*/
    pub fn Stop(&self, Signal: i32, TimeoutSeconds: i32) -> HRESULT;
    /* 2*/
    pub fn Start(
        &self,
        Flags: i32,
        StartOptions: *const WSLCProcessStartOptions,
        WarningCallback: *mut c_void,
    ) -> HRESULT;
    /* 3*/
    pub fn Delete(&self, Flags: i32) -> HRESULT;
    /* 4*/
    pub fn Export(&self, TarHandle: WSLCHandle) -> HRESULT;
    /* 5*/
    pub fn GetState(&self, State: *mut i32) -> HRESULT;
    /* 6*/
    pub fn GetInitProcess(&self, Process: *mut Option<IWSLCProcess>) -> HRESULT;
    /* 7*/
    pub fn Exec(
        &self,
        Options: *const WSLCProcessOptions,
        StartOptions: *const WSLCProcessStartOptions,
        Process: *mut Option<IWSLCProcess>,
    ) -> HRESULT;
    /* 8*/
    pub fn Inspect(&self, Size: BOOL, Output: *mut PSTR) -> HRESULT;
    /* 9*/
    pub fn Logs(
        &self,
        Flags: i32,
        Stdout: *mut WSLCHandle,
        Stderr: *mut WSLCHandle,
        Since: i64,
        Until: i64,
        Tail: u64,
    ) -> HRESULT;
    /*10*/
    pub fn GetId(&self, Id: *mut u8) -> HRESULT;
    /*11*/
    pub fn GetName(&self, Name: *mut PSTR) -> HRESULT;
    /*12*/
    pub fn GetLabels(&self, Labels: *mut *mut KeyValuePairInformation, Count: *mut u32) -> HRESULT;
    /*13*/
    pub fn Kill(&self, Signal: i32) -> HRESULT;
    /*14*/
    pub fn Stats(&self, Output: *mut PSTR) -> HRESULT;
    /*15*/
    pub fn ConnectToNetwork(&self, Options: *const WSLCNetworkConnectionOptions) -> HRESULT;
    /*16*/
    pub fn DisconnectFromNetwork(&self, NetworkName: PCSTR) -> HRESULT;
    /*17*/
    pub fn UploadArchive(
        &self,
        TarHandle: WSLCHandle,
        DestPath: PCSTR,
        ContentSize: u64,
    ) -> HRESULT;
    /*18*/
    pub fn DownloadArchive(
        &self,
        SrcPath: PCSTR,
        FollowLink: BOOL,
        OutHandle: WSLCHandle,
    ) -> HRESULT;
    /*19*/
    pub fn Restart(
        &self,
        Signal: i32,
        TimeoutSeconds: i32,
        WarningCallback: *mut c_void,
    ) -> HRESULT;
}

// ───────────────────────────── IWSLCEventStream ─────────────────────────────

#[windows::core::interface("7EC66D3B-D098-4D48-B69E-69166F6C4745")]
pub unsafe trait IWSLCEventStream: IUnknown {
    pub fn GetNext(&self, CancelEvent: HANDLE, EventJson: *mut PSTR) -> HRESULT;
}

// ───────────────────────────── IWSLCSession ─────────────────────────────

#[windows::core::interface("EF0661E4-6364-40EA-B433-E2FDF11F3519")]
pub unsafe trait IWSLCSession: IUnknown {
    /* 0*/
    pub fn GetId(&self, Id: *mut u32) -> HRESULT;
    /* 1*/
    pub fn GetDisplayName(&self, DisplayName: *mut PWSTR) -> HRESULT;
    /* 2*/
    pub fn GetState(&self, State: *mut i32) -> HRESULT;
    /* 3*/
    pub fn GetTerminationEvent(&self, Event: *mut HANDLE) -> HRESULT;
    /* 4*/
    pub fn GetTerminationReason(&self, Reason: *mut i32, Details: *mut PWSTR) -> HRESULT;
    /* 5*/
    pub fn GetEvents(
        &self,
        SinceTime: i64,
        UntilTime: i64,
        Filters: *const WSLCFilter,
        FiltersCount: u32,
        Stream: *mut Option<IWSLCEventStream>,
    ) -> HRESULT;
    /* 6*/
    pub fn PullImage(
        &self,
        Image: PCSTR,
        RegistryAuthenticationInformation: PCSTR,
        AllTags: BOOL,
        ProgressCallback: *mut c_void,
        WarningCallback: *mut c_void,
    ) -> HRESULT;
    /* 7*/
    pub fn BuildImage(
        &self,
        Options: *const c_void,
        ProgressCallback: *mut c_void,
        CancelEvent: HANDLE,
    ) -> HRESULT;
    /* 8*/
    pub fn LoadImage(
        &self,
        ImageHandle: WSLCHandle,
        ContentLength: u64,
        WarningCallback: *mut c_void,
        LoadCallback: *mut c_void,
    ) -> HRESULT;
    /* 9*/
    pub fn ImportImage(
        &self,
        ImageHandle: WSLCHandle,
        ImageName: PCSTR,
        ContentLength: u64,
        WarningCallback: *mut c_void,
        ImageId: *mut PSTR,
    ) -> HRESULT;
    /*10*/
    pub fn SaveImage(
        &self,
        OutputHandle: WSLCHandle,
        ImageNameOrID: PCSTR,
        ProgressCallback: *mut c_void,
        CancelEvent: HANDLE,
    ) -> HRESULT;
    /*11*/
    pub fn SaveImages(
        &self,
        OutputHandle: WSLCHandle,
        ImageNames: *const WSLCStringArray,
        ProgressCallback: *mut c_void,
        CancelEvent: HANDLE,
    ) -> HRESULT;
    /*12*/
    pub fn ListImages(
        &self,
        Options: *const WSLCListImagesOptions,
        Images: *mut *mut WSLCImageInformation,
        Count: *mut u32,
    ) -> HRESULT;
    /*13*/
    pub fn DeleteImage(
        &self,
        Options: *const WSLCDeleteImageOptions,
        DeletedImages: *mut *mut WSLCDeletedImageInformation,
        Count: *mut u32,
    ) -> HRESULT;
    /*14*/
    pub fn TagImage(&self, Options: *const WSLCTagImageOptions) -> HRESULT;
    /*15*/
    pub fn InspectImage(&self, ImageNameOrId: PCSTR, Output: *mut PSTR) -> HRESULT;
    /*16*/
    pub fn PruneImages(
        &self,
        Filters: *const WSLCFilter,
        FiltersCount: u32,
        DeletedImages: *mut *mut WSLCDeletedImageInformation,
        DeletedImagesCount: *mut u32,
        SpaceReclaimed: *mut u64,
    ) -> HRESULT;
    /*17*/
    pub fn CreateContainer(
        &self,
        Options: *const WSLCContainerOptions,
        WarningCallback: *mut c_void,
        Container: *mut Option<IWSLCContainer>,
    ) -> HRESULT;
    /*18*/
    pub fn OpenContainer(&self, Id: PCSTR, Container: *mut Option<IWSLCContainer>) -> HRESULT;
    /*19*/
    pub fn ListContainers(
        &self,
        Options: *const WSLCListContainersOptions,
        Containers: *mut *mut WSLCContainerEntry,
        Count: *mut u32,
        Ports: *mut *mut WSLCContainerPortMapping,
        PortsCount: *mut u32,
    ) -> HRESULT;
    /*20*/
    pub fn PruneContainers(
        &self,
        Filters: *const WSLCFilter,
        FiltersCount: u32,
        Result: *mut WSLCPruneContainersResults,
    ) -> HRESULT;
    /*21*/
    pub fn CreateRootNamespaceProcess(
        &self,
        Executable: PCSTR,
        Options: *const WSLCProcessOptions,
        TtyRows: u32,
        TtyColumns: u32,
        AcquireVmLease: BOOL,
        Process: *mut Option<IWSLCProcess>,
        Errno: *mut i32,
    ) -> HRESULT;
    /*22*/
    pub fn FormatVirtualDisk(&self, Path: PCWSTR) -> HRESULT;
    /*23*/
    pub fn Terminate(&self) -> HRESULT;
    /*24*/
    pub fn MountWindowsFolder(
        &self,
        WindowsPath: PCWSTR,
        LinuxPath: PCSTR,
        ReadOnly: BOOL,
        AcquireVmLease: BOOL,
    ) -> HRESULT;
    /*25*/
    pub fn UnmountWindowsFolder(&self, LinuxPath: PCSTR, AcquireVmLease: BOOL) -> HRESULT;
    /*26*/
    pub fn MapVmPort(&self, Family: i32, WindowsPort: u16, LinuxPort: u16) -> HRESULT;
    /*27*/
    pub fn UnmapVmPort(&self, Family: i32, WindowsPort: u16, LinuxPort: u16) -> HRESULT;
    /*28*/
    pub fn GetProcessHandle(&self, ProcessHandle: *mut HANDLE) -> HRESULT;
    /*29*/
    pub fn Initialize(
        &self,
        Settings: *const c_void,
        VmFactory: *mut c_void,
        PluginNotifier: *mut c_void,
        WarningCallback: *mut c_void,
    ) -> HRESULT;
    /*30*/
    pub fn CreateVolume(
        &self,
        Options: *const WSLCVolumeOptions,
        VolumeInfo: *mut WSLCVolumeInformation,
    ) -> HRESULT;
    /*31*/
    pub fn DeleteVolume(&self, Name: PCSTR) -> HRESULT;
    /*32*/
    pub fn ListVolumes(
        &self,
        Filters: *const WSLCFilter,
        FiltersCount: u32,
        Output: *mut PSTR,
    ) -> HRESULT;
    /*33*/
    pub fn InspectVolume(&self, Name: PCSTR, Output: *mut PSTR) -> HRESULT;
    /*34*/
    pub fn Authenticate(
        &self,
        ServerAddress: PCSTR,
        Username: PCSTR,
        Password: PCSTR,
        IdentityToken: *mut PSTR,
    ) -> HRESULT;
    /*35*/
    pub fn PushImage(
        &self,
        Image: PCSTR,
        RegistryAuthenticationInformation: PCSTR,
        AllTags: BOOL,
        ProgressCallback: *mut c_void,
        WarningCallback: *mut c_void,
    ) -> HRESULT;
    /*36*/
    pub fn PruneVolumes(
        &self,
        Filters: *const WSLCFilter,
        FiltersCount: u32,
        WarningCallback: *mut c_void,
        Volumes: *mut *mut WSLCVolumeName,
        VolumesCount: *mut u32,
        SpaceReclaimed: *mut u64,
    ) -> HRESULT;
    /*37*/
    pub fn CreateNetwork(&self, Options: *const c_void, WarningCallback: *mut c_void) -> HRESULT;
    /*38*/
    pub fn DeleteNetwork(&self, Name: PCSTR) -> HRESULT;
    /*39*/
    pub fn ListNetworks(
        &self,
        Filters: *const WSLCFilter,
        FiltersCount: u32,
        Output: *mut PSTR,
    ) -> HRESULT;
    /*40*/
    pub fn InspectNetwork(&self, Name: PCSTR, Output: *mut PSTR) -> HRESULT;
    /*41*/
    pub fn PruneNetworks(
        &self,
        Filters: *const WSLCFilter,
        FiltersCount: u32,
        Networks: *mut *mut WSLCNetworkName,
        NetworksCount: *mut u32,
    ) -> HRESULT;
    /*42*/
    pub fn RegisterCrashDumpCallback(
        &self,
        Callback: *mut c_void,
        Subscription: *mut *mut c_void,
    ) -> HRESULT;
    /*43*/
    pub fn TriggerIdleTermination(&self, WasAlreadyIdle: *mut BOOL) -> HRESULT;
    /*44*/
    pub fn BeginContainerOperation(&self, Operation: *mut Option<IUnknown>) -> HRESULT;
    /*45*/
    pub fn SetNetworkFaultsForTest(&self, FailCreateInspect: BOOL) -> HRESULT;
}

// ───────────────────────────── IWSLCSessionManager ─────────────────────────────

#[windows::core::interface("82A7ABC8-6B50-43FC-AB96-15FBBE7E8760")]
pub unsafe trait IWSLCSessionManager: IUnknown {
    /*0*/
    pub fn GetVersion(&self, Version: *mut WSLCVersion) -> HRESULT;
    /*1*/
    pub fn CreateSession(
        &self,
        Settings: *const c_void,
        Flags: i32,
        WarningCallback: *mut c_void,
        Session: *mut Option<IWSLCSession>,
    ) -> HRESULT;
    /*2*/
    pub fn EnterSession(
        &self,
        DisplayName: PCWSTR,
        StoragePath: PCWSTR,
        WarningCallback: *mut c_void,
        Session: *mut Option<IWSLCSession>,
    ) -> HRESULT;
    /*3*/
    pub fn ListSessions(
        &self,
        Sessions: *mut *mut WSLCSessionListEntry,
        SessionsCount: *mut u32,
    ) -> HRESULT;
    /*4*/
    pub fn OpenSession(&self, Id: u32, Session: *mut Option<IWSLCSession>) -> HRESULT;
    /*5*/
    pub fn OpenSessionByName(
        &self,
        DisplayName: PCWSTR,
        Session: *mut Option<IWSLCSession>,
    ) -> HRESULT;
}

/// Number of methods (excluding `IUnknown`) per interface, from the IDL. Tested against the
/// generated vtable sizes so a missing/extra slot fails the build's tests.
pub const METHOD_COUNTS: &[(&str, usize)] = &[
    ("IProgressCallback", 1),
    ("IWarningCallback", 1),
    ("IWSLCProcess", 7),
    ("IWSLCContainer", 20),
    ("IWSLCEventStream", 1),
    ("IWSLCSession", 46),
    ("IWSLCSessionManager", 6),
];

/// Sanity: the vtable struct sizes implied by [`METHOD_COUNTS`].
pub fn vtable_sizes() -> Vec<(&'static str, usize)> {
    vec![
        (
            "IProgressCallback",
            std::mem::size_of::<IProgressCallback_Vtbl>(),
        ),
        (
            "IWarningCallback",
            std::mem::size_of::<IWarningCallback_Vtbl>(),
        ),
        ("IWSLCProcess", std::mem::size_of::<IWSLCProcess_Vtbl>()),
        ("IWSLCContainer", std::mem::size_of::<IWSLCContainer_Vtbl>()),
        (
            "IWSLCEventStream",
            std::mem::size_of::<IWSLCEventStream_Vtbl>(),
        ),
        ("IWSLCSession", std::mem::size_of::<IWSLCSession_Vtbl>()),
        (
            "IWSLCSessionManager",
            std::mem::size_of::<IWSLCSessionManager_Vtbl>(),
        ),
    ]
}

/// Size of the `IUnknown` part of every vtable.
pub const IUNKNOWN_VTBL_SIZE: usize = std::mem::size_of::<IUnknown_Vtbl>();

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use super::*;

    #[test]
    fn vtables_have_exactly_the_idl_slots() {
        let ptr = size_of::<usize>();
        for ((name, count), (vname, size)) in METHOD_COUNTS.iter().zip(vtable_sizes()) {
            assert_eq!(*name, vname);
            assert_eq!(size, IUNKNOWN_VTBL_SIZE + count * ptr, "{name}");
        }
    }

    #[test]
    fn slot_offsets_of_called_methods() {
        let p = size_of::<usize>();
        let base = IUNKNOWN_VTBL_SIZE;
        // Spike F-6/F-7 proved these slots live on 3.0.1.
        assert_eq!(
            offset_of!(IWSLCSessionManager_Vtbl, OpenSessionByName),
            base + 5 * p
        );
        assert_eq!(offset_of!(IWSLCSession_Vtbl, OpenContainer), base + 18 * p);
        assert_eq!(offset_of!(IWSLCSession_Vtbl, ListContainers), base + 19 * p);
        assert_eq!(offset_of!(IWSLCSession_Vtbl, ListNetworks), base + 39 * p);
        assert_eq!(
            offset_of!(IWSLCSession_Vtbl, BeginContainerOperation),
            base + 44 * p
        );
        assert_eq!(offset_of!(IWSLCContainer_Vtbl, Exec), base + 7 * p);
        assert_eq!(offset_of!(IWSLCContainer_Vtbl, Logs), base + 9 * p);
        assert_eq!(offset_of!(IWSLCContainer_Vtbl, Stats), base + 14 * p);
        assert_eq!(offset_of!(IWSLCContainer_Vtbl, Restart), base + 19 * p);
        assert_eq!(offset_of!(IWSLCProcess_Vtbl, ResizeTty), base + 6 * p);
    }

    /// x64 MIDL layouts (natural alignment), computed by hand from the IDL.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn struct_layouts_match_midl_x64() {
        assert_eq!(size_of::<WSLCVersion>(), 12);
        // 4 + 4 + 512 + 514 = 1034, padded to 4-byte alignment.
        assert_eq!(size_of::<WSLCSessionListEntry>(), 1036);
        // 256+256 names, 5 ptrs @512, Id[65] @552, pad → i64 @624, …, u32 @656, i32 @660.
        assert_eq!(offset_of!(WSLCContainerEntry, Command), 512);
        assert_eq!(offset_of!(WSLCContainerEntry, Id), 552);
        assert_eq!(offset_of!(WSLCContainerEntry, StateChangedAt), 624);
        assert_eq!(offset_of!(WSLCContainerEntry, LocalVolumes), 656);
        assert_eq!(offset_of!(WSLCContainerEntry, State), 660);
        assert_eq!(size_of::<WSLCContainerEntry>(), 664);
        assert_eq!(size_of::<WSLCPortMapping>(), 2 + 2 + 4 + 4 + 46 + 2);
        assert_eq!(offset_of!(WSLCContainerPortMapping, PortMapping), 68);
        assert_eq!(size_of::<WSLCContainerPortMapping>(), 128);
        assert_eq!(size_of::<WSLCHandle>(), 16);
        assert_eq!(offset_of!(WSLCImageInformation, Size), 768);
        assert_eq!(offset_of!(WSLCImageInformation, ParentId), 784);
        assert_eq!(size_of::<WSLCImageInformation>(), 1048);
        assert_eq!(size_of::<WSLCDeletedImageInformation>(), 260);
        assert_eq!(size_of::<WSLCVolumeInformation>(), 512);
        assert_eq!(size_of::<WSLCPruneContainersResults>(), 24);
        assert_eq!(size_of::<WSLCListContainersOptions>(), 24);
        assert_eq!(size_of::<WSLCStringArray>(), 16);
        assert_eq!(size_of::<WSLCProcessOptions>(), 56);
        assert_eq!(size_of::<WSLCProcessStartOptions>(), 16);
        assert_eq!(size_of::<WSLCVolumeOptions>(), 48);
        assert_eq!(size_of::<WSLCDeleteImageOptions>(), 16);
        assert_eq!(size_of::<WSLCTagImageOptions>(), 24);
        assert_eq!(size_of::<WSLCListImagesOptions>(), 24);
        assert_eq!(size_of::<WSLCMountSpec>(), 56);
        assert_eq!(size_of::<WSLCContainerNetwork>(), 40);
    }
}
