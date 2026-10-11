//! cargo run --release -p pulse-probe --example disks
fn main() {
    let mut m = pulse_probe::disk::DiskMonitor::new();
    m.sample();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let t = std::time::Instant::now();
    let disks = m.sample();
    let dt = t.elapsed();
    let gb = |b: u64| b as f64 / 1e9;
    for d in &disks {
        println!(
            "#{} {} [{} {}] {:.0} GB  活动 {:.1}%  读 {} B/s 写 {} B/s 响应 {:.2} ms",
            d.index,
            d.model,
            d.bus,
            match d.ssd {
                Some(true) => "SSD",
                Some(false) => "HDD",
                None => "?",
            },
            gb(d.size),
            d.active,
            d.read_rate,
            d.write_rate,
            d.response_ms
        );
        for v in &d.volumes {
            println!(
                "    {}: {} {} {:.0}/{:.0} GB",
                v.letter,
                v.label,
                v.file_system,
                gb(v.total - v.free),
                gb(v.total)
            );
        }
        println!(
            "    温度 {:?} (警告 {:?} 临界 {:?})",
            d.health.temperature, d.health.warning_temperature, d.health.critical_temperature
        );
        if let Some(n) = &d.health.nvme {
            println!(
                "    NVMe: 已用寿命 {}% 备用 {}%/{}% 通电 {} h 写入 {:.1} TB 读取 {:.1} TB 异常断电 {} 介质错误 {} 警告 0x{:02x}",
                n.percentage_used,
                n.available_spare,
                n.spare_threshold,
                n.power_on_hours,
                n.data_written as f64 / 1e12,
                n.data_read as f64 / 1e12,
                n.unsafe_shutdowns,
                n.media_errors,
                n.critical_warning
            );
        }
    }
    println!("采样耗时 {dt:?}");
}
