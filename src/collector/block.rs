use crate::topology::snapshot::HostBlockDevice;
use anyhow::{Context, Result};
use std::{collections::HashMap, fs, path::Path, process::Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockInventorySource {
    Lsblk,
    SysfsFallback,
}

#[derive(Debug, Clone)]
pub struct BlockInventory {
    pub devices: Vec<HostBlockDevice>,
    pub source: BlockInventorySource,
    pub fallback_reason: Option<String>,
}

const REQUIRED_LSBLK_COLUMNS: &str = "NAME,KNAME,MAJ:MIN,TYPE,SIZE,MOUNTPOINT";
const FULL_LSBLK_COLUMNS: &str = "NAME,KNAME,MAJ:MIN,TYPE,SIZE,MODEL,SERIAL,WWN,MOUNTPOINT";

pub fn read_block_inventory() -> Result<BlockInventory> {
    match read_lsblk_inventory() {
        Ok(mut devices) if !devices.is_empty() => {
            enrich_from_sysfs(&mut devices, Path::new("/sys/class/block"));
            Ok(BlockInventory {
                devices,
                source: BlockInventorySource::Lsblk,
                fallback_reason: None,
            })
        }
        Ok(_) => fallback_to_sysfs("lsblk returned no block devices".into()),
        Err(error) => fallback_to_sysfs(format!("lsblk capability unavailable: {error:#}")),
    }
}

fn fallback_to_sysfs(reason: String) -> Result<BlockInventory> {
    let devices = read_sysfs_inventory(Path::new("/sys/class/block"))?;
    if devices.is_empty() {
        anyhow::bail!("{reason}; sysfs returned no block devices");
    }
    Ok(BlockInventory {
        devices,
        source: BlockInventorySource::SysfsFallback,
        fallback_reason: Some(reason),
    })
}

fn read_lsblk_inventory() -> Result<Vec<HostBlockDevice>> {
    match run_lsblk(FULL_LSBLK_COLUMNS).and_then(|output| parse_lsblk_output(&output)) {
        Ok(devices) => Ok(devices),
        Err(full_error) => run_lsblk(REQUIRED_LSBLK_COLUMNS)
            .and_then(|output| parse_lsblk_output(&output))
            .with_context(|| format!("full lsblk fields unavailable: {full_error:#}")),
    }
}

fn run_lsblk(columns: &str) -> Result<String> {
    let output = Command::new("lsblk")
        .args(["-P", "-b", "-o", columns])
        .output()
        .context("execute lsblk inventory")?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!(
            "{}",
            if reason.is_empty() {
                format!("lsblk exited with status {}", output.status)
            } else {
                reason
            }
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_lsblk_output(output: &str) -> Result<Vec<HostBlockDevice>> {
    parse_lsblk_pairs(output)
}

fn parse_lsblk_pairs(output: &str) -> Result<Vec<HostBlockDevice>> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields = parse_key_value_line(line)?;
            for required in REQUIRED_LSBLK_COLUMNS.split(',') {
                field(&fields, required)?;
            }
            let name = field(&fields, "NAME")?;
            Ok(HostBlockDevice {
                name: name.to_string(),
                kname: fields
                    .get("KNAME")
                    .filter(|value| !value.is_empty())
                    .cloned()
                    .unwrap_or_else(|| name.to_string()),
                major_minor: fields.get("MAJ:MIN").cloned().filter(not_empty),
                device_type: fields.get("TYPE").cloned().unwrap_or_default(),
                size_bytes: fields.get("SIZE").and_then(|value| value.parse().ok()),
                model: fields.get("MODEL").cloned().filter(not_empty),
                serial: fields.get("SERIAL").cloned().filter(not_empty),
                wwn: fields.get("WWN").cloned().filter(not_empty),
                mountpoint: fields.get("MOUNTPOINT").cloned().filter(not_empty),
                ..Default::default()
            })
        })
        .collect()
}

fn parse_key_value_line(line: &str) -> Result<HashMap<String, String>> {
    let mut fields = HashMap::new();
    let mut rest = line.trim();
    while !rest.is_empty() {
        let Some((key, value_rest)) = rest.split_once("=\"") else {
            anyhow::bail!("invalid lsblk key/value output: {rest}");
        };
        let mut value = String::new();
        let mut escaped = false;
        let mut end = None;
        for (index, character) in value_rest.char_indices() {
            if escaped {
                value.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                end = Some(index);
                break;
            } else {
                value.push(character);
            }
        }
        let end = end.context("unterminated lsblk value")?;
        fields.insert(key.trim().to_string(), value);
        rest = value_rest[end + 1..].trim_start();
    }
    Ok(fields)
}

fn field<'a>(fields: &'a HashMap<String, String>, name: &str) -> Result<&'a str> {
    fields
        .get(name)
        .map(String::as_str)
        .with_context(|| format!("lsblk output missing {name}"))
}

fn not_empty(value: &String) -> bool {
    !value.is_empty()
}

fn read_sysfs_inventory(root: &Path) -> Result<Vec<HostBlockDevice>> {
    let mut devices = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let sectors = read_trimmed(&path.join("size")).and_then(|value| value.parse::<u64>().ok());
        devices.push(HostBlockDevice {
            name: name.clone(),
            kname: name,
            major_minor: read_trimmed(&path.join("dev")),
            device_type: if path.join("partition").exists() {
                "part".into()
            } else if path.join("dm").exists() {
                "dm".into()
            } else {
                "disk".into()
            },
            size_bytes: sectors.map(|value| value.saturating_mul(512)),
            model: read_trimmed(&path.join("device/model")),
            serial: read_trimmed(&path.join("device/serial")),
            wwn: read_trimmed(&path.join("device/wwid")),
            dm_name: read_trimmed(&path.join("dm/name")),
            dm_uuid: read_trimmed(&path.join("dm/uuid")),
            holders: read_names(&path.join("holders")),
            ..Default::default()
        });
    }
    devices.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(devices)
}

fn enrich_from_sysfs(devices: &mut [HostBlockDevice], root: &Path) {
    for device in devices {
        let path = root.join(&device.kname);
        device.dm_name = read_trimmed(&path.join("dm/name"));
        device.dm_uuid = read_trimmed(&path.join("dm/uuid"));
        device.holders = read_names(&path.join("holders"));
        if device.wwn.is_none() {
            device.wwn = read_trimmed(&path.join("device/wwid"));
        }
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_names(path: &Path) -> Vec<String> {
    let mut names = fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_old_lsblk_pair_format_without_json_support() {
        let devices = parse_lsblk_pairs(
            "NAME=\"dm-3\" KNAME=\"dm-3\" MAJ:MIN=\"253:3\" TYPE=\"mpath\" SIZE=\"4096\" MODEL=\"Compellent Vol\" SERIAL=\"\" WWN=\"0x1234\" MOUNTPOINT=\"\"\n",
        )
        .unwrap();

        assert_eq!(devices[0].major_minor.as_deref(), Some("253:3"));
        assert_eq!(devices[0].wwn.as_deref(), Some("0x1234"));
        assert_eq!(devices[0].size_bytes, Some(4096));
    }

    #[test]
    fn parses_legacy_lsblk_when_optional_columns_are_unavailable() {
        let devices = parse_lsblk_pairs(
            "NAME=\"sda\" KNAME=\"sda\" MAJ:MIN=\"8:0\" TYPE=\"disk\" SIZE=\"8192\" MOUNTPOINT=\"\"\n",
        )
        .unwrap();

        assert_eq!(devices[0].name, "sda");
        assert_eq!(devices[0].size_bytes, Some(8192));
        assert_eq!(devices[0].model, None);
        assert_eq!(devices[0].serial, None);
        assert_eq!(devices[0].wwn, None);
    }

    #[test]
    fn reads_sysfs_fallback_with_dm_identity_and_holders() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("ovm-top-sysfs-{}-{unique}", std::process::id()));
        let device = root.join("dm-3");
        fs::create_dir_all(device.join("dm")).unwrap();
        fs::create_dir_all(device.join("holders/dm-9")).unwrap();
        fs::create_dir_all(device.join("device")).unwrap();
        fs::write(device.join("dev"), "253:3\n").unwrap();
        fs::write(device.join("size"), "8\n").unwrap();
        fs::write(device.join("dm/name"), "mpatha\n").unwrap();
        fs::write(device.join("dm/uuid"), "mpath-3600abc\n").unwrap();

        let devices = read_sysfs_inventory(&root).unwrap();

        assert_eq!(devices[0].size_bytes, Some(4096));
        assert_eq!(devices[0].dm_name.as_deref(), Some("mpatha"));
        assert_eq!(devices[0].dm_uuid.as_deref(), Some("mpath-3600abc"));
        assert_eq!(devices[0].holders, vec!["dm-9"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enriches_lsblk_device_from_sysfs() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ovm-top-enrichment-{}-{unique}",
            std::process::id()
        ));
        let device = root.join("dm-3");
        fs::create_dir_all(device.join("dm")).unwrap();
        fs::create_dir_all(device.join("holders/dm-9")).unwrap();
        fs::create_dir_all(device.join("device")).unwrap();
        fs::write(device.join("dm/name"), "mpatha\n").unwrap();
        fs::write(device.join("dm/uuid"), "mpath-3600abc\n").unwrap();
        fs::write(device.join("device/wwid"), "3600abc\n").unwrap();

        let mut devices = vec![HostBlockDevice {
            name: "dm-3".into(),
            kname: "dm-3".into(),
            ..Default::default()
        }];
        enrich_from_sysfs(&mut devices, &root);

        assert_eq!(devices[0].dm_name.as_deref(), Some("mpatha"));
        assert_eq!(devices[0].dm_uuid.as_deref(), Some("mpath-3600abc"));
        assert_eq!(devices[0].wwn.as_deref(), Some("3600abc"));
        assert_eq!(devices[0].holders, vec!["dm-9"]);
        fs::remove_dir_all(root).unwrap();
    }
}
