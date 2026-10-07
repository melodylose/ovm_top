use crate::topology::snapshot::{DomainBlockDevice, MappingConfidence, VifMapping};
use anyhow::{Context, Result};
use std::fs;
use std::process::Command;

fn read(path: &str) -> Result<String> {
    let output = Command::new("xenstore-read")
        .arg(path)
        .output()
        .with_context(|| format!("failed to read xenstore path {path}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "xenstore-read {path} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

fn by_id_wwid(device: &str) -> Option<String> {
    let target = fs::canonicalize(format!("/sys/class/block/{device}")).ok()?;
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

fn block_identity(
    physical_device: Option<&str>,
    device_name: Option<&str>,
) -> (Option<String>, Option<String>, MappingConfidence) {
    let sysfs_device = physical_device
        .filter(|value| value.contains(':'))
        .and_then(|value| {
            fs::read_to_string(format!("/sys/dev/block/{value}/uevent"))
                .ok()
                .and_then(|uevent| {
                    uevent
                        .lines()
                        .find_map(|line| line.strip_prefix("DEVNAME=").map(str::to_string))
                })
        });
    let device = sysfs_device.as_deref().or(device_name);
    let (uuid, identity_confidence) = device
        .map(|name| {
            if let Some(wwid) = by_id_wwid(name) {
                (Some(wwid), MappingConfidence::Exact)
            } else {
                let dm_uuid = fs::read_to_string(format!("/sys/class/block/{name}/dm/uuid"))
                    .ok()
                    .map(|value| value.trim().to_string())
                    .and_then(|value| value.strip_prefix("mpath-").map(str::to_string));
                (
                    dm_uuid,
                    if sysfs_device.is_some() {
                        MappingConfidence::Derived
                    } else {
                        MappingConfidence::Fallback
                    },
                )
            }
        })
        .unwrap_or((None, MappingConfidence::Unknown));
    let confidence = if uuid.is_some() {
        identity_confidence
    } else {
        MappingConfidence::Unknown
    };
    (sysfs_device, uuid, confidence)
}

pub fn read_domain_block_devices(domid: u32) -> Result<Vec<DomainBlockDevice>> {
    let base = format!("/local/domain/{domid}/device/vbd");
    let mut devices = Vec::new();
    for frontend in list(&base)? {
        let path = format!("{base}/{frontend}");
        let device_name = read(&format!("{path}/device"))
            .or_else(|_| read(&format!("{path}/virtual-device")))
            .ok();
        let backend = read(&format!("{path}/backend")).unwrap_or_default();
        let physical_device = read(&format!("{path}/physical-device")).ok();
        let (resolved_device, wwid, confidence) =
            block_identity(physical_device.as_deref(), device_name.as_deref());
        devices.push(DomainBlockDevice {
            domid,
            frontend,
            backend,
            major_minor: physical_device,
            device_name: resolved_device.or(device_name),
            wwid,
            confidence,
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
    use super::block_identity;
    use crate::topology::snapshot::MappingConfidence;

    #[test]
    fn unknown_block_identity_is_not_guessed() {
        let (device, wwid, confidence) = block_identity(None, Some("device-that-does-not-exist"));

        assert_eq!(device, None);
        assert_eq!(wwid, None);
        assert_eq!(confidence, MappingConfidence::Unknown);
    }

    #[test]
    fn invalid_physical_device_does_not_become_exact_mapping() {
        let (_, wwid, confidence) = block_identity(Some("not-a-major-minor"), None);

        assert_eq!(wwid, None);
        assert_eq!(confidence, MappingConfidence::Unknown);
    }
}
