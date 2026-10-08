use crate::topology::snapshot::{
    DomainBlockDevice, HostBlockDevice, MappingConfidence, MappingStage, VifMapping,
};
use anyhow::{Context, Result};
use std::{collections::HashMap, fs, path::Path, process::Command};

fn read(path: &str) -> Result<String> {
    match read_state(path) {
        XenstoreRead::Value(value) => Ok(value),
        XenstoreRead::Missing => anyhow::bail!("xenstore key {path} is missing"),
        XenstoreRead::Empty => anyhow::bail!("xenstore key {path} is empty"),
        XenstoreRead::Failed(error) => anyhow::bail!("{error}"),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum XenstoreRead {
    Value(String),
    Missing,
    Empty,
    Failed(String),
}

fn read_diagnostic(field: &str, result: &XenstoreRead) -> String {
    match result {
        XenstoreRead::Missing => format!("key missing ({field})"),
        XenstoreRead::Empty => format!("empty value ({field})"),
        XenstoreRead::Failed(error) => format!("read failed ({field}): {error}"),
        XenstoreRead::Value(_) => unreachable!("value has no read diagnostic"),
    }
}

fn read_state(path: &str) -> XenstoreRead {
    let output = Command::new("xenstore-read").arg(path).output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return XenstoreRead::Failed(format!("failed to read xenstore path {path}: {error}"));
        }
    };
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let lower = detail.to_ascii_lowercase();
        if lower.contains("not found")
            || lower.contains("no such file")
            || lower.contains("does not exist")
            || lower.contains("could not read")
        {
            return XenstoreRead::Missing;
        }
        return XenstoreRead::Failed(format!(
            "xenstore-read {path} failed: {}",
            if detail.is_empty() {
                "unknown error"
            } else {
                &detail
            }
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        XenstoreRead::Empty
    } else {
        XenstoreRead::Value(value)
    }
}

fn list(path: &str) -> Result<Vec<String>> {
    let output = Command::new("xenstore-list")
        .arg(path)
        .output()
        .with_context(|| format!("failed to list xenstore path {path}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "xenstore-list {path} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect())
}

fn read_xm_block_list(domid: u32) -> Result<HashMap<String, String>> {
    let output = Command::new("xm")
        .args(["block-list", &domid.to_string()])
        .output()
        .with_context(|| format!("execute xm block-list {domid}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "xm block-list {domid} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_xm_block_list(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_xm_block_list(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let columns = line.split_whitespace().collect::<Vec<_>>();
            let frontend = columns.first()?.parse::<u64>().ok()?.to_string();
            let backend = columns
                .iter()
                .rev()
                .find(|value| value.starts_with("/local/domain/"))?;
            Some((frontend, (*backend).to_string()))
        })
        .collect()
}

fn by_id_wwid(device: &str) -> Option<String> {
    let target = fs::canonicalize(format!("/dev/{device}")).ok()?;
    for entry in fs::read_dir("/dev/disk/by-id").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("scsi-") {
            continue;
        }
        if fs::canonicalize(entry.path()).ok().as_ref() == Some(&target) {
            return Some(name.trim_start_matches("scsi-").to_string());
        }
    }
    None
}

fn major_minor_candidates(value: &str) -> Vec<String> {
    let mut candidates = vec![value.to_string()];
    if let Some((major, minor)) = value.split_once(':')
        && let (Ok(major), Ok(minor)) = (
            u32::from_str_radix(major, 16),
            u32::from_str_radix(minor, 16),
        )
    {
        let decoded = format!("{major}:{minor}");
        if decoded != value {
            candidates.push(decoded);
        }
    }
    candidates
}

fn parameter_device(value: &str) -> Option<&str> {
    let value = value.rsplit_once(':').map_or(value, |(_, suffix)| {
        if suffix.starts_with("/dev/") {
            suffix
        } else {
            value
        }
    });
    value
        .strip_prefix("/dev/")
        .and_then(|value| value.rsplit('/').next())
}

fn find_host_device<'a>(
    physical_device: Option<&str>,
    backend_params: Option<&str>,
    inventory: &'a [HostBlockDevice],
) -> Option<&'a HostBlockDevice> {
    if let Some(physical_device) = physical_device {
        for candidate in major_minor_candidates(physical_device) {
            if let Some(device) = inventory
                .iter()
                .find(|device| device.major_minor.as_deref() == Some(candidate.as_str()))
            {
                return Some(device);
            }
        }
    }
    let name = backend_params.and_then(parameter_device)?;
    inventory.iter().find(|device| {
        device.name == name || device.kname == name || device.dm_name.as_deref() == Some(name)
    })
}

fn clean_wwid(value: &str) -> String {
    value
        .trim()
        .strip_prefix("0x")
        .unwrap_or(value.trim())
        .to_string()
}

fn is_file_backed_loop(host: &HostBlockDevice, backend_params: Option<&str>) -> bool {
    host.name.starts_with("loop")
        && backend_params.is_some_and(|params| parameter_device(params).is_none())
}

fn mounted_backing_device<'a>(
    backing_file: Option<&str>,
    inventory: &'a [HostBlockDevice],
) -> Option<&'a HostBlockDevice> {
    let backing_file = Path::new(backing_file?);
    inventory
        .iter()
        .filter(|device| {
            device
                .mountpoint
                .as_deref()
                .is_some_and(|mountpoint| backing_file.starts_with(Path::new(mountpoint)))
        })
        .max_by_key(|device| {
            device
                .mountpoint
                .as_deref()
                .map_or(0, |mountpoint| Path::new(mountpoint).components().count())
        })
}

fn direct_wwid(device: &HostBlockDevice) -> Option<String> {
    device
        .dm_uuid
        .as_deref()
        .and_then(|uuid| uuid.strip_prefix("mpath-"))
        .map(str::to_string)
        .or_else(|| device.wwn.as_deref().map(clean_wwid))
}

fn resolve_block_identity(
    backend: &str,
    physical_device: Option<&str>,
    backend_params: Option<&str>,
    inventory: &[HostBlockDevice],
    backend_diagnostic: Option<&str>,
) -> BlockResolution {
    resolve_block_identity_with_diagnostics(
        backend,
        physical_device,
        backend_params,
        inventory,
        backend_diagnostic,
        None,
        None,
    )
}

fn resolve_block_identity_with_diagnostics(
    backend: &str,
    physical_device: Option<&str>,
    backend_params: Option<&str>,
    inventory: &[HostBlockDevice],
    backend_diagnostic: Option<&str>,
    physical_diagnostic: Option<&str>,
    params_diagnostic: Option<&str>,
) -> BlockResolution {
    let backing_file = backend_params
        .filter(|params| parameter_device(params).is_none())
        .map(str::to_string);
    if backend.is_empty() {
        return BlockResolution::unresolved(
            MappingStage::Backend,
            backend_diagnostic.unwrap_or("xenstore backend and xm block-list evidence are absent"),
        );
    }
    if physical_device.is_none() && backend_params.is_none() {
        return BlockResolution::unresolved(
            MappingStage::PhysicalDevice,
            &format!(
                "backend has neither physical-device nor params host evidence{}{}",
                physical_diagnostic
                    .map(|reason| format!("; physical-device {reason}"))
                    .unwrap_or_default(),
                params_diagnostic
                    .map(|reason| format!("; params {reason}"))
                    .unwrap_or_default()
            ),
        );
    }
    let Some(host) = find_host_device(physical_device, backend_params, inventory) else {
        return BlockResolution {
            backing_file,
            ..BlockResolution::unresolved(
                MappingStage::HostBlock,
                &format!(
                    "no host block inventory match for physical-device={} params={}",
                    physical_device.unwrap_or("absent"),
                    backend_params.unwrap_or("absent")
                ),
            )
        };
    };

    let file_backed_loop = is_file_backed_loop(host, backend_params);
    let mut mapped = host;
    let mut confidence = MappingConfidence::Exact;
    if file_backed_loop {
        // The loop number identifies only the local loop device. For repository
        // images, correlate the backing path to the mounted host filesystem;
        // never infer an array identity from the loop number or path itself.
        if let Some(mounted) = mounted_backing_device(backend_params, inventory) {
            mapped = mounted;
            confidence = MappingConfidence::Derived;
        }
    } else if host.dm_uuid.is_none()
        && let Some(holder) = host.holders.iter().find_map(|holder| {
            inventory.iter().find(|candidate| {
                (&candidate.name == holder || &candidate.kname == holder)
                    && candidate
                        .dm_uuid
                        .as_deref()
                        .is_some_and(|uuid| uuid.starts_with("mpath-"))
            })
        })
    {
        mapped = holder;
        confidence = MappingConfidence::Derived;
    }

    let wwid = if file_backed_loop {
        // Only identity directly observed on the mounted candidate is valid.
        direct_wwid(mapped)
    } else {
        mapped
            .dm_uuid
            .as_deref()
            .and_then(|uuid| uuid.strip_prefix("mpath-"))
            .map(str::to_string)
            .or_else(|| mapped.wwn.as_deref().map(clean_wwid))
            .or_else(|| by_id_wwid(&mapped.name))
    };
    if mapped.dm_name.is_none() && mapped.dm_uuid.is_none() {
        let unresolved_reason = if file_backed_loop {
            format!(
                "host block/loop {} and backing file {} are observed, but no device-mapper/WWID evidence is available",
                host.name,
                backend_params.unwrap_or("absent")
            )
        } else {
            format!(
                "host block {} is observed but no device-mapper evidence is available",
                host.name
            )
        };
        return BlockResolution {
            host_device: Some(host.name.clone()),
            major_minor: host.major_minor.clone(),
            wwid,
            confidence: if confidence == MappingConfidence::Derived {
                MappingConfidence::Derived
            } else {
                MappingConfidence::Unknown
            },
            unresolved_stage: Some(MappingStage::DeviceMapper),
            unresolved_reason: Some(unresolved_reason),
            backing_file,
            ..Default::default()
        };
    }
    let Some(wwid) = wwid else {
        return BlockResolution {
            host_device: Some(host.name.clone()),
            major_minor: host.major_minor.clone(),
            dm_name: mapped.dm_name.clone(),
            dm_uuid: if file_backed_loop {
                mapped
                    .dm_uuid
                    .as_deref()
                    .filter(|uuid| uuid.starts_with("mpath-"))
                    .map(str::to_string)
            } else {
                mapped.dm_uuid.clone()
            },
            confidence: MappingConfidence::Unknown,
            unresolved_stage: Some(MappingStage::Wwid),
            unresolved_reason: Some(format!(
                "host block {} is observed but no host-side WWID evidence is available",
                host.name
            )),
            backing_file,
            ..Default::default()
        };
    };

    BlockResolution {
        host_device: Some(host.name.clone()),
        major_minor: host.major_minor.clone(),
        dm_name: mapped.dm_name.clone(),
        dm_uuid: mapped.dm_uuid.clone(),
        wwid: Some(wwid),
        confidence,
        backing_file,
        ..Default::default()
    }
}

#[derive(Debug, Default)]
struct BlockResolution {
    major_minor: Option<String>,
    host_device: Option<String>,
    dm_name: Option<String>,
    dm_uuid: Option<String>,
    wwid: Option<String>,
    confidence: MappingConfidence,
    backing_file: Option<String>,
    unresolved_stage: Option<MappingStage>,
    unresolved_reason: Option<String>,
}

impl BlockResolution {
    fn unresolved(stage: MappingStage, reason: &str) -> Self {
        Self {
            unresolved_stage: Some(stage),
            unresolved_reason: Some(reason.to_string()),
            ..Default::default()
        }
    }
}

pub fn read_domain_block_devices(
    domid: u32,
    inventory: &[HostBlockDevice],
) -> Result<Vec<DomainBlockDevice>> {
    let base = format!("/local/domain/{domid}/device/vbd");
    let (xm_blocks, xm_error) = match read_xm_block_list(domid) {
        Ok(blocks) => (blocks, None),
        Err(error) => (HashMap::new(), Some(error.to_string())),
    };
    let mut devices = Vec::new();
    for frontend in list(&base)? {
        let path = format!("{base}/{frontend}");
        let device = read(&format!("{path}/device")).ok();
        let virtual_device = read(&format!("{path}/virtual-device")).ok();
        let backend_read = read_state(&format!("{path}/backend"));
        let (backend, backend_diagnostic) = match backend_read {
            XenstoreRead::Value(value) => (value, None),
            XenstoreRead::Missing => (
                xm_blocks.get(&frontend).cloned().unwrap_or_default(),
                Some("key missing".to_string()),
            ),
            XenstoreRead::Empty => (
                xm_blocks.get(&frontend).cloned().unwrap_or_default(),
                Some("key empty".to_string()),
            ),
            XenstoreRead::Failed(error) => (
                xm_blocks.get(&frontend).cloned().unwrap_or_default(),
                Some(format!("read failed: {error}")),
            ),
        };
        let (physical_device, physical_diagnostic) = if backend.is_empty() {
            (None, None)
        } else {
            match read_state(&format!("{backend}/physical-device")) {
                XenstoreRead::Value(value) => (Some(value), None),
                XenstoreRead::Missing | XenstoreRead::Failed(_) => {
                    match read_state(&format!("{path}/physical-device")) {
                        XenstoreRead::Value(value) => (Some(value), None),
                        other => (None, Some(read_diagnostic("physical-device", &other))),
                    }
                }
                other => (None, Some(read_diagnostic("physical-device", &other))),
            }
        };
        let (backend_params, params_diagnostic) = if backend.is_empty() {
            (None, None)
        } else {
            match read_state(&format!("{backend}/params")) {
                XenstoreRead::Value(value) => (Some(value), None),
                other => (None, Some(read_diagnostic("params", &other))),
            }
        };
        let resolution = resolve_block_identity_with_diagnostics(
            &backend,
            physical_device.as_deref(),
            backend_params.as_deref(),
            inventory,
            backend_diagnostic.as_deref().or(xm_error.as_deref()),
            physical_diagnostic.as_deref(),
            params_diagnostic.as_deref(),
        );
        devices.push(DomainBlockDevice {
            domid,
            frontend,
            backend,
            device,
            virtual_device,
            physical_device,
            backing_file: resolution.backing_file,
            major_minor: resolution.major_minor,
            host_device: resolution.host_device,
            dm_name: resolution.dm_name,
            dm_uuid: resolution.dm_uuid,
            wwid: resolution.wwid,
            confidence: resolution.confidence,
            unresolved_stage: resolution.unresolved_stage,
            unresolved_reason: resolution.unresolved_reason,
        });
    }
    Ok(devices)
}

pub fn read_domain_vifs(domid: u32) -> Result<Vec<VifMapping>> {
    let base = format!("/local/domain/{domid}/device/vif");
    let mut vifs = Vec::new();
    for vif in list(&base)? {
        let path = format!("{base}/{vif}");
        let vif_name = format!("vif{domid}.{}", vif);
        let bridge = fs::read_link(format!("/sys/class/net/{vif_name}/brport/bridge"))
            .ok()
            .and_then(|link| {
                link.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
        vifs.push(VifMapping {
            domid,
            vif: vif_name,
            mac: read(&format!("{path}/mac")).ok(),
            bridge,
        });
    }
    Ok(vifs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multipath_inventory() -> Vec<HostBlockDevice> {
        vec![
            HostBlockDevice {
                name: "sdb".into(),
                kname: "sdb".into(),
                major_minor: Some("8:16".into()),
                holders: vec!["dm-3".into()],
                ..Default::default()
            },
            HostBlockDevice {
                name: "dm-3".into(),
                kname: "dm-3".into(),
                major_minor: Some("253:3".into()),
                dm_name: Some("mpatha".into()),
                dm_uuid: Some("mpath-36000d3100001".into()),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn parses_xm_block_list_as_supplemental_backend_evidence() {
        let parsed = parse_xm_block_list(
            "Vdev  BE handle state evt-ch ring-ref BE-path\n51712 0 0 4 8 7 /local/domain/0/backend/vbd/1859/51712\n",
        );

        assert_eq!(
            parsed.get("51712").map(String::as_str),
            Some("/local/domain/0/backend/vbd/1859/51712")
        );
    }

    #[test]
    fn resolves_hex_xen_major_minor_through_holder_to_stable_wwid() {
        let resolved = resolve_block_identity(
            "/local/domain/0/backend/vbd/1859/51712",
            Some("8:10"),
            None,
            &multipath_inventory(),
            None,
        );

        assert_eq!(resolved.host_device.as_deref(), Some("sdb"));
        assert_eq!(resolved.major_minor.as_deref(), Some("8:16"));
        assert_eq!(resolved.dm_name.as_deref(), Some("mpatha"));
        assert_eq!(resolved.wwid.as_deref(), Some("36000d3100001"));
        assert_eq!(resolved.confidence, MappingConfidence::Derived);
        assert_eq!(resolved.unresolved_stage, None);
    }

    #[test]
    fn resolves_backend_params_without_using_frontend_name_as_host_identity() {
        let inventory = vec![HostBlockDevice {
            name: "dm-3".into(),
            kname: "dm-3".into(),
            dm_name: Some("mpatha".into()),
            dm_uuid: Some("mpath-36000d3100001".into()),
            ..Default::default()
        }];
        let resolved = resolve_block_identity(
            "/local/domain/0/backend/vbd/1/51712",
            None,
            Some("/dev/mapper/mpatha"),
            &inventory,
            None,
        );

        assert_eq!(resolved.host_device.as_deref(), Some("dm-3"));
        assert_eq!(resolved.wwid.as_deref(), Some("36000d3100001"));
    }

    #[test]
    fn unresolved_mapping_has_specific_stage_and_reason() {
        let resolved = resolve_block_identity(
            "/local/domain/0/backend/vbd/1/51712",
            Some("ca:10"),
            None,
            &[],
            None,
        );

        assert_eq!(resolved.wwid, None);
        assert_eq!(resolved.unresolved_stage, Some(MappingStage::HostBlock));
        assert!(
            resolved
                .unresolved_reason
                .as_deref()
                .unwrap()
                .contains("ca:10")
        );
    }

    #[test]
    fn retains_direct_wwid_but_reports_missing_device_mapper_stage() {
        let inventory = vec![HostBlockDevice {
            name: "sdb".into(),
            kname: "sdb".into(),
            major_minor: Some("8:16".into()),
            wwn: Some("0x36000d3100001".into()),
            ..Default::default()
        }];
        let resolved = resolve_block_identity(
            "/local/domain/0/backend/vbd/1/51712",
            Some("8:10"),
            None,
            &inventory,
            None,
        );

        assert_eq!(resolved.wwid.as_deref(), Some("36000d3100001"));
        assert_eq!(resolved.unresolved_stage, Some(MappingStage::DeviceMapper));
        assert!(resolved.unresolved_reason.is_some());
    }

    #[test]
    fn file_backed_loop_retains_host_evidence_without_guessing_wwid() {
        let backing_file = "/OVS/Repositories/repo/VirtualDisks/disk.img";
        let inventory = vec![HostBlockDevice {
            name: "loop0".into(),
            kname: "loop0".into(),
            major_minor: Some("7:0".into()),
            ..Default::default()
        }];

        let resolved = resolve_block_identity(
            "/local/domain/0/backend/vbd/1/51712",
            Some("7:0"),
            Some(backing_file),
            &inventory,
            None,
        );

        assert_eq!(resolved.host_device.as_deref(), Some("loop0"));
        assert_eq!(resolved.backing_file.as_deref(), Some(backing_file));
        assert_eq!(resolved.major_minor.as_deref(), Some("7:0"));
        assert_eq!(resolved.wwid, None);
        assert_eq!(resolved.unresolved_stage, Some(MappingStage::DeviceMapper));
        let reason = resolved.unresolved_reason.as_deref().unwrap();
        assert!(reason.contains("host block/loop loop0"));
        assert!(reason.contains(backing_file));
        assert!(reason.contains("no device-mapper/WWID evidence"));
    }

    #[test]
    fn backing_file_uses_longest_containing_mountpoint() {
        let inventory = vec![
            HostBlockDevice {
                name: "dm-0".into(),
                kname: "dm-0".into(),
                mountpoint: Some("/OVS/Repositories".into()),
                ..Default::default()
            },
            HostBlockDevice {
                name: "dm-1".into(),
                kname: "dm-1".into(),
                mountpoint: Some("/OVS/Repositories/repo".into()),
                ..Default::default()
            },
        ];

        assert_eq!(
            mounted_backing_device(
                Some("/OVS/Repositories/repo/VirtualDisks/disk.img"),
                &inventory
            )
            .map(|device| device.name.as_str()),
            Some("dm-1")
        );
    }

    #[test]
    fn backing_file_path_boundary_does_not_match_similar_mountpoint() {
        let inventory = vec![HostBlockDevice {
            name: "dm-0".into(),
            kname: "dm-0".into(),
            mountpoint: Some("/OVS/Repositories/repo".into()),
            ..Default::default()
        }];

        assert!(
            mounted_backing_device(Some("/OVS/Repositories/repository/disk.img"), &inventory)
                .is_none()
        );
    }

    #[test]
    fn file_backed_loop_with_mount_match_derives_multipath_wwid() {
        let backing_file = "/OVS/Repositories/repo/VirtualDisks/disk.img";
        let inventory = vec![
            HostBlockDevice {
                name: "loop0".into(),
                kname: "loop0".into(),
                major_minor: Some("7:0".into()),
                ..Default::default()
            },
            HostBlockDevice {
                name: "dm-3".into(),
                kname: "dm-3".into(),
                mountpoint: Some("/OVS/Repositories/repo".into()),
                dm_name: Some("mpatha".into()),
                dm_uuid: Some("mpath-36000d3100001".into()),
                ..Default::default()
            },
        ];

        let resolved = resolve_block_identity(
            "/local/domain/0/backend/vbd/1/51712",
            Some("7:0"),
            Some(backing_file),
            &inventory,
            None,
        );

        assert_eq!(resolved.host_device.as_deref(), Some("loop0"));
        assert_eq!(resolved.dm_uuid.as_deref(), Some("mpath-36000d3100001"));
        assert_eq!(resolved.wwid.as_deref(), Some("36000d3100001"));
        assert_eq!(resolved.confidence, MappingConfidence::Derived);
        assert_eq!(resolved.unresolved_stage, None);
    }

    #[test]
    fn file_backed_loop_without_mount_match_remains_unresolved() {
        let resolved = resolve_block_identity(
            "/local/domain/0/backend/vbd/1/51712",
            Some("7:0"),
            Some("/OVS/Repositories/missing/VirtualDisks/disk.img"),
            &[HostBlockDevice {
                name: "loop0".into(),
                kname: "loop0".into(),
                major_minor: Some("7:0".into()),
                ..Default::default()
            }],
            None,
        );

        assert_eq!(resolved.host_device.as_deref(), Some("loop0"));
        assert_eq!(resolved.wwid, None);
        assert_eq!(resolved.confidence, MappingConfidence::Unknown);
        assert_eq!(resolved.unresolved_stage, Some(MappingStage::DeviceMapper));
    }

    #[test]
    fn preserves_distinct_xenstore_read_reasons_for_missing_empty_and_failure() {
        let missing = resolve_block_identity_with_diagnostics(
            "/local/domain/0/backend/vbd/1/51712",
            None,
            None,
            &[],
            Some("key missing"),
            Some("key missing (physical-device)"),
            Some("key missing (params)"),
        );
        assert_eq!(missing.unresolved_stage, Some(MappingStage::PhysicalDevice));
        assert!(missing.unresolved_reason.unwrap().contains("key missing"));

        let empty = resolve_block_identity_with_diagnostics(
            "/local/domain/0/backend/vbd/1/51712",
            None,
            None,
            &[],
            Some("key empty"),
            Some("empty value (physical-device)"),
            Some("empty value (params)"),
        );
        assert!(empty.unresolved_reason.unwrap().contains("empty value"));

        let failed = resolve_block_identity_with_diagnostics(
            "/local/domain/0/backend/vbd/1/51712",
            None,
            None,
            &[],
            Some("read failed: permission denied"),
            Some("read failed (physical-device): permission denied"),
            Some("read failed (params): permission denied"),
        );
        assert!(failed.unresolved_reason.unwrap().contains("read failed"));
    }

    #[test]
    fn classifies_empty_and_read_error_diagnostics_without_inventing_identity() {
        assert_eq!(
            read_diagnostic("params", &XenstoreRead::Empty),
            "empty value (params)"
        );
        assert_eq!(
            read_diagnostic("params", &XenstoreRead::Failed("permission denied".into())),
            "read failed (params): permission denied"
        );
    }
}
