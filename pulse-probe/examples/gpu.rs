//! cargo run --release -p pulse-probe --example gpu
fn main() {
    let mut c = pulse_probe::Collector::new();
    c.sample();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mut s = c.sample();
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    for g in &s.gpus {
        println!(
            "{} {:.1}%  专用 {:.2}/{:.2} GB  共享 {:.2}/{:.2} GB",
            g.name,
            g.usage,
            gb(g.dedicated_used),
            gb(g.dedicated_total),
            gb(g.shared_used),
            gb(g.shared_total)
        );
        println!(
            "  温度 {:?}°C（警告 {:?} / 最高 {:?}）  风扇 {:?} RPM  功耗 {:?}",
            g.temperature, g.warning_temperature, g.max_temperature, g.fan_rpm, g.power
        );
        for (name, u) in &g.engines {
            println!("  {name:<8} {u:5.1}%");
        }
    }
    s.processes
        .sort_by_key(|p| std::cmp::Reverse(p.gpu_dedicated));
    for p in s.processes.iter().take(6) {
        println!(
            "{:>7} {:<28} 显存 {:>7.0} MB  共享 {:>5.0} MB  CPU 时间 {:.0}s",
            p.pid,
            p.name,
            p.gpu_dedicated as f64 / 1048576.0,
            p.gpu_shared as f64 / 1048576.0,
            (p.user_time + p.kernel_time) as f64 / 1e7
        );
    }
    println!("采集 {:?}", s.collect_time);
}
