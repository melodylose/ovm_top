use anyhow::Result;
use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex, mpsc::Sender},
    thread,
    time::{Duration, Instant},
};

use crate::log::{LogEvent, LogSource};

const SECTOR_SIZE: u64 = 512;

#[derive(Debug, Clone, Default)]
pub struct DiskStats {
    pub name: String,
    pub reads_completed: u64,
    pub sectors_read: u64,
    pub writes_completed: u64,
    pub sectors_written: u64,
    pub io_time_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct DiskRate {
    pub name: String,
    pub read_bytes_per_sec: u64,
    pub write_bytes_per_sec: u64,
    pub read_iops: u64,
    pub write_iops: u64,
    pub utilization_percent: f64,
}

pub fn spawn_disk_collector(output: Arc<Mutex<Vec<DiskRate>>>, log_tx: Sender<LogEvent>) {
    thread::spawn(move || {
        let mut previous = match read_diskstats() {
            Ok(stats) => stats,
            Err(error) => {
                let _ = log_tx.send(LogEvent::error(
                    LogSource::Disk,
                    format!("failed to read /proc/diskstats: {error}"),
                ));
                return;
            }
        };
        let mut previous_at = Instant::now();

        loop {
            thread::sleep(Duration::from_secs(1));

            let current = match read_diskstats() {
                Ok(stats) => stats,
                Err(error) => {
                    let _ = log_tx.send(LogEvent::warn(
                        LogSource::Disk,
                        format!("failed to refresh /proc/diskstats: {error}"),
                    ));
                    continue;
                }
            };

            let now = Instant::now();
            let rates = calculate_rates(&previous, &current, now.duration_since(previous_at));

            if let Ok(mut shared) = output.lock() {
                *shared = rates;
            }

            previous = current;
            previous_at = now;
        }
    });
}

pub fn calculate_rates(
    previous: &[DiskStats],
    current: &[DiskStats],
    elapsed: Duration,
) -> Vec<DiskRate> {
    current
        .iter()
        .filter(|stat| is_display_device(&stat.name))
        .filter_map(|curr| calculate_rate(previous, curr, elapsed))
        .collect()
}

pub fn calculate_rate(
    previous: &[DiskStats],
    current: &DiskStats,
    elapsed: Duration,
) -> Option<DiskRate> {
    let previous_map: HashMap<&str, &DiskStats> = previous
        .iter()
        .map(|stat| (stat.name.as_str(), stat))
        .collect();
    let prev = previous_map.get(current.name.as_str())?;
    let elapsed_seconds = elapsed.as_secs_f64().max(f64::EPSILON);
    let elapsed_ms = elapsed.as_millis().max(1) as f64;
    let read_sectors = current.sectors_read.saturating_sub(prev.sectors_read);
    let write_sectors = current.sectors_written.saturating_sub(prev.sectors_written);
    let reads = current.reads_completed.saturating_sub(prev.reads_completed);
    let writes = current
        .writes_completed
        .saturating_sub(prev.writes_completed);
    let io_ms = current.io_time_ms.saturating_sub(prev.io_time_ms);

    Some(DiskRate {
        name: current.name.clone(),
        read_bytes_per_sec: (read_sectors as f64 * SECTOR_SIZE as f64 / elapsed_seconds) as u64,
        write_bytes_per_sec: (write_sectors as f64 * SECTOR_SIZE as f64 / elapsed_seconds) as u64,
        read_iops: (reads as f64 / elapsed_seconds) as u64,
        write_iops: (writes as f64 / elapsed_seconds) as u64,
        utilization_percent: ((io_ms as f64 / elapsed_ms) * 100.0).min(100.0),
    })
}

pub fn read_diskstats() -> Result<Vec<DiskStats>> {
    let content = fs::read_to_string("/proc/diskstats")?;
    let mut disks = Vec::new();

    for line in content.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 14 {
            continue;
        }

        disks.push(DiskStats {
            name: cols[2].to_string(),
            reads_completed: cols[3].parse().unwrap_or_default(),
            sectors_read: cols[5].parse().unwrap_or_default(),
            writes_completed: cols[7].parse().unwrap_or_default(),
            sectors_written: cols[9].parse().unwrap_or_default(),
            io_time_ms: cols[12].parse().unwrap_or_default(),
        });
    }

    Ok(disks)
}

pub fn is_display_device(name: &str) -> bool {
    if name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("sr")
        || name.starts_with("dm-")
    {
        return false;
    }

    !name
        .chars()
        .last()
        .is_some_and(|last| last.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_rates_using_elapsed_time() {
        let previous = vec![DiskStats {
            name: "sda".into(),
            ..Default::default()
        }];
        let current = vec![DiskStats {
            name: "sda".into(),
            reads_completed: 20,
            sectors_read: 2048,
            writes_completed: 10,
            sectors_written: 1024,
            io_time_ms: 500,
        }];

        let rates = calculate_rates(&previous, &current, Duration::from_secs(2));
        assert_eq!(rates[0].read_bytes_per_sec, 512 * 1024);
        assert_eq!(rates[0].write_bytes_per_sec, 256 * 1024);
        assert_eq!(rates[0].read_iops, 10);
        assert_eq!(rates[0].write_iops, 5);
        assert_eq!(rates[0].utilization_percent, 25.0);
    }

    #[test]
    fn filters_virtual_and_partition_devices() {
        assert!(is_display_device("sda"));
        assert!(is_display_device("xvda"));
        assert!(!is_display_device("sda1"));
        assert!(!is_display_device("loop0"));
        assert!(!is_display_device("dm-0"));
    }
}
